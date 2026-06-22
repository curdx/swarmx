//! `GET /api/usage` — token/cost observability.
//!
//! swarmx can't ask claude/codex for spend (we drive them over a PTY, not an
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

/// Embedded LiteLLM pricing snapshot (USD per 1M tokens), refreshed via
/// `scripts/update-litellm-pricing.mjs`. This is the FALLBACK rate source: the
/// hand-maintained `default_pricing_rules()` below stay the editable primary
/// layer (and a user's pricing.json overrides everything), but any model id no
/// rule matches falls through to this table — so a brand-new model auto-prices
/// instead of showing tokens-only. Same source ccusage uses.
const LITELLM_PRICING_JSON: &str = include_str!("../../resources/litellm_pricing.json");

#[derive(Clone, Copy, Debug, Deserialize)]
struct LiteLlmEntry {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    #[serde(default)]
    context_window: Option<u32>,
}

fn litellm_table() -> &'static std::collections::HashMap<String, LiteLlmEntry> {
    static TABLE: std::sync::OnceLock<std::collections::HashMap<String, LiteLlmEntry>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| match serde_json::from_str(LITELLM_PRICING_JSON) {
        Ok(map) => map,
        Err(err) => {
            tracing::error!(?err, "embedded litellm_pricing.json failed to parse; fallback pricing disabled");
            std::collections::HashMap::new()
        }
    })
}

/// Normalise a model id toward a LiteLLM key: lowercase, drop a provider prefix
/// (`anthropic/claude-…`), and strip swarmx's 1M-context markers (`[1m]` /
/// `-1m`) that LiteLLM keys don't carry.
fn normalize_model(model: &str) -> String {
    let mut m = model.trim().to_ascii_lowercase();
    m = m.replace("[1m]", "");
    if let Some(stripped) = m.strip_suffix("-1m") {
        m = stripped.to_string();
    }
    if let Some(idx) = m.rfind('/') {
        m = m[idx + 1..].to_string();
    }
    m.trim().to_string()
}

/// Look a model up in the embedded LiteLLM table: exact lowercase first, then
/// the normalised form. None for models LiteLLM doesn't know either.
fn litellm_lookup(model: &str) -> Option<&'static LiteLlmEntry> {
    let table = litellm_table();
    let lower = model.trim().to_ascii_lowercase();
    table
        .get(&lower)
        .or_else(|| table.get(&normalize_model(model)))
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
        PricingRule {
            id: "deepseek".into(),
            provider: "DeepSeek".into(),
            label: "DeepSeek (Reasonix)".into(),
            matchers: vec!["deepseek".into()],
            context_window: Some(1_000_000),
            rates_usd_per_mtok: Rate {
                // Approximate, from DeepSeek's published V3.x API rates — V4
                // (deepseek-v4-flash / -pro, used by reasonix) prices are not yet
                // officially confirmed. Cache-hit input is ~1/4 of cache-miss,
                // which is reasonix's whole cost story. Adjust when V4 rates land.
                input: 0.27,
                output: 1.10,
                cache_read: 0.07,
                cache_write: 0.27,
            },
            note: "Approximate (DeepSeek V3.x rates); V4 pricing unconfirmed. \
                   Matches deepseek-v4-flash / deepseek-v4-pro via 'deepseek'."
                .into(),
        },
    ]
}

fn pricing_config_path() -> PathBuf {
    // P1-39: HOME isn't set on Windows — fall back to USERPROFILE there so the
    // installed app writes/reads `~/.swarmx/pricing.json` instead of a
    // CWD-relative `.swarmx/pricing.json` (which, with CWD=`/` under the
    // sidecar, would make save/reset silently fail).
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(home).join(".swarmx").join("pricing.json");
    }
    PathBuf::from(".swarmx/pricing.json")
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
    let tmp = path.with_extension("json.swarmx-tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Best-effort rate lookup. Primary layer: model-name substring against the
/// (user-editable) pricing rules. Fallback: exact/normalised match in the
/// embedded LiteLLM snapshot. Returns None only when neither knows the model
/// (cost contribution = 0, `priced` flips false).
fn rate_for(model: &str, rules: &[PricingRule]) -> Option<Rate> {
    let m = model.to_ascii_lowercase();
    if let Some(rule) = rules.iter().find(|rule| {
        rule.matchers
            .iter()
            .any(|needle| m.contains(&needle.to_ascii_lowercase()))
    }) {
        return Some(rule.rates_usd_per_mtok);
    }
    litellm_lookup(model).map(|e| Rate {
        input: e.input,
        output: e.output,
        cache_read: e.cache_read,
        cache_write: e.cache_write,
    })
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
    if let Some(cw) = rules
        .iter()
        .find(|rule| {
            rule.matchers
                .iter()
                .any(|needle| m.contains(&needle.to_ascii_lowercase()))
        })
        .and_then(|rule| rule.context_window)
    {
        return Some(cw);
    }
    litellm_lookup(model).and_then(|e| e.context_window)
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
    // P1-37: don't unwrap_or_default() DB errors into empty stats — that renders
    // "你还没有用量" when the query actually failed. Surface a 500 so the page
    // shows a load error instead of a false "no usage yet".
    let usage_err = |e: anyhow::Error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response()
    };
    let by_model = match store.usage_by_model(ws.clone()).await {
        Ok(r) => r,
        Err(e) => return usage_err(e),
    };
    let by_day = match store.usage_by_day(90, ws.clone()).await {
        Ok(r) => r,
        Err(e) => return usage_err(e),
    };
    let by_agent = match store.usage_by_agent(ws).await {
        Ok(r) => r,
        Err(e) => return usage_err(e),
    };

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
    .into_response()
}

pub async fn usage_pricing_get() -> impl IntoResponse {
    let (rules, source) = load_pricing_rules();
    Json(json!({
        "unit": "USD per 1M tokens",
        "source": source,
        "path": pricing_config_path(),
        "rules": rules,
        // Models no rule matches fall through to this embedded LiteLLM snapshot
        // (refresh: scripts/update-litellm-pricing.mjs) so new models still price.
        "fallback": {
            "source": "litellm",
            "models": litellm_table().len(),
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn litellm_snapshot_parses_and_is_populated() {
        // If include_str! or the JSON shape broke, this drops to 0 and every
        // fallback price silently disappears — guard against that.
        assert!(
            litellm_table().len() > 1000,
            "embedded snapshot should hold the full table, got {}",
            litellm_table().len()
        );
    }

    #[test]
    fn litellm_conversion_is_correct() {
        // Anchors the USD-per-token -> USD-per-1M-token (×1e6) conversion against
        // a known published price. opus: 1.5e-5/7.5e-5/1.5e-6/1.875e-5 per token.
        let opus = litellm_lookup("claude-opus-4-1").expect("opus in snapshot");
        assert_eq!(opus.input, 15.0);
        assert_eq!(opus.output, 75.0);
        assert_eq!(opus.cache_read, 1.5);
        assert_eq!(opus.cache_write, 18.75);
        assert_eq!(opus.context_window, Some(200_000));

        // A provider without cache-creation pricing must come through as 0, not absent.
        let codex = litellm_lookup("gpt-5-codex").expect("gpt-5-codex in snapshot");
        assert_eq!(codex.cache_write, 0.0);
    }

    #[test]
    fn normalize_strips_prefix_and_1m_markers() {
        assert_eq!(normalize_model("anthropic/claude-opus-4-1"), "claude-opus-4-1");
        assert_eq!(normalize_model("claude-opus-4-8[1m]"), "claude-opus-4-8");
        assert_eq!(normalize_model("Gemini-2.5-Pro-1m"), "gemini-2.5-pro");
    }

    #[test]
    fn primary_rules_win_over_litellm_fallback() {
        // opus hits the hand-maintained substring rule; the [1m] marker must not
        // knock it down to the fallback.
        let rules = default_pricing_rules();
        let r = rate_for("claude-opus-4-8[1m]", &rules).expect("opus priced");
        assert_eq!(r.input, 15.0);
        assert_eq!(r.output, 75.0);
    }

    #[test]
    fn litellm_fallback_prices_models_no_rule_covers() {
        // gemini matches none of opus/sonnet/haiku/gpt-5/codex/o4 — before the
        // fallback it was tokens-only; now it prices and surfaces a window.
        let rules = default_pricing_rules();
        let r = rate_for("gemini-2.5-pro", &rules).expect("gemini priced via fallback");
        assert!(r.input > 0.0 && r.output > 0.0);
        assert!(context_window_for("gemini-2.5-pro", &rules).is_some());
    }

    #[test]
    fn truly_unknown_model_stays_unpriced() {
        let rules = default_pricing_rules();
        assert!(rate_for("totally-made-up-model-xyz", &rules).is_none());
        assert!(cost_of(Some("totally-made-up-model-xyz"), 100, 100, 0, 0, &rules).is_none());
    }

    #[test]
    fn cost_of_computes_the_actual_money() {
        // The pricing-LOOKUP tests above all verify which RATE matches a model,
        // but none asserted the arithmetic that turns tokens+rate into dollars.
        // Pin it against a hand-computed value so a swapped rate, a `+`→`*`
        // typo, or a drifted /1e6 divisor can't slip through.
        //
        // opus: input 15, output 75, cache_read 1.5, cache_write 18.75 (USD/Mtok).
        // 1M input + 1M output + 1M cache_read + 1M cache_write should be exactly
        // the sum of the four rates: 15 + 75 + 1.5 + 18.75 = 110.25.
        let rules = default_pricing_rules();
        let c = cost_of(Some("claude-opus-4-1"), 1_000_000, 1_000_000, 1_000_000, 1_000_000, &rules)
            .expect("opus is priced");
        assert!((c - 110.25).abs() < 1e-9, "expected 110.25, got {c}");

        // Each token class is weighted by ITS OWN rate, not lumped together:
        // 2M output @75 = 150; nothing else. Catches an input/output swap.
        let out_only = cost_of(Some("claude-opus-4-1"), 0, 2_000_000, 0, 0, &rules).unwrap();
        assert!((out_only - 150.0).abs() < 1e-9, "expected 150.0, got {out_only}");

        // Linear in token count and starts at zero.
        let half = cost_of(Some("claude-opus-4-1"), 500_000, 0, 0, 0, &rules).unwrap();
        assert!((half - 7.5).abs() < 1e-9, "expected 7.5, got {half}");
        let zero = cost_of(Some("claude-opus-4-1"), 0, 0, 0, 0, &rules).unwrap();
        assert_eq!(zero, 0.0);
    }
}
