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
    #[serde(default)]
    pub agents: Vec<SpellAgentManifest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpellAgentManifest {
    /// Symbolic role name. Used (a) as a key for `{<role>_id}` substitution
    /// in other agents' prompts, and (b) passed to spawn_agent so the UI
    /// shows e.g. "writer" instead of "claude" in the pane header.
    pub role: String,
    /// Which CLI plugin to spawn (must match a `cli-plugins/<id>.toml`).
    pub cli: String,
    /// Free-form prompt injected into the agent's PTY immediately after
    /// shim_ready. `{task}` and `{<role>_id}` are substituted before
    /// injection. Empty string = no auto-bootstrap (the spell just spawns
    /// the agent and leaves it to the user).
    #[serde(default)]
    pub system_prompt: String,
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
        if a.role.is_empty() {
            return Err(anyhow!("every [[agents]] needs a non-empty `role`"));
        }
        if a.cli.is_empty() {
            return Err(anyhow!("every [[agents]] needs a non-empty `cli`"));
        }
    }
    // Roles must be unique within a spell — the {<role>_id} substitution
    // would otherwise be ambiguous.
    let mut seen = std::collections::HashSet::new();
    for a in &m.agents {
        if !seen.insert(a.role.as_str()) {
            return Err(anyhow!("duplicate role `{}` in spell", a.role));
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
        assert_eq!(s.manifest.agents[0].role, "writer");
        assert!(s.markdown_body.contains("# notes here"));
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
