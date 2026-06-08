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
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::AppState;

/// USD per 1,000,000 tokens. (input, output, cache_read, cache_write).
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct Rate {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PricingRule {
    id: String,
    provider: String,
    label: String,
    matchers: Vec<String>,
    context_window: Option<u32>,
    rates_usd_per_mtok: Rate,
    note: String,
}

#[derive(Deserialize, Serialize)]
pub struct PricingUpdate {
    rules: Vec<PricingRule>,
}

fn default_pricing_rules() -> Vec<PricingRule> {
    vec![
        PricingRule {
            id: "anthropic-opus".into(),
            provider: "Anthropic".into(),
            label: "Claude Opus".into(),
            matchers: vec!["opus".into()],
            context_window: Some(200_000),
            rates_usd_per_mtok: Rate {
                input: 15.0,
                output: 75.0,
                cache_read: 1.5,
                cache_write: 18.75,
            },
            note: "Claude 1M-context variants are detected from model ids containing [1m] or -1m."
                .into(),
        },
        PricingRule {
            id: "anthropic-sonnet".into(),
            provider: "Anthropic".into(),
            label: "Claude Sonnet".into(),
            matchers: vec!["sonnet".into()],
            context_window: Some(200_000),
            rates_usd_per_mtok: Rate {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
            },
            note: "Claude 1M-context variants are detected from model ids containing [1m] or -1m."
                .into(),
        },
        PricingRule {
            id: "anthropic-haiku".into(),
            provider: "Anthropic".into(),
            label: "Claude Haiku".into(),
            matchers: vec!["haiku".into()],
            context_window: Some(200_000),
            rates_usd_per_mtok: Rate {
                input: 0.8,
                output: 4.0,
                cache_read: 0.08,
                cache_write: 1.0,
            },
            note: String::new(),
        },
        PricingRule {
            id: "openai-codex-gpt5".into(),
            provider: "OpenAI".into(),
            label: "GPT-5 / Codex family".into(),
            matchers: vec!["gpt-5".into(), "gpt5".into(), "codex".into(), "o4".into()],
            context_window: Some(272_000),
            rates_usd_per_mtok: Rate {
                input: 1.25,
                output: 10.0,
                cache_read: 0.125,
                cache_write: 1.25,
            },
            note: "Approximation for codex CLI token_count model ids.".into(),
        },
    ]
}

fn pricing_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".flockmux").join("pricing.json");
    }
    PathBuf::from(".flockmux/pricing.json")
}

fn validate_pricing_rules(rules: &[PricingRule]) -> Result<(), String> {
    if rules.is_empty() {
        return Err("pricing rules must not be empty".into());
    }
    for rule in rules {
        if rule.id.trim().is_empty() {
            return Err("pricing rule id must not be empty".into());
        }
        if rule.matchers.iter().all(|m| m.trim().is_empty()) {
            return Err(format!(
                "pricing rule {} needs at least one matcher",
                rule.id
            ));
        }
        let rates = [
            ("input", rule.rates_usd_per_mtok.input),
            ("output", rule.rates_usd_per_mtok.output),
            ("cache_read", rule.rates_usd_per_mtok.cache_read),
            ("cache_write", rule.rates_usd_per_mtok.cache_write),
        ];
        for (name, value) in rates {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("pricing rule {} has invalid {name} rate", rule.id));
            }
        }
    }
    Ok(())
}

fn load_pricing_rules() -> (Vec<PricingRule>, &'static str) {
    let path = pricing_config_path();
    match std::fs::read_to_string(&path) {
        Ok(txt) => match serde_json::from_str::<PricingUpdate>(&txt) {
            Ok(update) if validate_pricing_rules(&update.rules).is_ok() => (update.rules, "user"),
            Ok(_) => {
                tracing::warn!(path = %path.display(), "pricing.json validation failed; using defaults");
                (default_pricing_rules(), "default")
            }
            Err(err) => {
                tracing::warn!(?err, path = %path.display(), "pricing.json parse failed; using defaults");
                (default_pricing_rules(), "default")
            }
        },
        Err(_) => (default_pricing_rules(), "default"),
    }
}

fn save_pricing_rules(rules: &[PricingRule]) -> anyhow::Result<()> {
    use std::io::Write;
    let path = pricing_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&PricingUpdate {
        rules: rules.to_vec(),
    })?;
    let tmp = path.with_extension("json.flockmux-tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Best-effort rate lookup by model-name substring. Returns None for unknown
/// models (cost contribution = 0, `priced` flips false).
fn rate_for(model: &str, rules: &[PricingRule]) -> Option<Rate> {
    let m = model.to_ascii_lowercase();
    rules
        .iter()
        .find(|rule| {
            rule.matchers
                .iter()
                .any(|needle| m.contains(&needle.to_ascii_lowercase()))
        })
        .map(|rule| rule.rates_usd_per_mtok)
}

/// Best-effort context-window size (tokens) by model-name substring. Surfaced
/// in the Usage table so the operator can eyeball headroom. Returns None for
/// unknown models (UI shows "—"). Provider-aware where it matters (codex via
/// OAuth caps lower than the model's nominal window).
fn context_window_for(model: &str, rules: &[PricingRule]) -> Option<u32> {
    let m = model.to_ascii_lowercase();
    if (m.contains("opus") || m.contains("sonnet")) && (m.contains("[1m]") || m.contains("-1m")) {
        // 1M-context betas exist; the safe default for claude opus/sonnet is 200k.
        return Some(1_000_000);
    }
    rules
        .iter()
        .find(|rule| {
            rule.matchers
                .iter()
                .any(|needle| m.contains(&needle.to_ascii_lowercase()))
        })
        .and_then(|rule| rule.context_window)
}

fn cost_of(
    model: Option<&str>,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    rules: &[PricingRule],
) -> Option<f64> {
    let r = rate_for(model.unwrap_or(""), rules)?;
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
    /// The model's static context-window cap (tokens); null for unknown models.
    context_window: Option<u32>,
    /// Estimated peak context occupancy (tokens) — how full the window got.
    context_peak: i64,
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
    let (pricing_rules, _) = load_pricing_rules();
    let ws = q.workspace_id.filter(|s| !s.is_empty());
    let by_model = store.usage_by_model(ws.clone()).await.unwrap_or_default();
    let by_day = store.usage_by_day(90, ws.clone()).await.unwrap_or_default();
    let by_agent = store.usage_by_agent(ws).await.unwrap_or_default();

    let mut models = Vec::with_capacity(by_model.len());
    let (mut t_in, mut t_out, mut t_cr, mut t_cw, mut t_ev, mut t_cost) =
        (0i64, 0i64, 0i64, 0i64, 0i64, 0f64);
    let mut all_priced = true;
    for m in &by_model {
        let cost = cost_of(
            m.model.as_deref(),
            m.input_tokens,
            m.output_tokens,
            m.cache_read_tokens,
            m.cache_write_tokens,
            &pricing_rules,
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
            context_window: m
                .model
                .as_deref()
                .and_then(|model| context_window_for(model, &pricing_rules)),
            context_peak: m.context_peak,
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

pub async fn usage_pricing_get() -> impl IntoResponse {
    let (rules, source) = load_pricing_rules();
    Json(json!({
        "unit": "USD per 1M tokens",
        "source": source,
        "path": pricing_config_path(),
        "rules": rules,
    }))
}

pub async fn usage_pricing_put(Json(update): Json<PricingUpdate>) -> Response {
    if let Err(error) = validate_pricing_rules(&update.rules) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    match save_pricing_rules(&update.rules) {
        Ok(()) => usage_pricing_get().await.into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

pub async fn usage_pricing_reset() -> Response {
    let path = pricing_config_path();
    match std::fs::remove_file(&path) {
        Ok(()) => usage_pricing_get().await.into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            usage_pricing_get().await.into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}
