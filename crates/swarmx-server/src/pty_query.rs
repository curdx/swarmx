//! One-shot PTY text query — drive a REAL interactive `claude` over a PTY to get
//! a single model answer **billed to the user's interactive subscription**, not
//! the `claude -p` Agent-SDK credit pool.
//!
//! ## Why this exists
//!
//! Anthropic's print/headless mode (`claude -p`) bills as API / Agent-SDK usage
//! even under an OAuth subscription with no `ANTHROPIC_API_KEY` set — confirmed
//! by claude-code issues #43333 and #37686, and codified on 2026-06-15 when
//! non-interactive subscription usage (Agent SDK, `claude -p`, third-party apps)
//! was carved into a separate, limited monthly Agent-SDK credit. The whole
//! swarmx thesis is the opposite: run the UNMODIFIED CLI interactively over a
//! PTY so it reuses the user's `~/.claude/` OAuth and bills to their *interactive*
//! plan, exactly like typing `claude` in a terminal. So features that used to
//! shell out to `claude -p` (the chat composer's 「优化」 button, blackboard
//! compaction) move here: spawn a throwaway interactive claude, drive ONE turn,
//! read the answer, kill it.
//!
//! ## How the answer is captured — transcript, not screen-scrape
//!
//! The PTY is used only to *type the prompt and trigger the turn*. The answer is
//! read from claude's **session transcript JSONL**, NOT by scraping the TUI
//! screen. Scraping the screen is a dead end for verbatim text: claude's TUI
//! renders inter-word spaces as cursor-forward escapes (so naive ANSI-stripping
//! drops them) and redraws streaming lines (so a linear capture duplicates
//! them) — both reproduced live before this approach. The transcript carries the
//! assistant message as exact JSON strings: no ANSI, no reflow, no redraw. We
//! force `--session-id` at spawn (already done for usage tailing), so the file
//! path is deterministic; we poll it for the assistant turn's `text` blocks.
//! The PTY output is still read, but only to detect a not-logged-in banner
//! (→ `Auth`) and to tell whether a paste landed (for the re-deliver retry).

use crate::plugins::CliPlugin;
use crate::registry::{AgentSlot, LifecycleEvent};
use crate::spawn::{spawn_agent, WorkspaceLayout};
use std::path::Path;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// After ShimReady, give claude's TUI this long to paint its input box and load
/// the MCP server before the FIRST paste. We can't use the `mcp_ready` watch the
/// bootstrap path uses — that ping (`POST /api/agent/:id/mcp-ready`) is dropped
/// for an agent that isn't in the registry, and this throwaway deliberately
/// isn't registered — so this is a fixed settle, backstopped by the re-deliver
/// loop below (which makes the exact value non-critical).
const READY_SETTLE: Duration = Duration::from_millis(4000);

/// How often to re-scan the transcript / PTY for the answer.
const POLL: Duration = Duration::from_millis(250);

/// If a delivered prompt produces NO new PTY output for this long, the paste
/// almost certainly raced a not-yet-ready TUI and was dropped — re-deliver it.
/// (Mirrors `deliver_bootstrap`'s "re-submit until the turn actually starts" for
/// opencode.) Only re-delivers when the buffer hasn't grown, so a claude that's
/// already mid-turn is never double-submitted.
const REDELIVER_AFTER: Duration = Duration::from_millis(7000);
const MAX_DELIVERIES: u32 = 4;

/// Once the assistant turn appears in the transcript, re-read after this long and
/// require it unchanged before returning — cheap insurance against reading a
/// half-written multi-block turn mid-stream.
const SETTLE_CONFIRM: Duration = Duration::from_millis(500);

/// Why the one-shot couldn't produce an answer. The caller maps each to an
/// honest HTTP status; `Empty`/`Timeout` are recoverable (fall back to the
/// user's original text), `Auth` is actionable (log in).
#[derive(Debug)]
pub enum OneShotError {
    /// claude isn't a keystroke/PTY engine here, has no live PTY input, or the
    /// transcript path couldn't be resolved (no forced session id).
    NoPty,
    /// Spawning the throwaway claude failed.
    Spawn(String),
    /// Came up but a not-logged-in / unauthorized banner showed when we asked.
    Auth,
    /// Process exited early / never came up before the deadline.
    NotReady,
    /// Came up but produced no (non-empty) assistant turn before the deadline.
    Timeout,
}

impl std::fmt::Display for OneShotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OneShotError::NoPty => write!(f, "claude 没有可用的 PTY 输入通道"),
            OneShotError::Spawn(e) => write!(f, "启动 claude 失败：{e}"),
            OneShotError::Auth => write!(f, "claude 未登录 / 未授权"),
            OneShotError::NotReady => write!(f, "claude 启动后在超时内没就绪"),
            OneShotError::Timeout => write!(f, "claude 在超时内没有产出结果"),
        }
    }
}

/// Run ONE turn against a throwaway interactive claude and return the model's
/// answer text (billed to the interactive subscription).
///
/// `task` is the full instruction body (meta-prompt + payload). `model` is the
/// tier to resolve (caller passes a small/fast one). `total` bounds the whole
/// thing. Handles spawn → ready → paste → read-transcript → teardown.
pub async fn claude_one_shot(
    plugin: &CliPlugin,
    shim_path: &Path,
    mcp_bin: &Path,
    server_url: &str,
    model: Option<String>,
    task: &str,
    total: Duration,
) -> Result<String, OneShotError> {
    let deadline = Instant::now() + total;

    // Throwaway workspace under the OS temp dir — same pattern as the probe: its
    // .mcp.json / .claude scratch dies with it, and it never enters the
    // `workspaces` table so it can't surface in the UI.
    let tmp = std::env::temp_dir().join(format!("swarmx-optq-{}", &Uuid::new_v4().to_string()[..8]));
    let layout = WorkspaceLayout::Shared { dir: tmp.clone() };

    let spawn = match spawn_agent(
        plugin, None, model, None, &layout, shim_path, mcp_bin, server_url, None,
    ) {
        Ok(s) => s,
        Err(e) => {
            crate::engine_probe::cleanup(&tmp, None);
            return Err(OneShotError::Spawn(e.to_string()));
        }
    };
    let agent_id = spawn.agent_id.clone();
    let slot = spawn.slot;

    // The transcript is read from `<cwd>/<session-id>.jsonl`. `slot.workspace` is
    // the exact cwd claude runs in; `transcript_session_id` is the uuid forced via
    // `--session-id`. Without both we can't locate the clean answer channel.
    let transcript = spawn
        .transcript_session_id
        .as_deref()
        .and_then(|sid| crate::transcript::claude_transcript_path(Path::new(&slot.workspace), sid));

    let result = match transcript {
        Some(path) => drive_turn(&slot, task, &path, deadline).await,
        None => Err(OneShotError::NoPty),
    };

    // Always tear down (kill + wipe scratch) however we leave this fn.
    slot.kill();
    drop(slot);
    crate::engine_probe::cleanup(&tmp, Some(&agent_id));
    result
}

/// The spawn-independent middle: wait ready, paste (with re-deliver), and read
/// the assistant turn from the transcript. Split out so `claude_one_shot` can
/// guarantee teardown around it.
async fn drive_turn(
    slot: &AgentSlot,
    task: &str,
    transcript: &Path,
    deadline: Instant,
) -> Result<String, OneShotError> {
    // 1. Wait for the shim/CLI to come up (or fail decisively).
    let mut rx = slot.lifecycle_tx.subscribe();
    wait_ready(&mut rx, deadline).await?;

    // 2. Let the TUI finish painting before the first paste.
    tokio::time::sleep(READY_SETTLE).await;

    let (Some(input), Some(stream)) = (slot.pty_input(), slot.pty_stream()) else {
        return Err(OneShotError::NoPty);
    };

    // 3. Deliver the prompt; re-deliver if the TUI shows no reaction (the first
    //    paste raced a not-yet-ready input box).
    let mut cursor = stream.snapshot().next_seq.saturating_sub(1);
    let mut buf: Vec<u8> = Vec::new();
    const SCAN_CAP: usize = 64 * 1024;

    if !deliver_prompt(&input, task).await {
        return Err(OneShotError::NoPty);
    }
    let mut sent_at = Instant::now();
    let mut len_at_send = buf.len();
    let mut deliveries = 1u32;

    // Once an assistant turn first appears, confirm it's stable before returning.
    let mut pending: Option<(String, Instant)> = None;

    while Instant::now() < deadline {
        // a. The clean channel: the assistant turn in the transcript.
        if let Some(text) = read_assistant_text(transcript) {
            match &pending {
                Some((prev, since)) if prev == &text && since.elapsed() >= SETTLE_CONFIRM => {
                    return Ok(text);
                }
                Some((prev, _)) if prev == &text => { /* still settling */ }
                _ => pending = Some((text, Instant::now())),
            }
        }

        // b. PTY output: only for auth detection + paste-reaction tracking.
        if let crate::pty_stream::FetchResult::Ok(entries) = stream.fetch_since(cursor) {
            for (seq, bytes) in entries {
                cursor = cursor.max(seq);
                buf.extend_from_slice(&bytes);
            }
        }
        if buf.len() > SCAN_CAP {
            buf.drain(..buf.len() - SCAN_CAP);
        }
        // Don't mistake a not-logged-in banner for "still working": only auth-fail
        // when no answer is pending.
        if pending.is_none() && has_auth_banner(&strip_ansi(&String::from_utf8_lossy(&buf))) {
            return Err(OneShotError::Auth);
        }

        // c. No reaction since the last delivery → the paste was dropped; re-send.
        if pending.is_none()
            && deliveries < MAX_DELIVERIES
            && sent_at.elapsed() >= REDELIVER_AFTER
            && buf.len() == len_at_send
        {
            if !deliver_prompt(&input, task).await {
                return Err(OneShotError::NoPty);
            }
            sent_at = Instant::now();
            len_at_send = buf.len();
            deliveries += 1;
        }
        tokio::time::sleep(POLL).await;
    }

    // Deadline. If a turn appeared but never settled, return it rather than fail.
    match pending {
        Some((text, _)) if !text.trim().is_empty() => Ok(text),
        _ => Err(OneShotError::Timeout),
    }
}

/// Read the LAST assistant turn's concatenated `text` blocks from a claude
/// transcript JSONL, or `None` if no assistant text is present yet. Skips
/// `thinking`/`tool_use` blocks and non-assistant rows. Brand-new throwaway
/// session ⇒ the only assistant turn is ours, so "last" is unambiguous.
fn read_assistant_text(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut last: Option<String> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(blocks) = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        let mut text = String::new();
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
        }
        if !text.trim().is_empty() {
            last = Some(text);
        }
    }
    last.map(|t| t.trim().to_string())
}

/// Deliver one prompt to a keystroke (claude/codex) TUI: Ctrl-U clears any
/// pre-typed line, the sanitized body pastes (embedded `\n` stay in the buffer,
/// they don't submit), then a standalone `\r` submits — with a safety `\r` after
/// the paste window closes (the split-write trick that defeats bracketed-paste
/// auto-submit suppression; see `wake::inject_with_kick_text`). Returns false if
/// the PTY input channel is gone.
async fn deliver_prompt(input: &tokio::sync::mpsc::Sender<bytes::Bytes>, prompt: &str) -> bool {
    let mut body = vec![0x15u8]; // Ctrl-U
    body.extend_from_slice(crate::spells::sanitize_pty_inject(prompt).as_bytes());
    let body_len = body.len() as u64;
    if input.send(bytes::Bytes::from(body)).await.is_err() {
        return false;
    }
    tokio::time::sleep(Duration::from_millis(200 + body_len / 80)).await;
    let _ = input.send(bytes::Bytes::from_static(b"\r")).await;
    tokio::time::sleep(Duration::from_millis(450)).await;
    let _ = input.send(bytes::Bytes::from_static(b"\r")).await;
    true
}

/// Wait until ShimReady, or fail on a decisive negative (auth needle / early
/// exit), or the deadline.
async fn wait_ready(
    rx: &mut tokio::sync::broadcast::Receiver<LifecycleEvent>,
    deadline: Instant,
) -> Result<(), OneShotError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(OneShotError::NotReady);
        }
        match tokio::time::timeout(deadline - now, rx.recv()).await {
            Ok(Ok(LifecycleEvent::ShimReady)) => return Ok(()),
            Ok(Ok(LifecycleEvent::HealthFail { kind, .. })) if kind == "auth" => {
                return Err(OneShotError::Auth);
            }
            Ok(Ok(LifecycleEvent::HealthFail { .. })) | Ok(Ok(LifecycleEvent::ShimExit(_))) => {
                return Err(OneShotError::NotReady);
            }
            // Broadcast lag/closed — keep waiting.
            Ok(Err(_)) => continue,
            Err(_) => return Err(OneShotError::NotReady),
        }
    }
}

/// True if the buffer carries a not-logged-in / unauthorized banner (mirrors the
/// probe's `classify_turn` auth needles).
fn has_auth_banner(buf: &str) -> bool {
    let low = buf.to_lowercase();
    const AUTH: &[&str] = &[
        "not logged in",
        "not authenticated",
        "unauthorized",
        "please log in",
        "please login",
        "/login",
        "invalid api key",
    ];
    AUTH.iter().any(|n| low.contains(n))
}

/// Strip ANSI/VT escape sequences and C0 control bytes (used only for the auth
/// banner scan of PTY output), keeping every multi-byte UTF-8 sequence verbatim.
/// Operates on bytes so a multi-byte char is never split, then lossy-decodes.
fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            match bytes.get(i + 1) {
                Some(b'[') => {
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    i += 1;
                }
                Some(b']') => {
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                Some(_) => i += 2,
                None => i += 1,
            }
            continue;
        }
        if b == b'\n' || b == b'\t' || (b >= 0x20 && b != 0x7f) {
            out.push(b);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_last_assistant_text_joining_blocks_skipping_thinking() {
        let dir = std::env::temp_dir().join(format!("fmx-optq-t-{}", &Uuid::new_v4().to_string()[..8]));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let body = [
            r#"{"type":"user","message":{"content":[{"type":"text","text":"原始草稿"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"改写第一段 "},{"type":"text","text":"with spaces kept"}]}}"#,
        ]
        .join("\n");
        std::fs::write(&path, body).unwrap();
        assert_eq!(
            read_assistant_text(&path).unwrap(),
            "改写第一段 with spaces kept"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_assistant_text_none_when_only_user_rows() {
        let dir = std::env::temp_dir().join(format!("fmx-optq-t-{}", &Uuid::new_v4().to_string()[..8]));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(&path, r#"{"type":"user","message":{"content":[{"type":"text","text":"x"}]}}"#).unwrap();
        assert!(read_assistant_text(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_assistant_text_missing_file_is_none() {
        assert!(read_assistant_text(Path::new("/no/such/swarmx/transcript.jsonl")).is_none());
    }

    #[test]
    fn strip_ansi_keeps_chinese_and_drops_escapes() {
        let raw = "\x1b[33m未\x1b[0m登录\r\nnot logged in\x1b]0;t\x07";
        let s = strip_ansi(raw);
        assert!(s.contains("未登录"));
        assert!(s.contains("not logged in"));
        assert!(!s.contains('\x1b'));
    }

    #[test]
    fn auth_banner_detected_case_insensitively() {
        assert!(has_auth_banner("Please LOG IN to continue"));
        assert!(has_auth_banner("Error: Unauthorized"));
        assert!(!has_auth_banner("改写后的提示词正文"));
    }
}
