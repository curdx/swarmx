//! Tail a worker's CLI session transcript (JSONL) and emit tool-level
//! [`SwarmEvent::AgentActivity`] — WITHOUT touching the worker.
//!
//! Both claude and codex write a structured JSONL log of every turn (used for
//! their own `resume`). We read that file; the worker is never configured,
//! hooked, or slowed. Compare the alternatives:
//!   - hooks run SYNCHRONOUSLY inside the worker's tool loop (~50ms tax/call),
//!   - parsing the PTY means decoding a human-facing ANSI screen (brittle).
//! Reading the file the CLI already writes costs the worker nothing and also
//! carries token usage (for future cost stats).
//!
//! The format has NO official spec, so parsing is deliberately LENIENT: a bad
//! line is skipped, an unknown shape yields no activity, never a panic. The
//! fixture tests at the bottom lock the exact fields we depend on — if a CLI
//! upgrade changes them, CI turns red instead of the feature silently emitting
//! nothing.

use flockmux_protocol::rest::AgentActivityRecord;
use flockmux_protocol::ws_swarm::{AgentState, SwarmEvent};
use flockmux_swarm::Swarm;
use serde_json::Value;
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// How often to re-read the file's tail. Tool calls are seconds apart in an
/// interactive worker, so sub-second latency isn't needed; polling avoids a
/// notify dependency + its debounce/rotation edge cases.
const POLL_INTERVAL: Duration = Duration::from_millis(700);
/// Give the CLI this long to create its session file before giving up.
const LOCATE_TIMEOUT: Duration = Duration::from_secs(90);
const LOCATE_POLL: Duration = Duration::from_millis(500);
/// Cap a single tail read so one giant tool result can't balloon memory; we
/// only need the tool name + a short label, not the whole payload.
const MAX_READ: usize = 4 * 1024 * 1024;
/// Drop a single line longer than this (a huge embedded result) — it carries
/// no extra signal for our one-line labels and would bloat the buffer.
const MAX_LINE: usize = 512 * 1024;

#[derive(Clone, Copy)]
enum Flavor {
    Claude,
    Codex,
}

/// Spawn a background task that tails `agent_id`'s session transcript and emits
/// `AgentActivity` for each tool call/result. `cli` is the plugin id
/// (e.g. "claude" / "codex"); `cwd` is the worker's canonical workspace dir.
/// No-op for an unknown CLI (no known transcript format).
pub fn spawn_tailer(
    swarm: Arc<Swarm>,
    store: Arc<flockmux_storage::Store>,
    agent_id: String,
    cli: String,
    cwd: PathBuf,
    session_id: Option<String>,
) {
    let flavor = if cli.contains("codex") {
        Flavor::Codex
    } else if cli.contains("claude") {
        Flavor::Claude
    } else {
        tracing::debug!(agent = %agent_id, cli = %cli, "transcript: unknown CLI flavor, not tailing");
        return;
    };
    tokio::spawn(async move {
        run(swarm, store, &agent_id, flavor, &cwd, session_id.as_deref()).await;
    });
}

async fn run(
    swarm: Arc<Swarm>,
    store: Arc<flockmux_storage::Store>,
    agent_id: &str,
    flavor: Flavor,
    cwd: &Path,
    session_id: Option<&str>,
) {
    let path = match locate(flavor, agent_id, cwd, session_id).await {
        Some(p) => p,
        None => {
            tracing::debug!(agent = %agent_id, "transcript: session file never appeared; giving up");
            return;
        }
    };
    tracing::info!(agent = %agent_id, path = %path.display(), "transcript: tailing");

    let mut rx = swarm.subscribe();
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut st = TailState::new(flavor);

    // High-water mark we've already persisted, so an idle tick (no new events)
    // doesn't fire a redundant UPDATE.
    let mut persisted: Option<i64> = None;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                st.poll(&path, &swarm, agent_id).await;
                persist_activity(&store, agent_id, st.last_emit_at, &mut persisted).await;
            }
            ev = rx.recv() => {
                match ev {
                    Ok(SwarmEvent::AgentState { agent_id: a, state })
                        if a == agent_id && matches!(state, AgentState::Exited | AgentState::Error) =>
                    {
                        st.poll(&path, &swarm, agent_id).await; // final flush
                        persist_activity(&store, agent_id, st.last_emit_at, &mut persisted).await;
                        break;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    tracing::debug!(agent = %agent_id, "transcript: tailer stopped");
}

/// Persist `last_emit_at` to the agent row if it advanced past what we've
/// already written. Mutating `persisted` through `&mut` keeps the high-water
/// mark across poll ticks without a dead-store warning at the break path.
async fn persist_activity(
    store: &flockmux_storage::Store,
    agent_id: &str,
    last_emit_at: Option<i64>,
    persisted: &mut Option<i64>,
) {
    if last_emit_at <= *persisted {
        return;
    }
    if let Some(at) = last_emit_at {
        if let Err(e) = store.touch_agent_activity(agent_id.to_string(), at).await {
            tracing::debug!(agent = %agent_id, ?e, "transcript: touch_agent_activity failed");
        }
        *persisted = last_emit_at;
    }
}

// ── file location ──────────────────────────────────────────────────────────

async fn locate(
    flavor: Flavor,
    agent_id: &str,
    cwd: &Path,
    session_id: Option<&str>,
) -> Option<PathBuf> {
    let deadline = tokio::time::Instant::now() + LOCATE_TIMEOUT;
    loop {
        let found = match flavor {
            Flavor::Claude => claude_file(cwd, session_id),
            Flavor::Codex => codex_file(agent_id),
        };
        if found.is_some() {
            return found;
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(LOCATE_POLL).await;
    }
}

/// `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`, newest file.
fn claude_file(cwd: &Path, session_id: Option<&str>) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = Path::new(&home)
        .join(".claude")
        .join("projects")
        .join(encode_cwd(cwd));
    match session_id {
        // The exact file claude was told to write via `--session-id <uuid>`.
        // Avoids locking onto a STALE prior session's .jsonl that is still the
        // newest in this project dir before our claude has created its own —
        // the bug that surfaced re-spawning an orchestrator in one workspace.
        Some(sid) => {
            let p = dir.join(format!("{sid}.jsonl"));
            p.is_file().then_some(p)
        }
        // No forced id (defensive — claude always gets one now). Fall back to
        // the newest .jsonl in the dir.
        None => newest(&dir, false, &|p| {
            p.extension().and_then(|e| e.to_str()) == Some("jsonl")
        }),
    }
}

/// claude encodes the cwd into a directory name by replacing `/`, `.`, `_`, `\`
/// with `-` (case preserved). Lossy + collision-prone, so this is ONLY used
/// forward (known cwd → dir name), never reversed. Mirrors cc-trace's
/// `encode_path`.
fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| match c {
            '/' | '.' | '_' | '\\' => '-',
            other => other,
        })
        .collect()
}

/// `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`, newest. flockmux gives
/// each codex worker an isolated per-agent CODEX_HOME, so that tree holds only
/// this worker's session(s). New codex compresses cold files to `.jsonl.zst`;
/// the ACTIVE file stays plain `.jsonl`, which is what we tail.
fn codex_file(agent_id: &str) -> Option<PathBuf> {
    let home = crate::pre_spawn::codex_per_agent_home_path(agent_id)?;
    let sessions = home.join("sessions");
    newest(&sessions, true, &|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
    })
}

/// Newest (by mtime) file under `dir` matching `pred`. `recurse` walks
/// subdirectories (codex's date tree); claude is flat.
fn newest(dir: &Path, recurse: bool, pred: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            let ft = match e.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if recurse {
                    stack.push(p);
                }
                continue;
            }
            if !pred(&p) {
                continue;
            }
            if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                if best.as_ref().is_none_or(|(bt, _)| m > *bt) {
                    best = Some((m, p));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

// ── tail + parse ─────────────────────────────────────────────────────────────

struct Pending {
    seq: u32,
    start_ms: i64,
    label: String,
}

struct TailState {
    flavor: Flavor,
    offset: u64,
    partial: Vec<u8>,
    pending: HashMap<String, Pending>,
    seq: u32,
    /// Unix-ms of the most recent event emitted this tailer's lifetime. The run
    /// loop diffs this against what it has persisted to decide whether to call
    /// `touch_agent_activity` after a poll (avoids a redundant UPDATE on idle
    /// ticks that produced no new events).
    last_emit_at: Option<i64>,
}

impl TailState {
    fn new(flavor: Flavor) -> Self {
        Self {
            flavor,
            offset: 0,
            partial: Vec::new(),
            pending: HashMap::new(),
            seq: 0,
            last_emit_at: None,
        }
    }

    async fn poll(&mut self, path: &Path, swarm: &Swarm, agent_id: &str) {
        let mut f = match tokio::fs::File::open(path).await {
            Ok(f) => f,
            Err(_) => return,
        };
        let len = match f.metadata().await {
            Ok(m) => m.len(),
            Err(_) => return,
        };
        if len < self.offset {
            // File truncated / rotated — start over.
            self.offset = 0;
            self.partial.clear();
        }
        if len <= self.offset {
            return;
        }
        if f.seek(SeekFrom::Start(self.offset)).await.is_err() {
            return;
        }
        let want = ((len - self.offset) as usize).min(MAX_READ);
        let mut buf = vec![0u8; want];
        let n = match f.read(&mut buf).await {
            Ok(n) => n,
            Err(_) => return,
        };
        buf.truncate(n);
        self.offset += n as u64;
        self.partial.extend_from_slice(&buf);

        // Split out complete (newline-terminated) lines; keep the trailing
        // partial line for the next poll (it's still being written).
        while let Some(nl) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=nl).collect();
            if line.len() > MAX_LINE {
                continue;
            }
            let line = &line[..line.len() - 1];
            let s = match std::str::from_utf8(line) {
                Ok(s) => s.trim(),
                Err(_) => continue,
            };
            if s.is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(s) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let tools = match self.flavor {
                Flavor::Claude => parse_claude(&v),
                Flavor::Codex => parse_codex(&v),
            };
            for t in tools {
                self.emit(t, swarm, agent_id);
            }
        }
        // A single absurdly long line with no newline would grow unbounded —
        // drop it.
        if self.partial.len() > MAX_LINE {
            self.partial.clear();
        }
    }

    fn emit(&mut self, t: ParsedTool, swarm: &Swarm, agent_id: &str) {
        let at = now_ms();
        // Every tool start/end is "the agent did something at `at`" — record the
        // high-water mark so the run loop can persist it (F3 stuck-detection).
        self.last_emit_at = Some(at);
        match t {
            ParsedTool::Start { tool_id, label } => {
                self.seq = self.seq.wrapping_add(1);
                let seq = self.seq;
                self.pending.insert(
                    tool_id,
                    Pending {
                        seq,
                        start_ms: at,
                        label: label.clone(),
                    },
                );
                emit_activity(swarm, agent_id, "running", label, seq, None, at);
            }
            ParsedTool::End { tool_id, ok } => {
                // A result for a tool we never saw start (we attached
                // mid-session) is ignored — no running event to pair with.
                if let Some(p) = self.pending.remove(&tool_id) {
                    let dur = (at - p.start_ms).max(0) as u32;
                    let phase = if ok { "ok" } else { "error" };
                    emit_activity(swarm, agent_id, phase, p.label, p.seq, Some(dur), at);
                }
            }
        }
    }
}

/// Fan one tool-level activity out to BOTH sinks: the in-memory ring
/// (`Swarm::record_activity`, served by `GET /api/agent/:id/activity` for cold
/// backfill) and the live WS broadcast (`publish_event`). Centralised so a row
/// can never reach one sink but not the other — they share the same `seq`, so
/// the UI merges backfill + live by it.
fn emit_activity(
    swarm: &Swarm,
    agent_id: &str,
    phase: &str,
    label: String,
    seq: u32,
    duration_ms: Option<u32>,
    at: i64,
) {
    swarm.record_activity(
        agent_id,
        AgentActivityRecord {
            agent_id: agent_id.to_string(),
            kind: "tool".into(),
            label: label.clone(),
            phase: phase.to_string(),
            seq,
            duration_ms,
            at,
        },
    );
    swarm.publish_event(SwarmEvent::AgentActivity {
        agent_id: agent_id.to_string(),
        kind: "tool".into(),
        label,
        phase: phase.to_string(),
        seq,
        duration_ms,
        at,
    });
}

enum ParsedTool {
    Start { tool_id: String, label: String },
    End { tool_id: String, ok: bool },
}

/// claude: tool_use lives in an `assistant` row's `message.content[]`; the
/// matching tool_result lives in a later `user` row. The content-block `type`
/// is authoritative, so we don't gate on the row `type`.
fn parse_claude(v: &Value) -> Vec<ParsedTool> {
    let mut out = Vec::new();
    let content = match v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        Some(c) => c,
        None => return out,
    };
    for block in content {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("tool_use") => {
                if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                    let label = summarize(name, block.get("input").unwrap_or(&Value::Null));
                    out.push(ParsedTool::Start {
                        tool_id: id.to_string(),
                        label,
                    });
                }
            }
            Some("tool_result") => {
                if let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str()) {
                    let is_err = block
                        .get("is_error")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    out.push(ParsedTool::End {
                        tool_id: id.to_string(),
                        ok: !is_err,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// codex: a `response_item` row whose `payload.type` is a function/tool call or
/// its output. `arguments` is a JSON STRING (needs a second parse). codex
/// output carries no reliable error flag, so results are reported as `ok`.
fn parse_codex(v: &Value) -> Vec<ParsedTool> {
    let mut out = Vec::new();
    if v.get("type").and_then(|t| t.as_str()) != Some("response_item") {
        return out;
    }
    let payload = match v.get("payload") {
        Some(p) => p,
        None => return out,
    };
    match payload.get("type").and_then(|t| t.as_str()) {
        Some("function_call" | "custom_tool_call" | "local_shell_call") => {
            if let Some(id) = payload.get("call_id").and_then(|i| i.as_str()) {
                let name = payload
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool");
                let args = payload
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or(Value::Null);
                out.push(ParsedTool::Start {
                    tool_id: id.to_string(),
                    label: summarize(name, &args),
                });
            }
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            if let Some(id) = payload.get("call_id").and_then(|i| i.as_str()) {
                out.push(ParsedTool::End {
                    tool_id: id.to_string(),
                    ok: true,
                });
            }
        }
        _ => {}
    }
    out
}

/// MCP tools arrive as `mcp__<server>__<action>`; show just `<action>` so a
/// member row / activity line isn't dominated by the `mcp__flockmux-swarm__`
/// prefix. Non-MCP names (Bash, Edit, …) pass through unchanged.
fn prettify_tool_name(name: &str) -> &str {
    match name.strip_prefix("mcp__") {
        Some(rest) => rest.rsplit("__").next().unwrap_or(rest),
        None => name,
    }
}

/// One-line human label: tool name + a salient string argument (a path,
/// command, pattern, …), whitespace-collapsed and truncated. Falls back to
/// just the name when no salient arg is present.
fn summarize(name: &str, input: &Value) -> String {
    let name = prettify_tool_name(name);
    const KEYS: &[&str] = &[
        "file_path",
        "path",
        "command",
        "cmd",
        "pattern",
        "query",
        "url",
        "notebook_path",
        "prompt",
    ];
    let detail = input.as_object().and_then(|obj| {
        KEYS.iter().find_map(|k| match obj.get(*k) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
    });
    match detail {
        Some(s) => format!("{} {}", name, shorten(&collapse_ws(&s))),
        None => name.to_string(),
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn shorten(s: &str) -> String {
    const MAX: usize = 48;
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= MAX {
        s.to_string()
    } else {
        let head: String = chars[..MAX].iter().collect();
        format!("{head}…")
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(v: &[ParsedTool]) -> &ParsedTool {
        assert_eq!(v.len(), 1, "expected exactly one parsed tool");
        &v[0]
    }

    // ── claude fixtures (real-shape lines; lock the fields we read) ──────────

    #[test]
    fn claude_tool_use_makes_a_start_with_label() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Edit","input":{"file_path":"/Users/x/src/foo.rs"}}]}}"#;
        let v: Value = serde_json::from_str(line).unwrap();
        match one(&parse_claude(&v)) {
            ParsedTool::Start { tool_id, label } => {
                assert_eq!(tool_id, "toolu_1");
                assert!(label.starts_with("Edit "), "label was {label:?}");
                assert!(label.contains("foo.rs"), "label was {label:?}");
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn claude_tool_result_ok_and_error() {
        let ok_line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"done"}],"is_error":false}]}}"#;
        let v: Value = serde_json::from_str(ok_line).unwrap();
        match one(&parse_claude(&v)) {
            ParsedTool::End { tool_id, ok } => {
                assert_eq!(tool_id, "toolu_1");
                assert!(ok);
            }
            _ => panic!("expected End"),
        }
        let err_line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"boom","is_error":true}]}}"#;
        let v: Value = serde_json::from_str(err_line).unwrap();
        match one(&parse_claude(&v)) {
            ParsedTool::End { ok, .. } => assert!(!ok),
            _ => panic!("expected End"),
        }
    }

    #[test]
    fn claude_non_tool_rows_yield_nothing() {
        for line in [
            r#"{"type":"system","content":"hi"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"thinking out loud"}]}}"#,
            r#"{"type":"summary"}"#,
        ] {
            let v: Value = serde_json::from_str(line).unwrap();
            assert!(parse_claude(&v).is_empty(), "line should yield nothing: {line}");
        }
    }

    // ── codex fixtures ───────────────────────────────────────────────────────

    #[test]
    fn codex_function_call_and_output() {
        let call = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"git status\"}","call_id":"call_1"}}"#;
        let v: Value = serde_json::from_str(call).unwrap();
        match one(&parse_codex(&v)) {
            ParsedTool::Start { tool_id, label } => {
                assert_eq!(tool_id, "call_1");
                assert!(label.starts_with("exec_command "), "label was {label:?}");
                assert!(label.contains("git status"), "label was {label:?}");
            }
            _ => panic!("expected Start"),
        }
        let out = r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"on branch main"}}"#;
        let v: Value = serde_json::from_str(out).unwrap();
        match one(&parse_codex(&v)) {
            ParsedTool::End { tool_id, ok } => {
                assert_eq!(tool_id, "call_1");
                assert!(ok);
            }
            _ => panic!("expected End"),
        }
    }

    #[test]
    fn codex_non_tool_rows_yield_nothing() {
        for line in [
            r#"{"type":"session_meta","payload":{"id":"x"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{}}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant"}}"#,
        ] {
            let v: Value = serde_json::from_str(line).unwrap();
            assert!(parse_codex(&v).is_empty(), "line should yield nothing: {line}");
        }
    }

    // ── lenience + encoding ──────────────────────────────────────────────────

    #[test]
    fn garbage_never_panics() {
        for v in [
            serde_json::json!({}),
            serde_json::json!({"type": 42}),
            serde_json::json!({"message": {"content": "not-an-array"}}),
            serde_json::json!({"type": "response_item"}), // codex: no payload
        ] {
            let _ = parse_claude(&v);
            let _ = parse_codex(&v);
        }
    }

    #[test]
    fn prettify_mcp_tool_name_strips_prefix() {
        assert_eq!(
            prettify_tool_name("mcp__flockmux-swarm__swarm_list_messages"),
            "swarm_list_messages"
        );
        assert_eq!(prettify_tool_name("Bash"), "Bash");
        assert_eq!(prettify_tool_name("Edit"), "Edit");
    }

    #[test]
    fn encode_cwd_matches_claude_rule() {
        assert_eq!(
            encode_cwd(Path::new("/Users/wdx/opc/flockmux-core")),
            "-Users-wdx-opc-flockmux-core"
        );
        // `/.flockmux` → `--flockmux` (both `/` and `.` map to `-`).
        assert_eq!(
            encode_cwd(Path::new("/Users/wdx/.flockmux/workspaces/claude-106ea14e")),
            "-Users-wdx--flockmux-workspaces-claude-106ea14e"
        );
    }

    #[test]
    fn summarize_truncates_and_collapses() {
        let long = "a ".repeat(100);
        let v = serde_json::json!({ "command": long });
        let label = summarize("Bash", &v);
        assert!(label.starts_with("Bash "));
        assert!(label.chars().count() <= "Bash ".len() + 49); // 48 + ellipsis
        // no embedded newlines/double spaces
        let v = serde_json::json!({ "command": "git\n  status" });
        assert_eq!(summarize("Bash", &v), "Bash git status");
    }
}
