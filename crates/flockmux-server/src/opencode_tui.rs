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
//!
//! TIMING (the load-bearing subtlety): opencode's TUI takes several seconds to
//! cold-start, and a `/tui/submit-prompt` sent before it's fully ready returns
//! 200 but is a **no-op** — the prompt sits in the input box and is never
//! submitted, so the model is never called and the captain parks forever (which
//! the first-response watchdog then misreads as "未登录"). The bootstrap path
//! therefore does NOT trust a single submit: it re-submits until opencode
//! actually starts a turn (a new input-bearing session appears) — see
//! [`deliver_bootstrap`]. Wakes hit an already-warm TUI, so [`deliver_turn`]
//! submits once.

use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::time::{Duration, Instant};

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build opencode TUI http client")
}

/// Clear the input box, append `text`, and submit it as a user turn. Each step
/// is one `/tui/*` POST. Returns the submit's success (the append is retried on
/// transient connection errors so it rides out a port still being bound).
async fn clear_append_submit(c: &reqwest::Client, base: &str, text: &str) -> Result<()> {
    // Best-effort clear (a no-op append earlier could have left residue).
    let _ = c
        .post(format!("{base}/tui/clear-prompt"))
        .json(&json!({}))
        .send()
        .await;

    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..5u32 {
        match c
            .post(format!("{base}/tui/append-prompt"))
            .json(&json!({ "text": text }))
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

    let resp = c
        .post(format!("{base}/tui/submit-prompt"))
        .json(&json!({}))
        .send()
        .await
        .context("opencode TUI submit-prompt send")?;
    if !resp.status().is_success() {
        return Err(anyhow!("submit-prompt HTTP {}", resp.status()));
    }
    Ok(())
}

/// Newest `created` (unix-ms) among opencode sessions whose `directory` matches
/// `workspace_dir` AND that have actually consumed input tokens (a prompt
/// reached the model). `None` if there are none. opencode creates such a session
/// the instant a prompt is *really* submitted, so a value that advances past the
/// pre-submit baseline is proof the TUI accepted our prompt and started a turn.
/// Filtering by directory keeps concurrent opencode agents (shared session
/// store) from satisfying each other's check.
async fn newest_started_turn(c: &reqwest::Client, base: &str, workspace_dir: &str) -> Option<i64> {
    let resp = c.get(format!("{base}/session")).send().await.ok()?;
    let arr = resp.json::<serde_json::Value>().await.ok()?;
    arr.as_array()?
        .iter()
        .filter(|s| {
            s.get("directory")
                .and_then(|d| d.as_str())
                .is_some_and(|d| dir_matches(d, workspace_dir))
        })
        .filter(|s| {
            s.get("tokens")
                .and_then(|t| t.get("input"))
                .and_then(|i| i.as_i64())
                .is_some_and(|i| i > 0)
        })
        .filter_map(|s| s.get("time").and_then(|t| t.get("created")).and_then(|c| c.as_i64()))
        .max()
}

/// opencode reports canonical paths (`/private/tmp/...` on macOS) while flockmux
/// may hold the un-canonicalised `/tmp/...`. Match on equality or either being a
/// suffix of the other so the `/private` prefix difference doesn't break it.
fn dir_matches(a: &str, b: &str) -> bool {
    a == b || a.ends_with(b) || b.ends_with(a)
}

/// Append `text` to the agent's TUI prompt and submit it as a fresh turn, ONCE.
/// For wakes: the agent is already warm (it just went idle), so a single submit
/// lands. The bootstrap path must use [`deliver_bootstrap`] instead — a cold TUI
/// silently drops a too-early submit.
pub async fn deliver_turn(port: u16, text: &str) -> Result<()> {
    let base = format!("http://127.0.0.1:{port}");
    let c = client()?;
    clear_append_submit(&c, &base, text).await
}

/// Deliver the FIRST-turn bootstrap to a freshly-spawned opencode TUI, retrying
/// until opencode actually starts a turn. A cold TUI accepts the HTTP POSTs (200)
/// but doesn't submit until it's fully initialised, so we (re)submit on an
/// interval and confirm via [`newest_started_turn`] that an input-bearing session
/// for this workspace appeared. `workspace_dir` scopes the confirmation to THIS
/// agent. Gives up after a generous window (returns the last submit error).
pub async fn deliver_bootstrap(port: u16, text: &str, workspace_dir: &str) -> Result<()> {
    let base = format!("http://127.0.0.1:{port}");
    let c = client()?;

    // The world as of now: any already-started turn for this workspace. Almost
    // always `None` (fresh per-agent workspace), but a re-spawn into the same
    // dir could have a stale one — we only accept a turn NEWER than this.
    let baseline = newest_started_turn(&c, &base, workspace_dir)
        .await
        .unwrap_or(0);

    // opencode's TUI cold-start + the model's first response is slow (reasoning
    // effort + a ~24k-char bootstrap can take 45-60s+ before the turn registers).
    // Keep retrying across the whole first-response watchdog window — if opencode
    // hasn't started a turn by then it really is wedged and the watchdog's failure
    // card is the right outcome.
    let start = Instant::now();
    let overall = Duration::from_secs(90);
    let mut last_err: Option<anyhow::Error> = None;
    loop {
        // Check BEFORE (re)submitting so a turn that already started is never
        // double-submitted into a duplicate turn.
        if newest_started_turn(&c, &base, workspace_dir)
            .await
            .unwrap_or(0)
            > baseline
        {
            return Ok(());
        }
        if start.elapsed() > overall {
            return Err(last_err.unwrap_or_else(|| {
                anyhow!(
                    "opencode did not start a turn within {}s — TUI never became ready?",
                    overall.as_secs()
                )
            }));
        }
        if let Err(e) = clear_append_submit(&c, &base, text).await {
            last_err = Some(e);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
