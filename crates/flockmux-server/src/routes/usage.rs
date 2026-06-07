//! `GET /api/usage` — token/cost observability.
//!
//! flockmux can't ask claude/codex for spend (we drive them over a PTY, not an
//! API), so the transcript tailer scrapes per-turn token counts from each
//! worker's session JSONL into `agent_usage` (migration 0016). This endpoint
//! aggregates that table and applies a pricing table to derive cost.
//!
//! Pricing lives HERE (not in the DB) so re-pricing never needs a migration.
//! The rates below are approximate published list prices (USD / 1M tokens,
//! 2026) — a model we don't recognise contributes tokens but `cost_usd = 0`
//! and flips `priced = false` so the UI can show "tokens only" honestly.

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;

/// USD per 1,000,000 tokens. (input, output, cache_read, cache_write).
struct Rate {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

/// Best-effort rate lookup by model-name substring. Returns None for unknown
/// models (cost contribution = 0, `priced` flips false).
fn rate_for(model: &str) -> Option<Rate> {
    let m = model.to_ascii_lowercase();
    // Anthropic
    if m.contains("opus") {
        return Some(Rate { input: 15.0, output: 75.0, cache_read: 1.5, cache_write: 18.75 });
    }
    if m.contains("sonnet") {
        return Some(Rate { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 });
    }
    if m.contains("haiku") {
        return Some(Rate { input: 0.8, output: 4.0, cache_read: 0.08, cache_write: 1.0 });
    }
    // OpenAI / codex family (approx; codex CLI reports gpt-5* ids)
    if m.contains("gpt-5") || m.contains("codex") || m.contains("o4") || m.contains("gpt5") {
        return Some(Rate { input: 1.25, output: 10.0, cache_read: 0.125, cache_write: 1.25 });
    }
    None
}

/// Best-effort context-window size (tokens) by model-name substring. Surfaced
/// in the Usage table so the operator can eyeball headroom. Returns None for
/// unknown models (UI shows "—"). Provider-aware where it matters (codex via
/// OAuth caps lower than the model's nominal window).
fn context_window_for(model: &str) -> Option<u32> {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") || m.contains("sonnet") {
        // 1M-context betas exist; the safe default for claude opus/sonnet is 200k.
        return Some(if m.contains("[1m]") || m.contains("-1m") { 1_000_000 } else { 200_000 });
    }
    if m.contains("haiku") {
        return Some(200_000);
    }
    if m.contains("gpt-5") || m.contains("gpt5") || m.contains("codex") || m.contains("o4") {
        return Some(272_000);
    }
    None
}

fn cost_of(model: Option<&str>, input: i64, output: i64, cache_read: i64, cache_write: i64) -> Option<f64> {
    let r = rate_for(model.unwrap_or(""))?;
    let per = |toks: i64, rate: f64| (toks as f64) / 1_000_000.0 * rate;
    Some(
        per(input, r.input)
            + per(output, r.output)
            + per(cache_read, r.cache_read)
            + per(cache_write, r.cache_write),
    )
}

#[derive(Serialize)]
struct ModelRow {
    model: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    events: i64,
    cost_usd: f64,
    priced: bool,
    context_window: Option<u32>,
}

#[derive(Deserialize)]
pub struct UsageQuery {
    /// Scope usage to one workspace; empty/absent = all workspaces.
    workspace_id: Option<String>,
}

pub async fn usage_summary(
    State(state): State<AppState>,
    Query(q): Query<UsageQuery>,
) -> impl IntoResponse {
    let store = &state.store;
    let ws = q.workspace_id.filter(|s| !s.is_empty());
    let by_model = store.usage_by_model(ws.clone()).await.unwrap_or_default();
    let by_day = store.usage_by_day(90, ws.clone()).await.unwrap_or_default();
    let by_agent = store.usage_by_agent(ws).await.unwrap_or_default();

    let mut models = Vec::with_capacity(by_model.len());
    let (mut t_in, mut t_out, mut t_cr, mut t_cw, mut t_ev, mut t_cost) = (0i64, 0i64, 0i64, 0i64, 0i64, 0f64);
    let mut all_priced = true;
    for m in &by_model {
        let cost = cost_of(
            m.model.as_deref(),
            m.input_tokens,
            m.output_tokens,
            m.cache_read_tokens,
            m.cache_write_tokens,
        );
        let priced = cost.is_some();
        if !priced {
            all_priced = false;
        }
        let cost = cost.unwrap_or(0.0);
        t_in += m.input_tokens;
        t_out += m.output_tokens;
        t_cr += m.cache_read_tokens;
        t_cw += m.cache_write_tokens;
        t_ev += m.events;
        t_cost += cost;
        models.push(ModelRow {
            model: m.model.clone(),
            input_tokens: m.input_tokens,
            output_tokens: m.output_tokens,
            cache_read_tokens: m.cache_read_tokens,
            cache_write_tokens: m.cache_write_tokens,
            events: m.events,
            cost_usd: cost,
            priced,
            context_window: m.model.as_deref().and_then(context_window_for),
        });
    }

    Json(json!({
        "totals": {
            "input_tokens": t_in,
            "output_tokens": t_out,
            "cache_read_tokens": t_cr,
            "cache_write_tokens": t_cw,
            "events": t_ev,
            "cost_usd": t_cost,
            "priced": all_priced,
        },
        "by_model": models,
        "by_day": by_day,
        "by_agent": by_agent,
    }))
}
