//! Wire `flockmux-shim <real-cli> <args...>` together for a single agent.

use crate::plugins::CliPlugin;
use crate::pty_stream::PtyStream;
use crate::registry::{AgentSlot, Lifecycle, LifecycleEvent};
use anyhow::{Context, Result};
use bytes::Bytes;
use flockmux_pty::{PtyBridge, PtyHandles, SpawnOpts};
use flockmux_recorder::RecorderHandle;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use uuid::Uuid;

// OSC markers the shim emits — kept identical to flockmux-shim/src/main.rs.
const OSC_READY: &[u8] = b"\x1b]633;A\x07";
const OSC_EXIT_PREFIX: &[u8] = b"\x1b]633;D;";
const OSC_TERMINATOR: u8 = 0x07;

pub struct AgentSpawn {
    pub agent_id: String,
    pub slot: AgentSlot,
    /// For claude: the `--session-id` UUID we force at spawn, so the transcript
    /// tailer locates `~/.claude/projects/<enc-cwd>/<uuid>.jsonl` EXACTLY rather
    /// than guessing "newest .jsonl" (a stale prior-session file in the same
    /// project dir would otherwise win). None for codex — it locates via its
    /// isolated per-agent CODEX_HOME, which has no stale-file problem.
    pub transcript_session_id: Option<String>,
}

/// How [`spawn_agent`] resolves the workspace directory for a fresh
/// agent. Two strategies cover every current call site:
///
/// - [`WorkspaceLayout::PerAgent`] — the historical default: each agent
///   gets its own `<root>/<agent_id>/` subdirectory. Used by
///   `POST /api/agent` and by spells that don't set
///   `shared_workspace`.
/// - [`WorkspaceLayout::Shared`] — every caller agent runs in the same
///   absolute directory. M6a fullstack-feature spells use this so FE /
///   BE / Test peer agents see the same monorepo (`apps/frontend`,
///   `apps/backend`, `tests/`).
///
/// Kept as an enum (not a `bool` parameter) so future strategies — e.g.
/// `Worktree(git_repo, branch)` for M6b — slot in without touching
/// every call site again.
#[derive(Debug, Clone)]
pub enum WorkspaceLayout {
    PerAgent { root: PathBuf },
    Shared { dir: PathBuf },
}

/// `shim_path` is the absolute path to `flockmux-shim`. Caller normally
/// derives it from `std::env::current_exe()` parent + "flockmux-shim".
///
/// `mcp_bin` is the absolute path to `flockmux-mcp` (the swarm MCP stdio
/// server). It's baked into per-spawn MCP config entries written under
/// pre-spawn patches so claude / codex can launch it on first tool call.
///
/// `server_url` is the base URL of the flockmux-server REST API that the
/// MCP subprocess will speak to. Loopback today.
///
/// `recorder` is an optional asciicast v2 sink. When set, the PTY pump
/// mirrors every chunk (including OSC lifecycle markers) into the
/// recorder; when unset, the recording layer is bypassed.
#[allow(clippy::too_many_arguments)]
pub fn spawn_agent(
    plugin: &CliPlugin,
    role: Option<String>,
    model: Option<String>,
    reasoning: Option<String>,
    workspace: &WorkspaceLayout,
    shim_path: &Path,
    mcp_bin: &Path,
    server_url: &str,
    recorder: Option<RecorderHandle>,
) -> Result<AgentSpawn> {
    let agent_id = format!("{}-{}", plugin.id, &Uuid::new_v4().to_string()[..8]);
    let workspace = match workspace {
        WorkspaceLayout::PerAgent { root } => ensure_workspace(root, &agent_id)?,
        WorkspaceLayout::Shared { dir } => ensure_shared_workspace(dir)?,
    };

    // L4 transport seam: the ACP (structured JSON-RPC-over-stdio) session
    // driver isn't wired yet. `crate::acp` now has both the codec AND the async
    // `Connection` layer (request/response correlation + notification/peer-
    // request channels) — what's still missing is the ACP-specific session on
    // top: spawn the child with PIPED stdio (not a PTY), `Connection::spawn` on
    // it, do the `initialize` handshake, and map ACP notifications (permission
    // / tool-call / streaming) onto SwarmEvents. That needs a live ACP CLI to
    // pin the schema, so for now a plugin declaring `transport = "acp"` still
    // spawns over the PTY (usable today); this warning marks the branch point.
    if plugin.transport == crate::plugins::Transport::Acp {
        tracing::warn!(
            agent = %agent_id, cli = %plugin.id,
            "transport=acp declared but the ACP session driver isn't wired yet \
             (L4 foundation only); falling back to the PTY transport"
        );
    }

    // Suppress per-CLI interactive prompts that would block a headless PTY
    // (claude's "trust folder", codex's "update available"). Each patch is a
    // no-op when not configured / not applicable. The MCP-inject patch is
    // also routed here since it shares the "pre-spawn home dir mutation"
    // shape.
    let pre_ctx = crate::pre_spawn::PreSpawnCtx {
        agent_id: agent_id.clone(),
        mcp_bin: mcp_bin.to_path_buf(),
        server_url: server_url.to_string(),
    };
    crate::pre_spawn::run_patches(plugin, &workspace, &pre_ctx);

    let mut argv = Vec::with_capacity(2 + plugin.default_args.len() + 1);
    argv.push(shim_path.to_string_lossy().into_owned());
    argv.push(plugin.binary.clone());
    argv.extend(plugin.default_args.iter().cloned());

    // L5c model overlay: a model passed at spawn time (REST/MCP) wins over the
    // plugin's default_model. The flag itself (claude & codex both use
    // `--model <v>`) lives in the manifest's `model_args` template, not here —
    // host ≠ model: the same CLI runs any model with zero Rust/role forking.
    match model.or_else(|| plugin.default_model.clone()) {
        Some(m) if !plugin.model_args.is_empty() => {
            argv.extend(model_overlay_args(&m, &plugin.model_args));
            tracing::info!(agent = %agent_id, model = %m, "model overlay applied");
        }
        Some(m) => tracing::warn!(
            agent = %agent_id, model = %m,
            "model requested but plugin declares no model_args; ignoring"
        ),
        None => {}
    }

    // Reasoning/thinking effort overlay (parallel to model). The abstract level
    // (low|medium|high|max) maps to this CLI's concrete value via the manifest's
    // effort_levels; the result substitutes into effort_args (claude `--effort`,
    // codex `-c model_reasoning_effort=`). Unknown/"default" level or a CLI with
    // no effort support → emit nothing = the model's own default.
    if let Some(level) = reasoning.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        match plugin.effort_levels.get(level) {
            Some(concrete) if !plugin.effort_args.is_empty() => {
                argv.extend(effort_overlay_args(concrete, &plugin.effort_args));
                tracing::info!(agent = %agent_id, effort = %level, concrete = %concrete, "reasoning effort overlay applied");
            }
            _ => tracing::debug!(
                agent = %agent_id, effort = %level,
                "reasoning effort requested but unmapped / unsupported by this CLI; ignoring"
            ),
        }
    }

    // codex 0.130 gates non-managed Stop hooks behind an in-app /hooks
    // trust-review prompt — workspace-local hooks.json gets installed but
    // never executes until the user manually approves it. PR #21768 ships
    // `--dangerously-bypass-hook-trust` to skip the review for automation
    // hosts like us. The flag isn't in 0.130 yet (codex aborts spawn on
    // unknown argv), so probe `<binary> --help` once per process and only
    // inject the flag if it's already supported. Net effect:
    //   - codex 0.130: probe -> false, argv unchanged, hooks.json stays
    //     dormant (known constraint, documented in auto-memory).
    //   - codex >=0.131 (future): probe -> true, flag injected, our
    //     existing hooks.json install becomes immediately effective with
    //     zero config change on flockmux's side.
    // Gated on the codex hooks.json format (not the literal id) + a binary
    // probe, so it's the *capability* that drives it: any CLI declaring
    // `stop_hook_format = "codex-hooks-json"` whose binary supports the flag
    // gets it; others never do.
    if plugin.stop_hook_format == crate::plugins::StopHookFormat::CodexHooksJson
        && binary_supports_flag(&plugin.binary, "--dangerously-bypass-hook-trust")
    {
        argv.push("--dangerously-bypass-hook-trust".into());
        tracing::info!(
            agent = %agent_id,
            "--dangerously-bypass-hook-trust supported; injecting"
        );
    }

    // claude: point at the per-agent MCP config file pre_spawn just wrote.
    // `--strict-mcp-config` makes claude ignore `~/.claude.json` entirely so
    // a sibling spawn that overwrote the workspace's mcpServers section (the
    // shared_workspace collision that hung M6b run #4) can no longer leak
    // someone else's agent_id into our MCP server. Skipped if the file
    // wasn't written (no $HOME) — fall back to legacy ~/.claude.json path.
    if plugin.mcp_format == crate::plugins::McpFormat::ClaudeLocalScope && plugin.auto_inject_mcp {
        if let Some(path) = crate::pre_spawn::claude_per_agent_mcp_config_path(&agent_id) {
            if path.is_file() {
                argv.push("--mcp-config".into());
                argv.push(path.to_string_lossy().into_owned());
                argv.push("--strict-mcp-config".into());
                tracing::info!(
                    agent = %agent_id,
                    mcp_config = %path.display(),
                    "claude per-agent MCP config injected (bypasses ~/.claude.json collision)"
                );
            } else {
                tracing::warn!(
                    agent = %agent_id,
                    mcp_config = %path.display(),
                    "claude per-agent MCP config missing on disk; falling back to ~/.claude.json"
                );
            }
        }
    }

    // Env: flockmux-pty starts the child from an EMPTY environment
    // (`CommandBuilder::env_clear`), so the worker sees ONLY what we insert
    // here. We forward what a CLI legitimately needs (HOME for OAuth creds,
    // PATH, locale, proxy/TLS, and the provider's own config/keys) but NOT
    // arbitrary shell secrets the server happened to be launched with
    // (AWS_*, GITHUB_TOKEN, DB creds, …). This is the allowlist that realises
    // the per-agent isolation goal.
    let mut env = HashMap::new();
    let home_var = if plugin.home_env.is_empty() {
        "HOME"
    } else {
        &plugin.home_env
    };
    if let Ok(home) = std::env::var(home_var) {
        env.insert("HOME".into(), home);
    }
    // Useful unicode default — many CLIs probe LANG.
    if let Ok(lang) = std::env::var("LANG") {
        env.insert("LANG".into(), lang);
    } else {
        env.insert("LANG".into(), "en_US.UTF-8".into());
    }
    // PATH: keep the parent's so the inner CLI can resolve its own subcommands
    // (e.g. `claude doctor` may exec `node`).
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".into(), path);
    }
    // Non-secret runtime essentials the CLI may need to reach the network /
    // render unicode. Forwarded only if present. Locale (LC_*), temp dir,
    // HTTP(S) proxy, and TLS CA bundles cover corporate / proxied setups
    // (e.g. routing codex through a relay) that would otherwise break once we
    // stopped inheriting the full env.
    for key in [
        "LC_ALL",
        "LC_CTYPE",
        "TMPDIR",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "NODE_EXTRA_CA_CERTS",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "REQUESTS_CA_BUNDLE",
    ] {
        if let Ok(v) = std::env::var(key) {
            env.insert(key.into(), v);
        }
    }
    // Provider config/creds the inner CLI authenticates with, matched by
    // prefix so e.g. ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN / OPENAI_BASE_URL
    // pass through (a worker still authenticates), while unrelated *_TOKEN /
    // *_KEY shell secrets do NOT. `or_insert` so explicit entries below (e.g.
    // per-agent CODEX_HOME) always win over an inherited value.
    for (k, v) in std::env::vars() {
        if ["ANTHROPIC_", "OPENAI_", "CLAUDE_"]
            .iter()
            .any(|p| k.starts_with(p))
        {
            env.entry(k).or_insert(v);
        }
    }
    // Identity env passed to the CLI. codex picks `FLOCKMUX_AGENT_ID` /
    // `FLOCKMUX_SERVER_URL` up via the `env_vars` whitelist in
    // ~/.codex/config.toml and forwards them to the MCP subprocess. claude
    // also forwards them by spec (any vars present in the spawn env that
    // match the MCP entry's `env` block) — and the local-scope MCP entry
    // we write already lists them explicitly, so this is belt + braces.
    env.insert("FLOCKMUX_AGENT_ID".into(), agent_id.clone());
    env.insert("FLOCKMUX_SERVER_URL".into(), server_url.to_string());

    // codex: point the worker at its per-agent CODEX_HOME (written by
    // pre_spawn) so it loads an ISOLATED config with ONLY flockmux-swarm —
    // not the user's personal ~/.codex MCP servers (chrome-devtools, pencil,
    // …), which stall a headless worker at startup. Mirrors claude's
    // `--strict-mcp-config`. Gated on the per-agent config.toml existing;
    // otherwise codex falls back to the global ~/.codex (still has the block).
    if plugin.mcp_format == crate::plugins::McpFormat::CodexGlobalToml && plugin.auto_inject_mcp {
        if let Some(home) = crate::pre_spawn::codex_per_agent_home_path(&agent_id) {
            if home.join("config.toml").is_file() {
                env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
                tracing::info!(
                    agent = %agent_id,
                    codex_home = %home.display(),
                    "codex per-agent CODEX_HOME injected (isolates MCP from user's global ~/.codex)"
                );
            }
        }
    }

    // claude: force a known session id so the transcript tailer locates the
    // exact session JSONL (`<uuid>.jsonl`) instead of guessing the newest file
    // in the project dir — a stale prior session in the same workspace would
    // otherwise win. codex gets None (it locates via its per-agent CODEX_HOME).
    let transcript_session_id =
        if plugin.mcp_format == crate::plugins::McpFormat::ClaudeLocalScope {
            let sid = Uuid::new_v4().to_string();
            argv.push("--session-id".into());
            argv.push(sid.clone());
            tracing::info!(agent = %agent_id, session_id = %sid, "claude --session-id forced for transcript location");
            Some(sid)
        } else {
            None
        };

    let argv_strings: Vec<String> = argv;

    let PtyHandles { bridge, output_rx } = PtyBridge::spawn(SpawnOpts {
        argv: &argv_strings,
        cwd: Some(&workspace),
        env,
        cols: 120,
        rows: 32,
    })
    .with_context(|| format!("PtyBridge::spawn for {}", plugin.id))?;

    let input_tx = bridge.input_sender();
    let bridge = Arc::new(bridge);

    // Post-spawn readiness automation: auto-answer first-spawn confirmation
    // dialogs that would block a headless PTY (e.g. codex 0.130+'s "Hooks
    // need review" menu — the user can't reach the codex TUI to pick "2
    // Trust all and continue" because the dialog gates input). Steps are
    // declared in the plugin manifest's `ready_plan`, so claude (no such
    // dialog) ships an empty plan and this is `None`.
    let ready_plan = ReadyPlanRunner::from_plan(&plugin.ready_plan, input_tx.clone(), &agent_id);

    // Drain the PTY's output mpsc into the shared resume buffer. The pump
    // owns the receiver for the agent's whole lifetime — WS subscribers
    // read from the buffer, never the mpsc. EOF closes the buffer so any
    // WS writer task that's parked on `wait_changed` wakes up and exits.
    //
    // The pump is also where OSC lifecycle markers are scanned — exactly
    // once per agent, so multi-attach subscribers don't each redundantly
    // re-detect ShimReady/ShimExit, and resume attaches (whose cursor may
    // skip past the original OSC) can be told the current lifecycle via
    // Hello + the broadcast.
    let stream = Arc::new(PtyStream::new());
    let lifecycle = Arc::new(Mutex::new(Lifecycle::default()));
    // capacity=16 holds ShimReady + ShimExit plus headroom even if both
    // events fire faster than the slowest WS subscriber drains them.
    let (lifecycle_tx, _lifecycle_rx) = tokio::sync::broadcast::channel(16);
    {
        let stream = stream.clone();
        let lifecycle = lifecycle.clone();
        let lifecycle_tx = lifecycle_tx.clone();
        let agent_id_for_log = agent_id.clone();
        let recorder = recorder.clone();
        let mut ready_plan = ready_plan;
        tokio::spawn(async move {
            let mut output_rx = output_rx;
            let mut osc_buf: Vec<u8> = Vec::new();
            while let Some(chunk) = output_rx.recv().await {
                scan_osc(&mut osc_buf, &chunk, &lifecycle, &lifecycle_tx);
                if let Some(rp) = ready_plan.as_mut() {
                    rp.scan(&chunk);
                }
                if let Some(rec) = &recorder {
                    rec.write_chunk(chunk.clone());
                }
                stream.append(chunk);
            }
            tracing::debug!(agent = %agent_id_for_log, "pty output drained, closing stream");
            stream.close();
            // Dropping `recorder` here is what signals EOF to the writer
            // task — every clone of the handle (including this pump's) is
            // gone, the mpsc closes, and `wait_finalize` resolves.
            drop(recorder);
        });
    }

    let slot = AgentSlot {
        bridge,
        stream,
        lifecycle,
        lifecycle_tx,
        input_tx,
        cli: plugin.id.clone(),
        role: role.unwrap_or_else(|| plugin.id.clone()),
        workspace: workspace.to_string_lossy().into_owned(),
        paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        // Starts false; flipped true when the agent's flockmux-mcp pings
        // /api/agent/:id/mcp-ready. We keep only the Sender — subscribers call
        // `.subscribe()`; `send_replace` updates the retained value even before
        // any subscriber exists, so an early ping is never lost.
        mcp_ready: tokio::sync::watch::channel(false).0,
    };

    Ok(AgentSpawn {
        agent_id,
        slot,
        transcript_session_id,
    })
}

/// Server-side OSC scanner. Identical algorithm to the M2 client-side
/// scanner in `pty_ws.rs`, lifted here so it runs exactly once per agent
/// regardless of how many WS subscribers are attached.
///
/// Updates `lifecycle` in place and broadcasts state-change events.
/// `osc_buf` carries state across calls so OSC sequences split across
/// `read(2)` boundaries are still matched.
fn scan_osc(
    osc_buf: &mut Vec<u8>,
    chunk: &[u8],
    lifecycle: &Mutex<Lifecycle>,
    tx: &tokio::sync::broadcast::Sender<LifecycleEvent>,
) {
    osc_buf.extend_from_slice(chunk);

    loop {
        // Bound growth; OSC sequences are short, anything beyond a few KB
        // means we're holding non-OSC junk indefinitely.
        if osc_buf.len() > 4096 {
            let keep_from = osc_buf.len() - 256;
            osc_buf.drain(..keep_from);
        }

        if let Some(pos) = find(osc_buf, OSC_READY) {
            osc_buf.drain(..pos + OSC_READY.len());
            let already = {
                let mut l = lifecycle.lock();
                let prev = l.shim_ready;
                l.shim_ready = true;
                prev
            };
            if !already {
                let _ = tx.send(LifecycleEvent::ShimReady);
            }
            continue;
        }
        if let Some(pos) = find(osc_buf, OSC_EXIT_PREFIX) {
            let after = pos + OSC_EXIT_PREFIX.len();
            if let Some(end_rel) = osc_buf[after..].iter().position(|&b| b == OSC_TERMINATOR) {
                let code_bytes = &osc_buf[after..after + end_rel];
                let code = std::str::from_utf8(code_bytes)
                    .ok()
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(-1);
                osc_buf.drain(..after + end_rel + 1);
                let new_exit = {
                    let mut l = lifecycle.lock();
                    if l.shim_exit.is_some() {
                        false
                    } else {
                        l.shim_exit = Some(code);
                        true
                    }
                };
                if new_exit {
                    let _ = tx.send(LifecycleEvent::ShimExit(code));
                }
                continue;
            } else {
                // OSC exit prefix matched but terminator not in this chunk;
                // wait for more bytes.
                break;
            }
        }
        break;
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn ensure_workspace(root: &Path, agent_id: &str) -> Result<PathBuf> {
    let dir = root.join(agent_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create workspace {}", dir.display()))?;
    // Resolve symlinks so the path matches what the spawned CLI sees after
    // its own canonicalize step. On macOS `/tmp` is a symlink to
    // `/private/tmp`, and claude / codex both trust by canonical path —
    // without this, our trust-patch key wouldn't match and the prompt
    // resurfaces.
    let canonical = std::fs::canonicalize(&dir)
        .with_context(|| format!("canonicalize workspace {}", dir.display()))?;
    Ok(canonical)
}

/// Shared-workspace variant: the caller supplies the final directory
/// (typically `<workspaces_root>/spell-<uuid>/` or a user-supplied
/// monorepo path). We `create_dir_all` + canonicalize for the same
/// trust-by-canonical-path reason as `ensure_workspace`.
fn ensure_shared_workspace(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("create shared workspace {}", dir.display()))?;
    let canonical = std::fs::canonicalize(dir)
        .with_context(|| format!("canonicalize shared workspace {}", dir.display()))?;
    Ok(canonical)
}

/// Find `flockmux-shim` next to the current executable. Falls back to
/// `target/debug/flockmux-shim` relative to the manifest dir during
/// `cargo run`, since `current_exe` points into `target/debug/deps/...`
/// for tests but `target/debug/` for `cargo run`.
pub fn locate_shim() -> Result<PathBuf> {
    locate_sibling_bin("flockmux-shim", "FLOCKMUX_SHIM_PATH")
}

/// Find `flockmux-mcp` next to the current executable (same heuristic as
/// `locate_shim`). The path is baked into MCP config entries so claude /
/// codex can launch it directly.
pub fn locate_mcp() -> Result<PathBuf> {
    locate_sibling_bin("flockmux-mcp", "FLOCKMUX_MCP_PATH")
}

/// Host-side post-spawn readiness automation, built from a plugin's
/// `ready_plan` ([`crate::plugins::ReadyStep`]). A **sequential** golutra-style
/// state machine: the host advances a `cursor` through the steps in declared
/// order as PTY output arrives, fed by `scan(chunk)` from the pump. Step kinds:
///   - `answer_dialog` — wait for `needle`, inject `response`, advance (the
///     codex "Hooks need review" → `2\r` case; the only kind any shipped CLI
///     uses today).
///   - `wait_for` — block until `needle` appears, then advance (gate a later
///     step on a prompt/banner instead of a fixed sleep).
///   - `input` — inject `response` immediately when this step becomes active.
///   - `extract_session_id` — capture the token after `needle` into the named
///     `into` slot (resume support), then advance.
///
/// This is the **data-driven replacement** for the old hard-coded
/// `DialogAutoAnswer`: needle/response live in `cli-plugins/<id>.toml`, so a CLI
/// declares its own onboarding with zero Rust change. Safety constraints
/// preserved: each step is single-shot (cursor only moves forward), needle
/// steps are time-boxed (`window_ms`, default 30s — the plan advances past a
/// step whose needle never shows), needle matching is the literal substring
/// (never a short fragment), and the output buffer is bounded (sliding 8 KiB,
/// so a chatty agent can't OOM us). Host-side, so it works regardless of which
/// UI (if any) is attached. NOTE: advance is output-driven (only fires while
/// the pump delivers chunks); a step waiting on a needle that the CLL never
/// prints just times out and the plan moves on.
struct ReadyPlanRunner {
    steps: Vec<PlanStep>,
    /// Index of the currently-active step; `>= steps.len()` once the plan is done.
    cursor: usize,
    /// Deadline for the active needle step (`None` for `input`, which fires
    /// immediately, and once the plan is done). Set when a step becomes active.
    deadline: Option<Instant>,
    /// Sliding window over PTY output, shared across steps. Bounded by `MAX_BUFFER`.
    buf: Vec<u8>,
    /// Cloned PtyBridge input channel. `try_send` is non-blocking and used from
    /// the sync `scan` path; ample capacity for a few bytes, degrade silently.
    input_tx: mpsc::Sender<Bytes>,
    /// Agent_id for log lines only — never written into the PTY.
    agent_id: String,
    /// `extract_session_id` captures, keyed by the step's `into` name.
    captured: std::collections::HashMap<String, String>,
}

/// One resolved step (bytes pre-decoded from the manifest strings).
struct PlanStep {
    kind: crate::plugins::ReadyStepKind,
    needle: Vec<u8>,
    response: Vec<u8>,
    window: Duration,
    into: String,
}

impl ReadyPlanRunner {
    const MAX_BUFFER: usize = 8 * 1024;

    /// Build from a plugin's `ready_plan`. Returns `None` when there are no
    /// actionable steps, so the PTY pump skips scanning entirely (the common
    /// case — e.g. claude has no blocking dialog and ships an empty plan).
    /// Invalid steps (missing needle/response/into for their kind) are
    /// warn-skipped rather than aborting the whole plan.
    fn from_plan(
        plan: &[crate::plugins::ReadyStep],
        input_tx: mpsc::Sender<Bytes>,
        agent_id: &str,
    ) -> Option<Self> {
        use crate::plugins::ReadyStepKind;
        let mut steps = Vec::new();
        for step in plan {
            let needs_needle = matches!(
                step.kind,
                ReadyStepKind::AnswerDialog
                    | ReadyStepKind::WaitFor
                    | ReadyStepKind::ExtractSessionId
            );
            let needs_response =
                matches!(step.kind, ReadyStepKind::AnswerDialog | ReadyStepKind::Input);
            if needs_needle && step.needle.is_empty() {
                tracing::warn!(agent = %agent_id, kind = ?step.kind, "ready_plan: step missing needle; skipping");
                continue;
            }
            if needs_response && step.response.is_empty() {
                tracing::warn!(agent = %agent_id, kind = ?step.kind, "ready_plan: step missing response; skipping");
                continue;
            }
            if matches!(step.kind, ReadyStepKind::ExtractSessionId) && step.into.is_empty() {
                tracing::warn!(agent = %agent_id, "ready_plan: extract_session_id missing `into`; skipping");
                continue;
            }
            steps.push(PlanStep {
                kind: step.kind,
                needle: step.needle.clone().into_bytes(),
                response: step.response.clone().into_bytes(),
                window: Duration::from_millis(step.window_ms),
                into: step.into.clone(),
            });
        }
        if steps.is_empty() {
            return None;
        }
        let mut runner = Self {
            steps,
            cursor: 0,
            deadline: None,
            buf: Vec::with_capacity(2048),
            input_tx,
            agent_id: agent_id.to_string(),
            captured: std::collections::HashMap::new(),
        };
        runner.arm_deadline(Instant::now());
        Some(runner)
    }

    /// Captured `extract_session_id` values (slot name → token). A future
    /// resume path reads this; today it's surfaced for logging/tests.
    #[allow(dead_code)]
    fn captured(&self) -> &std::collections::HashMap<String, String> {
        &self.captured
    }

    /// Set `deadline` for the step now under `cursor`: a window for needle
    /// steps, `None` for `input` (fires immediately) and for "plan done".
    fn arm_deadline(&mut self, now: Instant) {
        use crate::plugins::ReadyStepKind;
        self.deadline = self.steps.get(self.cursor).and_then(|s| match s.kind {
            ReadyStepKind::Input => None,
            _ => Some(now + s.window),
        });
    }

    fn inject(&self, bytes: &[u8]) {
        // try_send is non-blocking; if the channel is full something is very
        // wrong but it's not worth blocking the PTY pump for.
        match self.input_tx.try_send(Bytes::copy_from_slice(bytes)) {
            Ok(()) => tracing::info!(agent = %self.agent_id, "ready_plan: injected step response"),
            Err(err) => tracing::warn!(
                agent = %self.agent_id, ?err,
                "ready_plan: inject try_send failed; user may need to act manually",
            ),
        }
    }

    fn scan(&mut self, chunk: &[u8]) {
        if self.cursor >= self.steps.len() {
            return; // plan complete
        }
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > Self::MAX_BUFFER {
            let keep_from = self.buf.len() - 1024;
            self.buf.drain(..keep_from);
        }
        self.advance();
        if self.cursor >= self.steps.len() {
            self.buf.clear();
        }
    }

    /// Progress the cursor as far as the current buffer allows. Stops at the
    /// first needle step whose needle hasn't appeared yet (and hasn't timed
    /// out). Each iteration clones the step's small byte fields so we can then
    /// mutate `self` (cursor / captured / inject) without an aliasing borrow.
    fn advance(&mut self) {
        use crate::plugins::ReadyStepKind;
        while self.cursor < self.steps.len() {
            let now = Instant::now();
            if let Some(dl) = self.deadline {
                if now > dl {
                    tracing::warn!(
                        agent = %self.agent_id, step = self.cursor,
                        "ready_plan: step timed out waiting for its needle; advancing",
                    );
                    self.cursor += 1;
                    self.arm_deadline(now);
                    continue;
                }
            }
            let (kind, needle, response, into) = {
                let s = &self.steps[self.cursor];
                (s.kind, s.needle.clone(), s.response.clone(), s.into.clone())
            };
            match kind {
                ReadyStepKind::Input => {
                    self.inject(&response);
                    self.cursor += 1;
                    self.arm_deadline(now);
                }
                ReadyStepKind::AnswerDialog => {
                    if find(&self.buf, &needle).is_some() {
                        self.inject(&response);
                        self.cursor += 1;
                        self.arm_deadline(now);
                    } else {
                        break;
                    }
                }
                ReadyStepKind::WaitFor => {
                    if find(&self.buf, &needle).is_some() {
                        self.cursor += 1;
                        self.arm_deadline(now);
                    } else {
                        break;
                    }
                }
                ReadyStepKind::ExtractSessionId => {
                    if let Some(pos) = find(&self.buf, &needle) {
                        let token = extract_token_after(&self.buf, pos + needle.len());
                        tracing::info!(
                            agent = %self.agent_id, into = %into, value = %token,
                            "ready_plan: captured session id",
                        );
                        self.captured.insert(into, token);
                        self.cursor += 1;
                        self.arm_deadline(now);
                    } else {
                        break;
                    }
                }
            }
        }
    }
}

/// Take the first whitespace-delimited token starting at byte `start` in `buf`
/// (skipping leading spaces/tabs), lossy-decoded. Used by `extract_session_id`
/// to grab the value printed right after a marker like `Session id:`.
fn extract_token_after(buf: &[u8], start: usize) -> String {
    let tail = buf.get(start..).unwrap_or(&[]);
    let trimmed: &[u8] = {
        let lead = tail.iter().take_while(|b| **b == b' ' || **b == b'\t').count();
        &tail[lead..]
    };
    let end = trimmed
        .iter()
        .position(|b| b.is_ascii_whitespace())
        .unwrap_or(trimmed.len());
    String::from_utf8_lossy(&trimmed[..end]).into_owned()
}

#[cfg(test)]
mod model_overlay_tests {
    use super::model_overlay_args;

    #[test]
    fn substitutes_placeholder() {
        let tmpl = vec!["--model".to_string(), "{model}".to_string()];
        assert_eq!(
            model_overlay_args("opus", &tmpl),
            vec!["--model".to_string(), "opus".to_string()]
        );
    }

    #[test]
    fn substitutes_inside_a_joined_arg() {
        // A CLI that takes `--model=<v>` as one token still works.
        let tmpl = vec!["--model={model}".to_string()];
        assert_eq!(model_overlay_args("sonnet", &tmpl), vec!["--model=sonnet"]);
    }

    #[test]
    fn empty_template_yields_no_args() {
        assert!(model_overlay_args("opus", &[]).is_empty());
    }
}

#[cfg(test)]
mod ready_plan_tests {
    use super::*;
    use crate::plugins::{ReadyStep, ReadyStepKind};

    fn step(kind: ReadyStepKind, needle: &str, response: &str, into: &str) -> ReadyStep {
        ReadyStep {
            kind,
            needle: needle.into(),
            response: response.into(),
            window_ms: 30_000,
            into: into.into(),
        }
    }
    fn answer(needle: &str, response: &str) -> ReadyStep {
        step(ReadyStepKind::AnswerDialog, needle, response, "")
    }
    fn codex_hooks_step() -> ReadyStep {
        answer("Hooks need review", "2\r")
    }

    fn make_pair() -> (ReadyPlanRunner, mpsc::Receiver<Bytes>) {
        let (tx, rx) = mpsc::channel::<Bytes>(8);
        let runner = ReadyPlanRunner::from_plan(&[codex_hooks_step()], tx, "codex-test")
            .expect("non-empty plan yields a runner");
        (runner, rx)
    }

    #[tokio::test]
    async fn empty_plan_is_none() {
        let (tx, _rx) = mpsc::channel::<Bytes>(8);
        assert!(
            ReadyPlanRunner::from_plan(&[], tx, "x").is_none(),
            "no steps ⇒ no runner (pump skips scanning)",
        );
    }

    #[tokio::test]
    async fn sends_response_on_match() {
        let (mut r, mut rx) = make_pair();
        r.scan(b"some noise before the dialog\n");
        r.scan(b"\nHooks need review\n1 hook is new\n");
        // Should have sent "2\r" exactly once, then advanced past the step.
        let got = rx.try_recv().expect("response should have been queued");
        assert_eq!(&got[..], b"2\r");
        assert!(rx.try_recv().is_err(), "no second response");
        assert_eq!(r.cursor, 1, "cursor advanced past the only step");
    }

    #[tokio::test]
    async fn single_shot_after_fired() {
        let (mut r, mut rx) = make_pair();
        r.scan(b"Hooks need review");
        let _ = rx.try_recv().expect("first response sent");
        // Cursor already moved past; a repeat of the dialog text is ignored.
        r.scan(b"Hooks need review again somehow");
        assert!(
            rx.try_recv().is_err(),
            "second match must NOT enqueue another response",
        );
    }

    #[tokio::test]
    async fn matches_across_chunk_boundary() {
        let (mut r, mut rx) = make_pair();
        // Split the needle across two chunks — the sliding buffer must
        // stitch them back together before matching.
        r.scan(b"Hooks ne");
        assert!(rx.try_recv().is_err(), "no premature match");
        r.scan(b"ed review");
        let got = rx.try_recv().expect("match after stitching chunks");
        assert_eq!(&got[..], b"2\r");
    }

    #[tokio::test]
    async fn ignores_unrelated_substrings() {
        let (mut r, mut rx) = make_pair();
        r.scan(b"Trust this folder? hook count: 0");
        r.scan(b"reviewing your code changes now");
        assert!(rx.try_recv().is_err(), "substring 'hook' / 'review' alone must NOT trigger");
        assert_eq!(r.cursor, 0, "still waiting on the real needle");
    }

    #[tokio::test]
    async fn does_not_fire_after_window_expires() {
        let (mut r, mut rx) = make_pair();
        // Synthesize an expired deadline so we don't actually sleep 30s.
        r.deadline = Some(Instant::now() - Duration::from_secs(1));
        r.scan(b"Hooks need review");
        assert!(rx.try_recv().is_err(), "expired window must not fire");
        assert_eq!(r.cursor, 1, "timed-out step is skipped, not fired");
    }

    #[tokio::test]
    async fn buffer_stays_bounded_under_chatty_input() {
        let (mut r, _rx) = make_pair();
        // Push many small chunks of unrelated bytes.
        for _ in 0..200 {
            r.scan(&[b'.'; 1024]);
        }
        assert!(
            r.buf.len() <= ReadyPlanRunner::MAX_BUFFER,
            "buffer must stay capped (got {})",
            r.buf.len(),
        );
    }

    #[tokio::test]
    async fn multiple_dialogs_each_fire_once() {
        let (tx, mut rx) = mpsc::channel::<Bytes>(8);
        let plan = vec![answer("Trust this folder", "1\r"), codex_hooks_step()];
        let mut r = ReadyPlanRunner::from_plan(&plan, tx, "multi").expect("runner");
        // Sequential: the dialogs are declared (and here arrive) in order.
        r.scan(b"? Trust this folder ? [y/N]");
        assert_eq!(&rx.try_recv().expect("first dialog answered")[..], b"1\r");
        r.scan(b"... later ... Hooks need review ...");
        assert_eq!(&rx.try_recv().expect("second dialog answered")[..], b"2\r");
        assert!(rx.try_recv().is_err(), "no extra responses");
        assert_eq!(r.cursor, 2, "both steps consumed");
    }

    #[tokio::test]
    async fn wait_for_then_input_is_sequential() {
        // wait_for gates the input: nothing is typed until the banner shows.
        let (tx, mut rx) = mpsc::channel::<Bytes>(8);
        let plan = vec![
            step(ReadyStepKind::WaitFor, "READY", "", ""),
            step(ReadyStepKind::Input, "", "go\r", ""),
        ];
        let mut r = ReadyPlanRunner::from_plan(&plan, tx, "seq").expect("runner");
        r.scan(b"booting...\n");
        assert!(rx.try_recv().is_err(), "input must NOT fire before the wait_for matches");
        assert_eq!(r.cursor, 0);
        r.scan(b"all systems READY now\n");
        // wait_for matched → cursor advances → input fires immediately.
        assert_eq!(&rx.try_recv().expect("input injected after wait")[..], b"go\r");
        assert_eq!(r.cursor, 2, "plan complete");
    }

    #[tokio::test]
    async fn input_first_fires_immediately() {
        let (tx, mut rx) = mpsc::channel::<Bytes>(8);
        let plan = vec![step(ReadyStepKind::Input, "", "hi\r", "")];
        let mut r = ReadyPlanRunner::from_plan(&plan, tx, "in").expect("runner");
        r.scan(b"any output at all");
        assert_eq!(&rx.try_recv().expect("input fired on first scan")[..], b"hi\r");
    }

    #[tokio::test]
    async fn extract_session_id_captures_token() {
        let (tx, _rx) = mpsc::channel::<Bytes>(8);
        let plan = vec![step(ReadyStepKind::ExtractSessionId, "Session id:", "", "sid")];
        let mut r = ReadyPlanRunner::from_plan(&plan, tx, "ex").expect("runner");
        r.scan(b"banner\nSession id:  abc-123-def \ntrailing output\n");
        assert_eq!(r.captured().get("sid").map(String::as_str), Some("abc-123-def"));
        assert_eq!(r.cursor, 1, "advanced after capture");
    }

    #[test]
    fn extract_token_after_skips_leading_space_and_stops_at_whitespace() {
        assert_eq!(extract_token_after(b"   tok-99\nrest", 0), "tok-99");
        assert_eq!(extract_token_after(b"x", 1), ""); // start at EOF → empty
    }
}

/// Probe `<binary> --help` once and cache whether `flag` appears anywhere
/// in stdout or stderr. Used to feature-detect CLI flags whose absence
/// would crash spawn (codex 0.130 rejects unknown argv with non-zero exit
/// — adding a future-only flag unconditionally would brick every spawn on
/// the older version).
///
/// L5c — substitute `{model}` in each manifest `model_args` entry. Pure +
/// unit-tested so the spawn argv path stays trivial. Caller decides whether a
/// model is in effect; this only renders the template.
fn model_overlay_args(model: &str, template: &[String]) -> Vec<String> {
    template.iter().map(|a| a.replace("{model}", model)).collect()
}

/// Substitute the concrete effort value into a CLI's `effort_args` template
/// (`{effort}` placeholder). Mirrors `model_overlay_args`.
fn effort_overlay_args(effort: &str, template: &[String]) -> Vec<String> {
    template.iter().map(|a| a.replace("{effort}", effort)).collect()
}

/// Cache key is `(binary, flag)` so different plugins probing different
/// flags don't collide. The cache is process-lifetime — a CLI upgrade
/// requires a server restart to re-probe, which is fine for the local
/// single-user model.
///
/// Errors and timeouts on the probe fall through as `false`: if we can't
/// confirm the flag is supported, we don't inject it.
///
/// The probe is **timeout-bounded** (F17): `<binary> --help` runs on a worker
/// thread and we wait at most `PROBE_TIMEOUT` for it via `recv_timeout`. This
/// fn is called synchronously on the async spawn path, so an unresponsive
/// `--help` must not be able to stall a spawn forever — past the deadline we
/// give up and return `false`, making the doc comment above actually true.
/// (A genuinely hung `--help` leaves its child + thread lingering until it
/// exits on its own or the server does; a real CLI's `--help` returns in ms,
/// so this is an acceptable bound on a pathological case.)
fn binary_supports_flag(binary: &str, flag: &str) -> bool {
    const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    static CACHE: OnceLock<Mutex<HashMap<(String, String), bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let key = (binary.to_string(), flag.to_string());
    if let Some(&v) = cache.lock().get(&key) {
        return v;
    }

    let bin = binary.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // output() drains stdout+stderr (so the child can't deadlock on a full
        // pipe) and waits for exit. Result is sent back; ignore send errors
        // (receiver already gave up on timeout).
        let _ = tx.send(std::process::Command::new(&bin).arg("--help").output());
    });

    let supported = match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(Ok(o)) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            stdout.contains(flag) || stderr.contains(flag)
        }
        Ok(Err(_)) => false, // spawn / IO error
        Err(_) => {
            // recv timed out — the probe took longer than PROBE_TIMEOUT.
            tracing::warn!(binary, flag, "binary flag probe timed out; assuming unsupported");
            false
        }
    };

    tracing::info!(
        binary, flag, supported,
        "binary flag probe result"
    );
    cache.lock().insert(key, supported);
    supported
}

fn locate_sibling_bin(name: &str, env_override: &str) -> Result<PathBuf> {
    if let Ok(p) = std::env::var(env_override) {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe().context("current_exe")?;
    if let Some(dir) = exe.parent() {
        let cand = dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        });
        if cand.is_file() {
            return Ok(cand);
        }
    }
    anyhow::bail!(
        "{name} not found next to flockmux-server. Build it with \
         `cargo build -p {name}` or set {env_override}"
    )
}
