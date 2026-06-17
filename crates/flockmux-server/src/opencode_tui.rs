//! Drive opencode's TUI over its built-in HTTP control API.
//!
//! opencode runs as a full-screen TUI in the PTY (so the terminal view and the
//! asciicast recordings work exactly like claude/codex), but its TUI cannot
//! reliably accept a *large* bootstrap prompt via keystroke injection: a
//! ~24k-char bracketed paste parks the TUI at READY without ever submitting
//! (verified live, opencode 1.17.7). opencode's own remedy is the documented
//! `/tui/*` HTTP endpoints its embedded server exposes — the same control
//! surface its browser extension drives. So flockmux spawns the TUI with a
//! known per-agent `--port <p>` and POSTs the prompt instead of faking
//! keystrokes: deterministic and size-independent.
//!
//! Used for BOTH the first-turn bootstrap and every wake "kick" for opencode
//! agents (claude/codex keep their keystroke path). A slot exposes its port via
//! `AgentSlot::tui_http_port()`; `Some(port)` is the signal to deliver here.

use anyhow::{anyhow, Context, Result};
use std::time::Duration;

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build opencode TUI http client")
}

/// Append `text` to the agent's TUI prompt and submit it as a fresh turn.
/// Clears any residual input first. Retries the append briefly so it rides out
/// a TUI server that is still binding its `--port` (the bootstrap normally runs
/// well after the agent's MCP pinged ready, but wakes and races can be earlier).
pub async fn deliver_turn(port: u16, text: &str) -> Result<()> {
    let base = format!("http://127.0.0.1:{port}");
    let c = client()?;

    // Best-effort clear of any residual buffer (a fresh prompt is already
    // empty; a wake mid-idle might have stray text). Failure is non-fatal.
    let _ = c
        .post(format!("{base}/tui/clear-prompt"))
        .json(&serde_json::json!({}))
        .send()
        .await;

    // Append the prompt body, retrying on transient connection errors with a
    // short backoff (~0.3s, 0.6s, 0.9s, 1.2s, 1.5s = ~4.5s budget).
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..5u32 {
        match c
            .post(format!("{base}/tui/append-prompt"))
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                last_err = None;
                break;
            }
            Ok(resp) => last_err = Some(anyhow!("append-prompt HTTP {}", resp.status())),
            Err(e) => last_err = Some(anyhow!("append-prompt send: {e}")),
        }
        tokio::time::sleep(Duration::from_millis(300 * (attempt as u64 + 1))).await;
    }
    if let Some(e) = last_err {
        return Err(e.context("opencode TUI append-prompt failed after retries"));
    }

    // Submit the buffered prompt as a user turn.
    let resp = c
        .post(format!("{base}/tui/submit-prompt"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .context("opencode TUI submit-prompt send")?;
    if !resp.status().is_success() {
        return Err(anyhow!("submit-prompt HTTP {}", resp.status()));
    }
    Ok(())
}
