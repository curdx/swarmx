//! Role registry — reusable per-agent SOP templates loaded from `roles/`.
//!
//! A *role* is the spell-author-facing equivalent of MetaGPT's pinned
//! "PM/Architect/Engineer/QA" team slots: a markdown file under `roles/`
//! captures the default CLI to spawn, a system_prompt template, a handoff
//! signal (which blackboard key the agent writes when done), and a soft
//! artifact_paths convention so the agent knows which part of a shared
//! workspace it owns.
//!
//! Spells reference roles via `role_ref = "frontend"` on a `[[agents]]`
//! entry instead of inlining the prompt verbatim. The runner resolves
//! the ref at spell-launch time, filling in any field the spell didn't
//! override.
//!
//! File shape mirrors `spells/<name>.md`:
//!
//! ```markdown
//! +++
//! id = "frontend"
//! name = "Frontend Engineer"
//! description = "..."
//! default_cli = "claude"
//! artifact_paths = ["apps/frontend/**"]
//! handoff_signal = "frontend.done"
//! system_prompt_template = """ ... """
//! +++
//!
//! # role docs
//! (free-form body, ignored by parser)
//! ```
//!
//! Bad files are skipped with a `warn!` at load time — never panic.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Parsed front-matter for one role file.
#[derive(Debug, Clone, Deserialize)]
pub struct RoleManifest {
    /// Machine-readable identifier. Used as the lookup key when a spell
    /// references this role via `role_ref = "<id>"`.
    pub id: String,
    /// Human-readable display name (for UI / logs).
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Which CLI plugin a spell should default to for this role, unless
    /// the spell overrides it explicitly.
    pub default_cli: String,
    /// Soft convention listing the workspace paths this role is allowed
    /// to write. M6a does NOT enforce this — it's repeated inside the
    /// prompt so the LLM honours it. M6b may add a runtime sandbox.
    #[serde(default)]
    pub artifact_paths: Vec<String>,
    /// Blackboard key this role writes when its phase completes.
    /// Recorded here so future tooling (DAG viewer, planner) can
    /// statically reason about role handoffs.
    #[serde(default)]
    pub handoff_signal: String,
    /// Blackboard keys this role is waiting on before it can do real
    /// work. Consumed by the M6b WakeCoordinator: when one of these keys
    /// is written, any agent playing this role gets a mailbox note + a
    /// PTY kick to start a fresh turn. Defaults to empty for roles with
    /// no upstream (frontend, backend).
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Template string used as the agent's system_prompt unless the
    /// spell explicitly overrides it. Supports the same `{task}` and
    /// `{<role>_id}` placeholders as a spell's inline system_prompt
    /// (rendered by `spells::render_prompt`).
    pub system_prompt_template: String,
}

#[derive(Debug, Clone)]
pub struct Role {
    pub manifest: RoleManifest,
    #[allow(dead_code)]
    pub source_path: PathBuf,
    #[allow(dead_code)]
    pub markdown_body: String,
}

#[derive(Debug, Clone, Default)]
pub struct RoleRegistry {
    roles: HashMap<String, Role>,
}

impl RoleRegistry {
    /// Walk `dir` for `*.md` files. Each file is parsed independently;
    /// failures log a `warn!` and skip the file without aborting. If
    /// `dir` doesn't exist we return an empty registry — roles are
    /// optional and a fresh checkout shouldn't fail to start the server.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut roles = HashMap::new();
        if !dir.exists() {
            return Ok(Self { roles });
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
                    tracing::warn!(?err, path = %path.display(), "skip role: read failed");
                    continue;
                }
            };
            let role = match parse_role(&bytes, &path) {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!(?err, path = %path.display(), "skip role: parse failed");
                    continue;
                }
            };
            if roles.contains_key(&role.manifest.id) {
                tracing::warn!(
                    id = %role.manifest.id,
                    path = %path.display(),
                    "skip role: duplicate id (first one wins)",
                );
                continue;
            }
            roles.insert(role.manifest.id.clone(), role);
        }
        Ok(Self { roles })
    }

    pub fn get(&self, id: &str) -> Option<&Role> {
        self.roles.get(id)
    }

    #[allow(dead_code)]
    pub fn list(&self) -> Vec<&Role> {
        let mut v: Vec<_> = self.roles.values().collect();
        v.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        v
    }
}

/// Locate the `roles/` directory: env override > workspace-relative.
/// Mirrors `spells::default_spells_dir` so roles and spells live side-
/// by-side under the repo root.
pub fn default_roles_dir() -> PathBuf {
    if let Ok(p) = std::env::var("FLOCKMUX_ROLES_DIR") {
        return PathBuf::from(p);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(ws) = manifest.parent().and_then(|p| p.parent()) {
        let candidate = ws.join("roles");
        if candidate.is_dir() {
            return candidate;
        }
    }
    PathBuf::from("roles")
}

fn parse_role(content: &str, source_path: &Path) -> Result<Role> {
    // Reuse the same front-matter convention as spells (same `+++`
    // fences). Inlining the split here instead of importing from
    // spells.rs keeps the two modules decoupled — they could diverge
    // later (e.g. roles want YAML support) without rippling.
    let (front_matter, body) = split_front_matter(content)
        .ok_or_else(|| anyhow!("no `+++` front-matter delimiters found"))?;
    let manifest: RoleManifest = toml::from_str(front_matter)
        .with_context(|| "parse role front-matter as TOML")?;
    validate_manifest(&manifest)?;
    Ok(Role {
        manifest,
        source_path: source_path.to_path_buf(),
        markdown_body: body.to_string(),
    })
}

fn split_front_matter(content: &str) -> Option<(&str, &str)> {
    let trimmed_start = content.trim_start_matches(['\u{FEFF}', '\n', '\r', ' ', '\t']);
    if !trimmed_start.starts_with("+++") {
        return None;
    }
    let after_open = &trimmed_start["+++".len()..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let close_idx = after_open.find("\n+++")?;
    let fm = &after_open[..close_idx];
    let body_start = close_idx + "\n+++".len();
    let body = &after_open[body_start..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    Some((fm, body))
}

fn validate_manifest(m: &RoleManifest) -> Result<()> {
    if m.id.is_empty() {
        return Err(anyhow!("role manifest `id` must be non-empty"));
    }
    if m.default_cli.is_empty() {
        return Err(anyhow!("role manifest `default_cli` must be non-empty"));
    }
    if m.system_prompt_template.is_empty() {
        return Err(anyhow!(
            "role manifest `system_prompt_template` must be non-empty"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn parse_role_minimal() {
        let src = r#"+++
id = "frontend"
name = "Frontend Engineer"
default_cli = "claude"
system_prompt_template = "You are FE. Task: {task}."
+++

# docs body
"#;
        let r = parse_role(src, Path::new("/tmp/frontend.md")).unwrap();
        assert_eq!(r.manifest.id, "frontend");
        assert_eq!(r.manifest.default_cli, "claude");
        assert!(r.manifest.system_prompt_template.contains("{task}"));
        assert!(r.markdown_body.contains("# docs body"));
        assert!(r.manifest.depends_on.is_empty(), "default empty");
    }

    #[test]
    fn parse_role_with_depends_on() {
        let src = r#"+++
id = "test"
default_cli = "claude"
depends_on = ["frontend.done", "backend.done"]
system_prompt_template = "you are test"
+++"#;
        let r = parse_role(src, Path::new("/tmp/test.md")).unwrap();
        assert_eq!(
            r.manifest.depends_on,
            vec!["frontend.done".to_string(), "backend.done".to_string()]
        );
    }

    #[test]
    fn parse_role_rejects_missing_id() {
        let src = r#"+++
default_cli = "claude"
system_prompt_template = "x"
+++"#;
        let err = parse_role(src, Path::new("/tmp/x.md")).unwrap_err();
        // toml deserialize fails because `id` is required (no #[serde(default)])
        assert!(format!("{err:#}").to_lowercase().contains("id"));
    }

    #[test]
    fn parse_role_rejects_missing_default_cli() {
        let src = r#"+++
id = "x"
system_prompt_template = "x"
+++"#;
        let err = parse_role(src, Path::new("/tmp/x.md")).unwrap_err();
        assert!(format!("{err:#}").to_lowercase().contains("default_cli"));
    }

    #[test]
    fn parse_role_rejects_empty_template() {
        let src = r#"+++
id = "x"
default_cli = "claude"
system_prompt_template = ""
+++"#;
        let err = parse_role(src, Path::new("/tmp/x.md")).unwrap_err();
        assert!(
            format!("{err:#}").contains("system_prompt_template"),
            "got: {err:#}"
        );
    }

    #[test]
    fn registry_loads_only_md_files_and_skips_bad_ones() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "good.md",
            r#"+++
id = "good"
default_cli = "claude"
system_prompt_template = "ok"
+++
"#,
        );
        write(dir.path(), "bad.md", "no front matter at all");
        write(
            dir.path(),
            "ignored.txt",
            "+++\nid=\"x\"\ndefault_cli=\"y\"\nsystem_prompt_template=\"z\"\n+++",
        );

        let reg = RoleRegistry::load_dir(dir.path()).unwrap();
        let ids: Vec<_> = reg.list().iter().map(|r| r.manifest.id.clone()).collect();
        assert_eq!(ids, vec!["good".to_string()]);
    }

    #[test]
    fn registry_load_dir_missing_returns_empty() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("does-not-exist");
        let reg = RoleRegistry::load_dir(&nonexistent).unwrap();
        assert_eq!(reg.list().len(), 0);
    }

    #[test]
    fn registry_deduplicates_by_id() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "a.md",
            "+++\nid=\"same\"\ndefault_cli=\"claude\"\nsystem_prompt_template=\"a\"\n+++",
        );
        write(
            dir.path(),
            "b.md",
            "+++\nid=\"same\"\ndefault_cli=\"codex\"\nsystem_prompt_template=\"b\"\n+++",
        );
        let reg = RoleRegistry::load_dir(dir.path()).unwrap();
        assert_eq!(reg.list().len(), 1, "duplicate id should be deduped");
    }

    #[test]
    fn registry_get_by_id() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "frontend.md",
            "+++\nid=\"frontend\"\ndefault_cli=\"claude\"\nsystem_prompt_template=\"x\"\n+++",
        );
        let reg = RoleRegistry::load_dir(dir.path()).unwrap();
        assert!(reg.get("frontend").is_some());
        assert!(reg.get("backend").is_none());
    }
}
