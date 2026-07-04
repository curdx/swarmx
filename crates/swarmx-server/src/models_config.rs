//! Per-CLI model configuration (F1 配给智能, model layer).
//!
//! Maps an ABSTRACT tier (`opus`/`sonnet`/`haiku`) to a CONCRETE model id PER
//! CLI, plus a per-CLI default. Resolved at the spawn chokepoint so a role or
//! spawn that asks for tier `sonnet` gets the right concrete model for whatever
//! CLI is actually launching: claude keeps its alias (`--model sonnet` works),
//! codex gets the user's mapped model — or, if unmapped, codex's OWN default —
//! never a bogus `sonnet` forwarded to a custom provider (the 503 class we hit).
//!
//! Persisted as `~/.swarmx/models.json`, edited via the 模型 settings page
//! (GET/PUT `/api/models`). An absent file ⇒ shipped defaults ⇒ behaviour
//! identical to legacy: claude tiers pass through, codex tiers fall to codex's
//! own default. User config separate from the CLI's personal config (like the
//! model overlay always was — swarmx-local, not read from ~/.claude/~/.codex).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Abstract tier vocabulary. A requested model that is NOT one of these is
/// treated as a concrete model id and passed through verbatim (lets an
/// orchestrator pin an exact model).
pub const KNOWN_TIERS: &[&str] = &["opus", "sonnet", "haiku"];

/// One CLI's model mapping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CliModels {
    /// Concrete model id to use when a spawn resolves to NO tier (sits above
    /// `plugin.default_model`). Empty ⇒ let the CLI pick its own default.
    #[serde(default)]
    pub default: String,
    /// tier → concrete model id. A present-but-empty value means "this tier
    /// explicitly uses the CLI's own default" (emit no `--model`). An absent
    /// tier key falls back to `default`.
    #[serde(default)]
    pub tiers: BTreeMap<String, String>,
    /// Global default reasoning/thinking effort for this CLI (abstract level
    /// low|medium|high|max). Empty ⇒ the model's own default (emit no effort
    /// flag). A per-direction `thread.reasoning_effort` overrides this; this is
    /// the fallback applied at the spawn chokepoint when a direction sets none.
    /// Mapped to the CLI's concrete flag via the plugin's `effort_levels`.
    #[serde(default)]
    pub effort: String,
}

/// The whole model config (all CLIs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    /// cli id → its model mapping.
    #[serde(default)]
    pub clis: BTreeMap<String, CliModels>,
}

fn default_version() -> u32 {
    1
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self::shipped()
    }
}

impl ModelConfig {
    /// Shipped defaults: claude tiers identity-map (claude accepts opus/sonnet/
    /// haiku aliases, so behaviour == legacy); codex left empty (any tier →
    /// codex's own default, never a 503). A CLI not listed here ⇒ pure
    /// passthrough for concrete ids, default for tiers.
    pub fn shipped() -> Self {
        let mut clis = BTreeMap::new();
        let mut claude_tiers = BTreeMap::new();
        for t in KNOWN_TIERS {
            claude_tiers.insert(t.to_string(), t.to_string());
        }
        clis.insert(
            "claude".into(),
            CliModels {
                default: String::new(),
                tiers: claude_tiers,
                effort: String::new(),
            },
        );
        clis.insert("codex".into(), CliModels::default());
        Self { version: 1, clis }
    }

    /// Resolve a requested model/tier for `cli` into the concrete model to pass
    /// via `--model`, or `None` to emit no flag (→ plugin/CLI default).
    ///
    /// Precedence:
    /// 1. explicit per-CLI tier mapping (non-empty value) → that concrete id;
    ///    mapped-but-empty → `None` (this tier uses the CLI's own default).
    /// 2. a KNOWN abstract tier with no mapping → the per-CLI `default`, else
    ///    `None`. We do NOT forward the bare tier name (that 503s on a custom
    ///    provider that doesn't know "sonnet").
    /// 3. anything else (a concrete model id, not a known tier) → verbatim.
    /// 4. nothing requested → per-CLI `default`, else `None`.
    pub fn resolve(&self, cli: &str, requested: Option<&str>) -> Option<String> {
        let cli_cfg = self.clis.get(cli);
        let per_cli_default = || {
            cli_cfg
                .map(|c| c.default.trim())
                .filter(|d| !d.is_empty())
                .map(|d| d.to_string())
        };
        match requested.map(str::trim).filter(|s| !s.is_empty()) {
            Some(req) => {
                if let Some(mapped) = cli_cfg.and_then(|c| c.tiers.get(req)) {
                    let m = mapped.trim();
                    return if m.is_empty() {
                        None
                    } else {
                        Some(m.to_string())
                    };
                }
                if KNOWN_TIERS.contains(&req) {
                    return per_cli_default();
                }
                Some(req.to_string())
            }
            None => per_cli_default(),
        }
    }

    /// Global default reasoning effort for `cli` (abstract level), or `None` if
    /// unset. The per-direction `thread.reasoning_effort` takes precedence; this
    /// is the fallback applied at the spawn chokepoint.
    pub fn effort_for(&self, cli: &str) -> Option<String> {
        self.clis
            .get(cli)
            .map(|c| c.effort.trim())
            .filter(|e| !e.is_empty())
            .map(|e| e.to_string())
    }
}

/// `~/.swarmx/models.json`.
pub fn models_config_path() -> PathBuf {
    // HOME is unset on Windows — fall back to USERPROFILE so the installed app
    // writes/reads `~/.swarmx/models.json` instead of a CWD-relative path (with
    // CWD=`/` under the Tauri sidecar, that would silently never persist).
    // Mirrors pricing_config_path's P1-39 fix.
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(home).join(".swarmx").join("models.json");
    }
    PathBuf::from(".swarmx/models.json")
}

/// Load the config, layering the on-disk file over the shipped defaults so a
/// fresh install (no file) behaves exactly like legacy, and a file that only
/// customizes one CLI keeps the shipped entries for the others.
pub fn load_or_default() -> ModelConfig {
    let path = models_config_path();
    let mut cfg = ModelConfig::shipped();
    match std::fs::read_to_string(&path) {
        Ok(txt) => match serde_json::from_str::<ModelConfig>(&txt) {
            Ok(loaded) => {
                cfg.version = loaded.version;
                // Per-CLI wholesale override (the settings UI always PUTs the
                // complete object); CLIs absent from the file keep shipped.
                for (cli, m) in loaded.clis {
                    cfg.clis.insert(cli, m);
                }
            }
            Err(e) => {
                tracing::warn!(?e, path = %path.display(), "models.json parse failed; using shipped defaults")
            }
        },
        Err(_) => { /* no file → shipped defaults */ }
    }
    cfg
}

/// A process- and call-unique temp extension (`json.<pid>.<seq>.swarmx-tmp`) for
/// atomic config writes. A *fixed* temp name lets two concurrent saves race:
/// writer B `File::create`s the same tmp A just filled, truncating it, then A
/// renames the now-empty tmp over the real config. A unique name per call keeps
/// each writer's temp private, so the rename is always atomic and complete.
pub(crate) fn unique_tmp_ext() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "json.{}.{}.swarmx-tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Atomically persist to `~/.swarmx/models.json` (temp → fsync → rename),
/// creating the parent dir if needed. Mirrors the pre_spawn/mcp_admin pattern.
pub fn save(cfg: &ModelConfig) -> anyhow::Result<()> {
    use std::io::Write;
    let path = models_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)?;
    let tmp = path.with_extension(unique_tmp_ext());
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_codex_map(pairs: &[(&str, &str)], default: &str) -> ModelConfig {
        let mut cfg = ModelConfig::shipped();
        let tiers = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        cfg.clis.insert(
            "codex".into(),
            CliModels {
                default: default.to_string(),
                tiers,
                effort: String::new(),
            },
        );
        cfg
    }

    #[test]
    fn unique_tmp_ext_is_distinct_per_call() {
        // A fixed temp name lets concurrent saves clobber each other; each call
        // must yield a different extension so the tmp→rename stays atomic.
        let a = unique_tmp_ext();
        let b = unique_tmp_ext();
        assert_ne!(a, b);
        assert!(a.ends_with(".swarmx-tmp") && b.ends_with(".swarmx-tmp"));
    }

    #[test]
    fn claude_tiers_identity_legacy_behaviour() {
        let c = ModelConfig::shipped();
        assert_eq!(
            c.resolve("claude", Some("sonnet")).as_deref(),
            Some("sonnet")
        );
        assert_eq!(c.resolve("claude", Some("opus")).as_deref(), Some("opus"));
    }

    #[test]
    fn codex_known_tier_unmapped_does_not_forward_bare_tier() {
        // The 503 class: shipped codex has no tier map + empty default → a tier
        // request resolves to None (no --model), NOT the literal "sonnet".
        let c = ModelConfig::shipped();
        assert_eq!(c.resolve("codex", Some("sonnet")), None);
        assert_eq!(c.resolve("codex", Some("opus")), None);
    }

    #[test]
    fn codex_mapped_tier_uses_concrete_model() {
        let c = with_codex_map(&[("sonnet", "gpt-5.5")], "");
        assert_eq!(
            c.resolve("codex", Some("sonnet")).as_deref(),
            Some("gpt-5.5")
        );
    }

    #[test]
    fn mapped_to_empty_means_cli_default() {
        let c = with_codex_map(&[("sonnet", "")], "");
        assert_eq!(c.resolve("codex", Some("sonnet")), None);
    }

    #[test]
    fn concrete_model_id_passes_through_verbatim() {
        // Not a known tier ⇒ an orchestrator pinned an exact model ⇒ verbatim.
        let c = ModelConfig::shipped();
        assert_eq!(
            c.resolve("codex", Some("gpt-5.5")).as_deref(),
            Some("gpt-5.5")
        );
        assert_eq!(
            c.resolve("claude", Some("claude-opus-4-8")).as_deref(),
            Some("claude-opus-4-8")
        );
    }

    #[test]
    fn none_request_uses_per_cli_default_or_none() {
        let c = ModelConfig::shipped();
        assert_eq!(c.resolve("codex", None), None); // empty default
        let c2 = with_codex_map(&[], "gpt-5.5");
        assert_eq!(c2.resolve("codex", None).as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn unknown_cli_known_tier_none_concrete_verbatim() {
        let c = ModelConfig::shipped();
        // unknown cli, known tier, no config → None (no bare-tier forward)
        assert_eq!(c.resolve("gemini", Some("sonnet")), None);
        // unknown cli, concrete id → verbatim
        assert_eq!(
            c.resolve("gemini", Some("gemini-2.0")).as_deref(),
            Some("gemini-2.0")
        );
    }

    #[test]
    fn load_merge_keeps_shipped_claude_when_only_codex_in_file() {
        let shipped = ModelConfig::shipped();
        // Simulate a file that only customized codex.
        let mut file = ModelConfig {
            version: 1,
            clis: BTreeMap::new(),
        };
        file.clis.insert(
            "codex".into(),
            CliModels {
                default: "gpt-5.5".into(),
                tiers: BTreeMap::new(),
                effort: String::new(),
            },
        );
        // Manual merge mirrors load_or_default's logic.
        let mut merged = shipped.clone();
        for (cli, m) in file.clis {
            merged.clis.insert(cli, m);
        }
        assert_eq!(
            merged.resolve("claude", Some("sonnet")).as_deref(),
            Some("sonnet"),
            "claude identity tiers survive a codex-only file"
        );
        assert_eq!(merged.resolve("codex", None).as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn roundtrip_serde() {
        let c = with_codex_map(&[("sonnet", "gpt-5.5"), ("opus", "o3")], "gpt-5.5");
        let json = serde_json::to_string(&c).unwrap();
        let back: ModelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
