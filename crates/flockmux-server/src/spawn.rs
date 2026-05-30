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
pub fn spawn_agent(
    plugin: &CliPlugin,
    role: Option<String>,
    model: Option<String>,
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

    // Env: pass through HOME so the CLI finds its OAuth credentials
    // (~/.claude or ~/.codex). Drop everything else from the parent
    // process — the CLI shouldn't inherit ad-hoc shell vars.
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
    // Identity env passed to the CLI. codex picks `FLOCKMUX_AGENT_ID` /
    // `FLOCKMUX_SERVER_URL` up via the `env_vars` whitelist in
    // ~/.codex/config.toml and forwards them to the MCP subprocess. claude
    // also forwards them by spec (any vars present in the spawn env that
    // match the MCP entry's `env` block) — and the local-scope MCP entry
    // we write already lists them explicitly, so this is belt + braces.
    env.insert("FLOCKMUX_AGENT_ID".into(), agent_id.clone());
    env.insert("FLOCKMUX_SERVER_URL".into(), server_url.to_string());

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

    Ok(AgentSpawn { agent_id, slot })
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
/// `ready_plan` ([`crate::plugins::ReadyStep`]). Today it handles
/// `answer_dialog` steps: watch the PTY output stream for a `needle` and
/// inject a `response` once, so a first-spawn TUI dialog that gates input
/// (e.g. codex 0.130+'s "Hooks need review" menu — the user can't reach the
/// codex TUI to pick "2 Trust all and continue") doesn't hang a headless PTY.
///
/// This is the **data-driven replacement** for the old hard-coded
/// `DialogAutoAnswer`: the needle/response now live in `cli-plugins/<id>.toml`,
/// so any CLI declares its own dialogs with zero Rust change, and a CLI may
/// list multiple. Each step keeps the original safety constraints — single-shot
/// (`fired`), time-boxed (`window_ms`, default 30s), specific-needle (we match
/// the literal heading, never a short substring like "hook"), and
/// buffer-bounded (a sliding 8 KiB window so a chatty agent can't OOM us).
/// Implemented host-side so it works regardless of which UI (if any) is
/// attached (`flockmux-cli` headless / agent-to-agent included).
struct ReadyPlanRunner {
    answers: Vec<AnswerDialogState>,
    /// Sliding window over PTY output, shared across steps. Bounded by `MAX_BUFFER`.
    buf: Vec<u8>,
    /// Cloned PtyBridge input channel. `try_send` is non-blocking and used from
    /// the sync `scan` path; ample capacity for a few bytes, degrade silently.
    input_tx: mpsc::Sender<Bytes>,
    /// Agent_id for log lines only — never written into the PTY.
    agent_id: String,
}

/// Per-`answer_dialog`-step state.
struct AnswerDialogState {
    needle: Vec<u8>,
    response: Vec<u8>,
    /// Stop watching this step at this instant even if the needle never showed.
    deadline: Instant,
    /// One-shot guard: set once we've injected the response.
    fired: bool,
}

impl ReadyPlanRunner {
    const MAX_BUFFER: usize = 8 * 1024;

    /// Build from a plugin's `ready_plan`. Returns `None` when there are no
    /// actionable steps, so the PTY pump skips scanning entirely (the common
    /// case — e.g. claude has no blocking dialog and ships an empty plan).
    fn from_plan(
        plan: &[crate::plugins::ReadyStep],
        input_tx: mpsc::Sender<Bytes>,
        agent_id: &str,
    ) -> Option<Self> {
        use crate::plugins::ReadyStepKind;
        let now = Instant::now();
        let mut answers = Vec::new();
        for step in plan {
            match step.kind {
                ReadyStepKind::AnswerDialog => {
                    if step.needle.is_empty() || step.response.is_empty() {
                        tracing::warn!(
                            agent = %agent_id,
                            "ready_plan: answer_dialog step missing needle/response; skipping",
                        );
                        continue;
                    }
                    answers.push(AnswerDialogState {
                        needle: step.needle.clone().into_bytes(),
                        response: step.response.clone().into_bytes(),
                        deadline: now + Duration::from_millis(step.window_ms),
                        fired: false,
                    });
                }
            }
        }
        if answers.is_empty() {
            None
        } else {
            Some(Self {
                answers,
                buf: Vec::with_capacity(2048),
                input_tx,
                agent_id: agent_id.to_string(),
            })
        }
    }

    fn scan(&mut self, chunk: &[u8]) {
        let now = Instant::now();
        // Nothing left to do once every step has fired or timed out.
        if self.answers.iter().all(|a| a.fired || now > a.deadline) {
            return;
        }
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > Self::MAX_BUFFER {
            let keep_from = self.buf.len() - 1024;
            self.buf.drain(..keep_from);
        }
        for a in &mut self.answers {
            if a.fired || now > a.deadline {
                continue;
            }
            if find(&self.buf, &a.needle).is_some() {
                a.fired = true;
                // try_send is non-blocking; if the channel is full something
                // is very wrong but it's not worth blocking the PTY pump for.
                match self.input_tx.try_send(Bytes::copy_from_slice(&a.response)) {
                    Ok(()) => tracing::info!(
                        agent = %self.agent_id,
                        needle = %String::from_utf8_lossy(&a.needle),
                        "ready_plan: auto-answered dialog",
                    ),
                    Err(err) => tracing::warn!(
                        agent = %self.agent_id,
                        ?err,
                        "ready_plan: answer try_send failed; user may need to dismiss the dialog manually",
                    ),
                }
            }
        }
        // Free the buffer once all steps are done.
        if self.answers.iter().all(|a| a.fired) {
            self.buf.clear();
        }
    }
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

    fn codex_hooks_step() -> ReadyStep {
        ReadyStep {
            kind: ReadyStepKind::AnswerDialog,
            needle: "Hooks need review".into(),
            response: "2\r".into(),
            window_ms: 30_000,
        }
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
        // Should have sent "2\r" exactly once.
        let got = rx.try_recv().expect("response should have been queued");
        assert_eq!(&got[..], b"2\r");
        assert!(rx.try_recv().is_err(), "no second response");
        assert!(r.answers[0].fired, "fired flag should be set");
    }

    #[tokio::test]
    async fn single_shot_after_fired() {
        let (mut r, mut rx) = make_pair();
        r.scan(b"Hooks need review");
        let _ = rx.try_recv().expect("first response sent");
        // Even if the dialog text repeats, never fire again.
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
        assert!(!r.answers[0].fired);
    }

    #[tokio::test]
    async fn does_not_fire_after_window_expires() {
        let (mut r, mut rx) = make_pair();
        // Synthesize an expired deadline so we don't actually sleep 30s.
        r.answers[0].deadline = Instant::now() - Duration::from_secs(1);
        r.scan(b"Hooks need review");
        assert!(rx.try_recv().is_err(), "expired window must not fire");
        assert!(!r.answers[0].fired);
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
        let plan = vec![
            ReadyStep {
                kind: ReadyStepKind::AnswerDialog,
                needle: "Trust this folder".into(),
                response: "1\r".into(),
                window_ms: 30_000,
            },
            codex_hooks_step(),
        ];
        let mut r = ReadyPlanRunner::from_plan(&plan, tx, "multi").expect("runner");
        // Needle match is case-sensitive (same as the old DialogAutoAnswer),
        // so the chunk must carry the literal heading "Trust this folder".
        r.scan(b"? Trust this folder ? [y/N]");
        assert_eq!(&rx.try_recv().expect("first dialog answered")[..], b"1\r");
        r.scan(b"... later ... Hooks need review ...");
        assert_eq!(&rx.try_recv().expect("second dialog answered")[..], b"2\r");
        assert!(rx.try_recv().is_err(), "no extra responses");
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

/// Cache key is `(binary, flag)` so different plugins probing different
/// flags don't collide. The cache is process-lifetime — a CLI upgrade
/// requires a server restart to re-probe, which is fine for the local
/// single-user model.
///
/// Errors and timeouts on the probe fall through as `false`: if we can't
/// confirm the flag is supported, we don't inject it.
fn binary_supports_flag(binary: &str, flag: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let key = (binary.to_string(), flag.to_string());
    if let Some(&v) = cache.lock().get(&key) {
        return v;
    }

    let supported = std::process::Command::new(binary)
        .arg("--help")
        .output()
        .ok()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            stdout.contains(flag) || stderr.contains(flag)
        })
        .unwrap_or(false);

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
