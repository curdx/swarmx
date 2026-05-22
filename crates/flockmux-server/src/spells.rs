//! Spell registry — markdown files with a TOML front-matter declaring the
//! topology of a multi-agent flockmux session.
//!
//! Why TOML front-matter (not YAML, which plan §4.7 shows as an example):
//!   - `toml` is already a workspace dep; adding `serde_yaml` (or its
//!     post-archive forks) for the sake of three-deep config is overkill.
//!   - TOML's `[[agents]]` array-of-tables + triple-quoted string literals
//!     handle our shallow data cleanly with zero new crate surface.
//!   - Front-matter format is implementation detail anyway; plan's YAML
//!     example was illustrative, not a contract.
//!
//! Spell file shape (`spells/<name>.md`):
//!
//! ```markdown
//! +++
//! name = "critic-loop"
//! description = "writer → critic → editor loop"
//!
//! [[agents]]
//! role = "writer"
//! cli = "claude"
//! system_prompt = """
//! You are the writer. Task: {task}
//! Other agents: critic={critic_id}, editor={editor_id}.
//! ...
//! """
//!
//! [[agents]]
//! role = "critic"
//! cli = "codex"
//! system_prompt = """..."""
//! +++
//!
//! # critic-loop spell
//!
//! (free-form markdown body, ignored by the parser — pure docs)
//! ```
//!
//! Placeholders the runner substitutes at execution time:
//!   - `{task}`     — user-supplied task string from the run request
//!   - `{<role>_id}` — agent_id of the agent declared with that role
//!                    (so writer's prompt can reference critic by id)
//!
//! Bad spells are skipped at load time with a `warn!` log — never panic.

use crate::roles::{Role, RoleRegistry};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Parsed front-matter for one spell file.
#[derive(Debug, Clone, Deserialize)]
pub struct SpellManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// When true, every agent in this spell runs in the same workspace
    /// directory (a monorepo-style layout). When false (default), each
    /// agent gets its own `<workspaces_root>/<agent_id>/` directory.
    /// Drives the M6a fullstack-feature pattern where FE/BE/Test share
    /// `apps/frontend`, `apps/backend`, `tests/` under one cwd.
    #[serde(default)]
    pub shared_workspace: bool,
    #[serde(default)]
    pub agents: Vec<SpellAgentManifest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpellAgentManifest {
    /// Symbolic role name within the spell. Used (a) as a key for
    /// `{<role>_id}` substitution in other agents' prompts, and (b)
    /// passed to spawn_agent so the UI pane header shows e.g. "writer".
    ///
    /// Optional ONLY because `role_ref` provides a natural default — if
    /// `role_ref = "frontend"` is given and `role` is omitted, role
    /// implicitly becomes `"frontend"`. Validation ensures every agent
    /// ends up with a role one way or another.
    #[serde(default)]
    pub role: Option<String>,
    /// Which CLI plugin to spawn (must match a `cli-plugins/<id>.toml`).
    /// Optional when `role_ref` is given — the runner falls back to the
    /// referenced role's `default_cli`. Required otherwise.
    #[serde(default)]
    pub cli: Option<String>,
    /// Inline prompt template. Optional when `role_ref` is given (the
    /// runner falls back to the role's `system_prompt_template`); when
    /// both are given, this inline value wins (spell-side override).
    /// `{task}` and `{<role>_id}` are substituted at run time.
    #[serde(default)]
    pub system_prompt: String,
    /// If set, look up this id in the [`RoleRegistry`] at spell-launch
    /// time and use the role's `default_cli` / `system_prompt_template`
    /// to fill in any field not explicitly set here. Lets spells stay
    /// terse (fullstack-feature.md is ~3 `[[agents]]` blocks with just
    /// `role_ref` lines).
    #[serde(default)]
    pub role_ref: Option<String>,
    /// Override the role's `depends_on` declaration. `None` means "use
    /// whatever the referenced role declares (or empty if no role_ref)".
    /// `Some(vec![])` means "explicitly clear deps". `Some(["x"])` means
    /// "replace whatever the role had".
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
}

impl SpellAgentManifest {
    /// The role name an agent ends up with after `role_ref` fallback.
    /// Returns `None` only if neither `role` nor `role_ref` was set,
    /// which `validate_manifest` already rejects.
    pub fn effective_role(&self) -> Option<&str> {
        self.role.as_deref().or(self.role_ref.as_deref())
    }
}

/// A fully-resolved spell agent: every field is a concrete `String`,
/// ready for spawn. Produced by [`resolve_agent`] which merges the
/// spell's inline values with the referenced [`Role`]'s defaults.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub role: String,
    pub cli: String,
    pub system_prompt: String,
    /// Effective list of blackboard keys this agent waits on (spell
    /// override > role default > empty). Drives the M6b WakeCoordinator
    /// subscription table. De-duplicated, order-preserved.
    pub depends_on: Vec<String>,
}

/// Merge a [`SpellAgentManifest`] with its referenced [`Role`] (if any)
/// from the registry. Spell-side values win over role defaults — this
/// matches the principle "the more specific layer overrides the more
/// general one" (spell knows the concrete topology; role only the
/// generic SOP).
///
/// Returns an error if `role_ref` is set but the registry doesn't know
/// it — better to fail loudly at spell-launch than to spawn an agent
/// with an empty prompt and let the user wonder why nothing happened.
pub fn resolve_agent(
    agent: &SpellAgentManifest,
    roles: &RoleRegistry,
) -> Result<ResolvedAgent> {
    let role_template: Option<&Role> = match agent.role_ref.as_deref() {
        Some(id) => {
            let r = roles.get(id).ok_or_else(|| {
                anyhow!("role_ref = \"{id}\" not found in role registry (roles/)")
            })?;
            Some(r)
        }
        None => None,
    };

    let role = agent
        .role
        .clone()
        .or_else(|| agent.role_ref.clone())
        .ok_or_else(|| anyhow!("agent has neither `role` nor `role_ref`"))?;

    let cli = match (&agent.cli, role_template) {
        (Some(c), _) if !c.is_empty() => c.clone(),
        (_, Some(rt)) => rt.manifest.default_cli.clone(),
        _ => return Err(anyhow!("agent `{role}` has no cli and no role_ref to default from")),
    };

    let system_prompt = if !agent.system_prompt.is_empty() {
        agent.system_prompt.clone()
    } else if let Some(rt) = role_template {
        rt.manifest.system_prompt_template.clone()
    } else {
        String::new()
    };

    // depends_on resolution: spell's explicit override wins (even if
    // empty, which is the "I want to clear what the role declared" signal).
    // Otherwise fall through to role's depends_on, or empty.
    let depends_on_raw = match agent.depends_on.as_ref() {
        Some(spell_deps) => spell_deps.clone(),
        None => role_template
            .map(|rt| rt.manifest.depends_on.clone())
            .unwrap_or_default(),
    };
    let depends_on = dedup_preserve_order(depends_on_raw);

    Ok(ResolvedAgent {
        role,
        cli,
        system_prompt,
        depends_on,
    })
}

/// Stable de-dup preserving first occurrence order. Used for depends_on
/// so that listing `["a", "b", "a"]` collapses to `["a", "b"]` without
/// silently reordering.
fn dedup_preserve_order(input: Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(input.len());
    for v in input {
        if seen.insert(v.clone()) {
            out.push(v);
        }
    }
    out
}

/// A loaded spell: parsed manifest + path it came from (for diagnostics)
/// + the raw markdown body (currently unused but kept for future UI hover
/// previews showing the spell's documentation).
#[derive(Debug, Clone)]
pub struct Spell {
    pub manifest: SpellManifest,
    #[allow(dead_code)]
    pub source_path: PathBuf,
    #[allow(dead_code)]
    pub markdown_body: String,
}

#[derive(Debug, Clone, Default)]
pub struct SpellRegistry {
    spells: HashMap<String, Spell>,
}

impl SpellRegistry {
    /// Walk `dir` for `*.md` files. Each file is parsed independently;
    /// failures log a `warn!` and skip the file without aborting the load.
    /// If `dir` doesn't exist we return an empty registry — spells are
    /// optional and a fresh checkout shouldn't fail to start the server.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut spells = HashMap::new();
        if !dir.exists() {
            return Ok(Self { spells });
        }
        let read = std::fs::read_dir(dir)
            .with_context(|| format!("read_dir({})", dir.display()))?;
        for entry in read {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let bytes = match std::fs::read_to_string(&path) {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(?err, path = %path.display(), "skip spell: read failed");
                    continue;
                }
            };
            let spell = match parse_spell(&bytes, &path) {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(?err, path = %path.display(), "skip spell: parse failed");
                    continue;
                }
            };
            if spells.contains_key(&spell.manifest.name) {
                tracing::warn!(
                    name = %spell.manifest.name,
                    path = %path.display(),
                    "skip spell: duplicate name (first one wins)",
                );
                continue;
            }
            spells.insert(spell.manifest.name.clone(), spell);
        }
        Ok(Self { spells })
    }

    pub fn get(&self, name: &str) -> Option<&Spell> {
        self.spells.get(name)
    }

    pub fn list(&self) -> Vec<&Spell> {
        let mut v: Vec<_> = self.spells.values().collect();
        v.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        v
    }
}

/// Locate the `spells/` directory: env override > workspace-relative.
/// Mirrors `plugins::default_plugins_dir` so spells and plugins live
/// side-by-side under the repo root.
pub fn default_spells_dir() -> PathBuf {
    if let Ok(p) = std::env::var("FLOCKMUX_SPELLS_DIR") {
        return PathBuf::from(p);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(ws) = manifest.parent().and_then(|p| p.parent()) {
        let candidate = ws.join("spells");
        if candidate.is_dir() {
            return candidate;
        }
    }
    PathBuf::from("spells")
}

/// Split a spell file into TOML front-matter + body, parse the front-matter,
/// return the assembled `Spell`. Format: text between the first pair of
/// `+++` lines (each on its own line) is the front-matter; everything after
/// the second `+++` is the markdown body.
fn parse_spell(content: &str, source_path: &Path) -> Result<Spell> {
    let (front_matter, body) = split_front_matter(content)
        .ok_or_else(|| anyhow!("no `+++` front-matter delimiters found"))?;
    let manifest: SpellManifest = toml::from_str(front_matter)
        .with_context(|| "parse front-matter as TOML")?;
    validate_manifest(&manifest)?;
    Ok(Spell {
        manifest,
        source_path: source_path.to_path_buf(),
        markdown_body: body.to_string(),
    })
}

/// Returns (front_matter, body) if the content begins with a `+++` line
/// and contains a closing `+++` line. Whitespace before the delimiter is
/// tolerated (BOM, leading blank line, etc.).
fn split_front_matter(content: &str) -> Option<(&str, &str)> {
    let trimmed_start = content.trim_start_matches(['\u{FEFF}', '\n', '\r', ' ', '\t']);
    let offset = content.len() - trimmed_start.len();
    if !trimmed_start.starts_with("+++") {
        return None;
    }
    // Skip the opening fence line.
    let after_open = &trimmed_start["+++".len()..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let close_idx = after_open.find("\n+++")?;
    let fm = &after_open[..close_idx];
    let body_start = close_idx + "\n+++".len();
    let body = &after_open[body_start..];
    // Strip a leading newline on body so it starts at the first markdown
    // character, not a blank.
    let body = body.strip_prefix('\n').unwrap_or(body);
    // Untrim for return — the caller doesn't care about the leading offset,
    // but the inferred slice should still be valid pointers into `content`.
    // Rust slice ops on the trimmed input remain valid since they're still
    // subslices of `content`.
    let _ = offset; // suppress unused warning when not debugging
    Some((fm, body))
}

fn validate_manifest(m: &SpellManifest) -> Result<()> {
    if m.name.is_empty() {
        return Err(anyhow!("manifest `name` must be non-empty"));
    }
    if m.agents.is_empty() {
        return Err(anyhow!("manifest must declare at least one [[agents]]"));
    }
    for a in &m.agents {
        // Either an explicit role or a role_ref to default from. Without
        // one of these we have no name for the agent and no key for the
        // `{<role>_id}` substitution.
        if a.effective_role().is_none() {
            return Err(anyhow!(
                "every [[agents]] needs a `role` or a `role_ref`"
            ));
        }
        // If there's no role_ref, the spell must inline cli + a non-
        // empty system_prompt — nothing can fall back from. With a
        // role_ref, both are optional (the role provides defaults).
        if a.role_ref.is_none() {
            let cli_empty = a.cli.as_deref().map(|c| c.is_empty()).unwrap_or(true);
            if cli_empty {
                return Err(anyhow!(
                    "agent `{}` has no `cli` and no `role_ref` to default from",
                    a.effective_role().unwrap_or("?")
                ));
            }
        }
    }
    // Roles must be unique within a spell — the {<role>_id} substitution
    // would otherwise be ambiguous.
    let mut seen = std::collections::HashSet::new();
    for a in &m.agents {
        // Safe: we just validated effective_role().is_some() above.
        let role = a.effective_role().unwrap();
        if !seen.insert(role.to_string()) {
            return Err(anyhow!("duplicate role `{role}` in spell"));
        }
    }
    Ok(())
}

/// Substitute `{task}` and `{<role>_id}` placeholders in a system prompt.
/// Unknown placeholders are left literal so we don't silently drop content
/// the spell author cared about — bad data is more recoverable than missing.
pub fn render_prompt(prompt: &str, task: &str, role_to_id: &HashMap<String, String>) -> String {
    let mut out = prompt.replace("{task}", task);
    for (role, id) in role_to_id {
        let needle = format!("{{{}_id}}", role);
        out = out.replace(&needle, id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn split_front_matter_extracts_toml_and_body() {
        let src = "+++\nname = \"x\"\n+++\n# body line\n";
        let (fm, body) = split_front_matter(src).unwrap();
        assert!(fm.contains("name = \"x\""));
        assert_eq!(body, "# body line\n");
    }

    #[test]
    fn split_front_matter_tolerates_leading_blanks_and_bom() {
        let src = "\u{FEFF}\n  \n+++\na=1\n+++\nrest\n";
        let (fm, body) = split_front_matter(src).unwrap();
        assert_eq!(fm.trim(), "a=1");
        assert_eq!(body, "rest\n");
    }

    #[test]
    fn split_front_matter_returns_none_when_no_fence() {
        assert!(split_front_matter("# just markdown\nno fence").is_none());
        assert!(split_front_matter("+++\nopen but never closed").is_none());
    }

    #[test]
    fn parse_spell_minimal() {
        let src = r#"+++
name = "demo"
description = "demo spell"

[[agents]]
role = "writer"
cli = "claude"
system_prompt = "hello"
+++

# notes here
"#;
        let s = parse_spell(src, Path::new("/tmp/demo.md")).unwrap();
        assert_eq!(s.manifest.name, "demo");
        assert_eq!(s.manifest.agents.len(), 1);
        assert_eq!(s.manifest.agents[0].effective_role(), Some("writer"));
        assert!(s.markdown_body.contains("# notes here"));
    }

    #[test]
    fn parse_spell_with_role_ref_inferred_role() {
        let src = r#"+++
name = "fullstack"
[[agents]]
role_ref = "frontend"
[[agents]]
role_ref = "backend"
+++
"#;
        let s = parse_spell(src, Path::new("/tmp/fullstack.md")).unwrap();
        assert_eq!(s.manifest.agents.len(), 2);
        // role omitted → role_ref provides the implicit name
        assert_eq!(s.manifest.agents[0].effective_role(), Some("frontend"));
        assert_eq!(s.manifest.agents[1].effective_role(), Some("backend"));
        assert!(s.manifest.agents[0].cli.is_none());
        assert!(s.manifest.agents[0].system_prompt.is_empty());
    }

    #[test]
    fn parse_spell_rejects_agent_without_role_or_role_ref() {
        let src = r#"+++
name = "broken"
[[agents]]
cli = "claude"
+++"#;
        let err = parse_spell(src, Path::new("/tmp/x.md")).unwrap_err();
        assert!(
            format!("{err:#}").contains("`role` or a `role_ref`"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_spell_rejects_agent_without_cli_or_role_ref() {
        let src = r#"+++
name = "broken"
[[agents]]
role = "writer"
+++"#;
        let err = parse_spell(src, Path::new("/tmp/x.md")).unwrap_err();
        assert!(
            format!("{err:#}").contains("no `cli`"),
            "got: {err:#}"
        );
    }

    #[test]
    fn shared_workspace_defaults_false() {
        let src = r#"+++
name = "x"
[[agents]]
role = "r"
cli = "claude"
+++"#;
        let s = parse_spell(src, Path::new("/tmp/x.md")).unwrap();
        assert!(!s.manifest.shared_workspace);
    }

    #[test]
    fn shared_workspace_set_true_parses() {
        let src = r#"+++
name = "x"
shared_workspace = true
[[agents]]
role = "r"
cli = "claude"
+++"#;
        let s = parse_spell(src, Path::new("/tmp/x.md")).unwrap();
        assert!(s.manifest.shared_workspace);
    }

    #[test]
    fn resolve_agent_fills_cli_and_prompt_from_role() {
        // Set up a registry with one role.
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("frontend.md"),
            "+++\nid=\"frontend\"\ndefault_cli=\"claude\"\nsystem_prompt_template=\"You are FE: {task}.\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();

        // Spell agent with only role_ref — should fill cli + prompt from role.
        let agent = SpellAgentManifest {
            role: None,
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("frontend".to_string()),
            depends_on: None,
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(resolved.role, "frontend");
        assert_eq!(resolved.cli, "claude");
        assert!(resolved.system_prompt.contains("You are FE"));
        assert!(resolved.depends_on.is_empty());
    }

    #[test]
    fn resolve_agent_inline_overrides_role_defaults() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("backend.md"),
            "+++\nid=\"backend\"\ndefault_cli=\"codex\"\nsystem_prompt_template=\"role default\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();

        // Spell agent override: spell says use claude + custom prompt.
        let agent = SpellAgentManifest {
            role: None,
            cli: Some("claude".to_string()),
            system_prompt: "spell override".to_string(),
            role_ref: Some("backend".to_string()),
            depends_on: None,
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(resolved.cli, "claude"); // spell wins
        assert_eq!(resolved.system_prompt, "spell override"); // spell wins
    }

    #[test]
    fn resolve_agent_errors_on_unknown_role_ref() {
        let roles = RoleRegistry::default();
        let agent = SpellAgentManifest {
            role: None,
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("nonexistent".to_string()),
            depends_on: None,
        };
        let err = resolve_agent(&agent, &roles).unwrap_err();
        assert!(format!("{err:#}").contains("nonexistent"));
    }

    #[test]
    fn resolve_agent_works_without_role_ref() {
        // Old-style spell (e.g. critic-loop) with everything inline.
        let roles = RoleRegistry::default();
        let agent = SpellAgentManifest {
            role: Some("writer".to_string()),
            cli: Some("claude".to_string()),
            system_prompt: "hello".to_string(),
            role_ref: None,
            depends_on: None,
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(resolved.role, "writer");
        assert_eq!(resolved.cli, "claude");
        assert_eq!(resolved.system_prompt, "hello");
        assert!(resolved.depends_on.is_empty());
    }

    #[test]
    fn resolve_agent_inherits_depends_on_from_role() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.md"),
            "+++\nid=\"test\"\ndefault_cli=\"claude\"\ndepends_on=[\"frontend.done\",\"backend.done\"]\nsystem_prompt_template=\"x\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();
        let agent = SpellAgentManifest {
            role: None,
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("test".to_string()),
            depends_on: None,
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(
            resolved.depends_on,
            vec!["frontend.done".to_string(), "backend.done".to_string()]
        );
    }

    #[test]
    fn resolve_agent_spell_overrides_depends_on() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.md"),
            "+++\nid=\"test\"\ndefault_cli=\"claude\"\ndepends_on=[\"a\"]\nsystem_prompt_template=\"x\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();
        let agent = SpellAgentManifest {
            role: None,
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("test".to_string()),
            depends_on: Some(vec!["b".to_string(), "c".to_string()]),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(resolved.depends_on, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn resolve_agent_spell_explicit_empty_clears_role_depends_on() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.md"),
            "+++\nid=\"test\"\ndefault_cli=\"claude\"\ndepends_on=[\"a\"]\nsystem_prompt_template=\"x\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();
        let agent = SpellAgentManifest {
            role: None,
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("test".to_string()),
            depends_on: Some(vec![]),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert!(resolved.depends_on.is_empty());
    }

    #[test]
    fn resolve_agent_dedups_depends_on() {
        let roles = RoleRegistry::default();
        let agent = SpellAgentManifest {
            role: Some("r".into()),
            cli: Some("claude".into()),
            system_prompt: "x".into(),
            role_ref: None,
            depends_on: Some(vec!["a".into(), "b".into(), "a".into()]),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(resolved.depends_on, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn parse_spell_rejects_duplicate_roles() {
        let src = r#"+++
name = "dup"
[[agents]]
role = "x"
cli = "claude"
[[agents]]
role = "x"
cli = "codex"
+++
"#;
        let err = parse_spell(src, Path::new("/tmp/x.md")).unwrap_err();
        assert!(format!("{err:#}").contains("duplicate role"), "got: {err:#}");
    }

    #[test]
    fn parse_spell_rejects_empty_agents() {
        let src = r#"+++
name = "empty"
+++"#;
        let err = parse_spell(src, Path::new("/tmp/empty.md")).unwrap_err();
        assert!(
            format!("{err:#}").contains("at least one [[agents]]"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_spell_rejects_missing_name() {
        let src = r#"+++
[[agents]]
role = "x"
cli = "claude"
+++"#;
        // toml::from_str fails to deserialize because `name` is required
        let err = parse_spell(src, Path::new("/tmp/x.md")).unwrap_err();
        assert!(format!("{err:#}").to_lowercase().contains("name"));
    }

    #[test]
    fn registry_loads_only_md_files_and_skips_bad_ones() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "good.md",
            r#"+++
name = "good"
[[agents]]
role = "r"
cli = "claude"
+++
"#,
        );
        write(dir.path(), "bad.md", "this has no front matter at all");
        write(dir.path(), "ignored.txt", "+++\nname = \"x\"\n+++");

        let reg = SpellRegistry::load_dir(dir.path()).unwrap();
        let names: Vec<_> = reg.list().iter().map(|s| s.manifest.name.clone()).collect();
        assert_eq!(names, vec!["good".to_string()]);
    }

    #[test]
    fn registry_load_dir_missing_returns_empty() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("does-not-exist");
        let reg = SpellRegistry::load_dir(&nonexistent).unwrap();
        assert_eq!(reg.list().len(), 0);
    }

    #[test]
    fn registry_deduplicates_by_name() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "a.md",
            "+++\nname=\"same\"\n[[agents]]\nrole=\"r\"\ncli=\"claude\"\n+++",
        );
        write(
            dir.path(),
            "b.md",
            "+++\nname=\"same\"\n[[agents]]\nrole=\"r\"\ncli=\"codex\"\n+++",
        );
        let reg = SpellRegistry::load_dir(dir.path()).unwrap();
        assert_eq!(reg.list().len(), 1, "duplicate name should be deduped");
    }

    #[test]
    fn render_prompt_substitutes_task_and_role_ids() {
        let mut map = HashMap::new();
        map.insert("writer".to_string(), "claude-aaa".to_string());
        map.insert("critic".to_string(), "codex-bbb".to_string());
        let rendered = render_prompt(
            "Task: {task}. Writer is {writer_id}, critic is {critic_id}.",
            "build a parser",
            &map,
        );
        assert_eq!(
            rendered,
            "Task: build a parser. Writer is claude-aaa, critic is codex-bbb."
        );
    }

    #[test]
    fn render_prompt_leaves_unknown_placeholders_literal() {
        let map = HashMap::new();
        let out = render_prompt("ref {unknown_id} here", "t", &map);
        // We deliberately don't strip unknown {…_id} substrings — silent
        // dropping would hide spell author bugs.
        assert!(out.contains("{unknown_id}"));
    }
}
