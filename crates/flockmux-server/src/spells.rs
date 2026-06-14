//! Spell registry — markdown files with a TOML front-matter declaring the
//! topology of a multi-agent flockmux session.
//!
//! STATUS (decision, 2026-05): this system is **deliberately minimal, not
//! dead**. The full feature set — `role_ref` merge, `system_prompt_prefix`
//! (HITL gate), `allow_cycles`, `{<role>_id}` cross-refs, shared-workspace
//! layout, multi-agent spawn — is implemented and unit-tested, but ONLY the
//! single `init` spell (one orchestrator) is driven in production, at
//! workspace creation (see `routes::rest::run_spell` ← `main.rs`). Everything
//! downstream is dispatched ad-hoc by the orchestrator via
//! `swarm_spawn_worker` (Magentic-One model: decide the team per task, don't
//! pre-declare a fixed topology), so the `swarm_run_spell` MCP tool was
//! removed. Do NOT (a) delete this as "dead code" — `init` depends on it and
//! the machinery is tested — nor (b) pad it with speculative spells. If a real
//! multi-agent template need appears, add one spell + reuse this runner.
//! (README's "M6c auto-dispatch / swarm_run_spell" sections predate this pivot
//! and are historical.)
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
    /// M6d-3: opt-in escape hatch for intentional `depends_on` cycles.
    /// The default-on cycle detector at `run_spell` blocks any role-to-
    /// role dependency loop because in the typical fullstack-feature
    /// shape a cycle means deadlock. But `fullstack-feature-strict`
    /// needs critic↔fixer to depend on each other's outputs in
    /// alternation, bounded by a round counter in the prompts. Setting
    /// this flag tells the cycle detector "I know what I'm doing — the
    /// loop is bounded elsewhere (prompt-level round cap)."
    #[serde(default)]
    pub allow_cycles: bool,
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
    /// Text prepended to the resolved `system_prompt` (after role-template
    /// vs spell-override resolution; before placeholder substitution). The
    /// canonical use case is M6c-7 HITL gating: the spell prepends a short
    /// "before doing anything else, idle until <key> exists" instruction
    /// to the role's normal SOP, so the agent's FIRST turn (driven by the
    /// initial bootstrap inject, NOT by wake-check) actually checks the
    /// gate. Without this, `depends_on` only affects post-Stop wakes — it
    /// does NOT suppress the initial bootstrap, so an agent would code
    /// happily through an "approval gate" because nobody told it to wait.
    /// Empty = no prefix.
    #[serde(default)]
    pub system_prompt_prefix: String,
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
    /// Blackboard key this agent PRODUCES on completion — the `handoff_signal`
    /// of the referenced role, or empty for a truly inline agent whose symbolic
    /// `role` names no registered role. Resolved HERE (where the `role_ref` →
    /// `Role` mapping is unambiguous) rather than re-looked-up by `role` name in
    /// `run_spell`: a spell that renames a role (`role = "fe"`, `role_ref =
    /// "frontend"`) would make a `roles.get("fe")` miss, dropping this agent's
    /// produced key from the cycle graph and the wake exit-key registration — a
    /// blind spot where a role↔role loop through the renamed producer goes
    /// undetected. Carrying the value off the resolved template closes that gap.
    pub handoff_signal: String,
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
pub fn resolve_agent(agent: &SpellAgentManifest, roles: &RoleRegistry) -> Result<ResolvedAgent> {
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
        _ => {
            return Err(anyhow!(
                "agent `{role}` has no cli and no role_ref to default from"
            ))
        }
    };

    let body = if !agent.system_prompt.is_empty() {
        agent.system_prompt.clone()
    } else if let Some(rt) = role_template {
        rt.manifest.system_prompt_template.clone()
    } else {
        String::new()
    };
    // Prepend the spell-level prefix (if any). Separated from the body
    // by a blank line so it renders as its own paragraph at the top of
    // the prompt. Empty prefix → just the body verbatim.
    let system_prompt = if agent.system_prompt_prefix.is_empty() {
        body
    } else if body.is_empty() {
        agent.system_prompt_prefix.clone()
    } else {
        format!("{}\n\n{}", agent.system_prompt_prefix, body)
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

    // Producer key, taken off the role template by `role_ref` (the unambiguous
    // mapping) — NOT re-derived from the symbolic `role` name later. Inline-only
    // agents (no role_ref) declare no handoff_signal and so produce nothing.
    let handoff_signal = role_template
        .map(|rt| rt.manifest.handoff_signal.clone())
        .unwrap_or_default();

    Ok(ResolvedAgent {
        role,
        cli,
        system_prompt,
        depends_on,
        handoff_signal,
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
        let read =
            std::fs::read_dir(dir).with_context(|| format!("read_dir({})", dir.display()))?;
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

    /// Compiled-in spell catalog — the production `init` spell baked into the
    /// binary via `include_str!`, so a packaged app whose CWD has no `spells/`
    /// dir (and whose CARGO_MANIFEST_DIR points at a vanished build path) still
    /// drives workspace creation / orchestrator spawn / auto-respawn. Without
    /// this, the registry was EMPTY on a user machine and "新建空间" failed with
    /// spell-not-found. A malformed embed `warn!` + skips. Mirrors
    /// `roles::RoleRegistry::builtin()`; the on-disk `spells/` dir overlays
    /// this base (dev override by name).
    pub fn builtin() -> Self {
        const BUILTIN: &[(&str, &str)] = &[("init.md", include_str!("../../../spells/init.md"))];
        let mut spells = HashMap::new();
        for (name, content) in BUILTIN {
            match parse_spell(content, Path::new(name)) {
                Ok(spell) => {
                    spells.insert(spell.manifest.name.clone(), spell);
                }
                Err(err) => {
                    tracing::warn!(?err, spell = name, "skip builtin spell: parse failed");
                }
            }
        }
        Self { spells }
    }

    /// Overlay `other`'s spells onto self, overriding by name (other wins).
    /// Used to layer: built-ins → repo `spells/` dir.
    pub fn overlay(&mut self, other: SpellRegistry) {
        for (name, spell) in other.spells {
            self.spells.insert(name, spell);
        }
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
    let manifest: SpellManifest =
        toml::from_str(front_matter).with_context(|| "parse front-matter as TOML")?;
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
///
/// F21: the closing fence is the first line that is EXACTLY `+++` (trailing
/// whitespace / CR tolerated), not merely the first `\n+++` substring. A spell
/// whose `system_prompt = """..."""` value embeds a diff line such as
/// `+++ b/path` (very common — prompts quote diffs/code) used to be truncated
/// there, breaking TOML parse and getting the whole spell silently warn-skipped.
/// A diff line has content after `+++`, so requiring a standalone fence line
/// fixes it. The one remaining constraint: don't put a BARE `+++` line on its
/// own inside a TOML value.
fn split_front_matter(content: &str) -> Option<(&str, &str)> {
    let trimmed_start = content.trim_start_matches(['\u{FEFF}', '\n', '\r', ' ', '\t']);
    if !trimmed_start.starts_with("+++") {
        return None;
    }
    // Skip the opening fence line.
    let after_open = &trimmed_start["+++".len()..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);

    // Scan line by line for a standalone `+++` fence. `line_start` is the byte
    // offset of the current line within `after_open`; we only accept a fence at
    // line_start > 0 (a preceding newline must exist, matching the old
    // `\n+++` requirement — empty front matter stays unsupported).
    let mut line_start = 0usize;
    loop {
        let rel_nl = after_open[line_start..].find('\n');
        let line_end = rel_nl.map(|i| line_start + i).unwrap_or(after_open.len());
        let line = after_open[line_start..line_end].trim_end_matches(['\r', ' ', '\t']);
        if line_start > 0 && line == "+++" {
            // fm excludes the newline immediately before this fence line.
            let fm = &after_open[..line_start - 1];
            // body starts after the fence line's terminating newline (if any).
            let body = match rel_nl {
                Some(i) => &after_open[line_start + i + 1..],
                None => "", // fence is the last line, no body
            };
            return Some((fm, body));
        }
        match rel_nl {
            Some(i) => line_start = line_start + i + 1,
            None => return None, // ran out of lines without a closing fence
        }
    }
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
            return Err(anyhow!("every [[agents]] needs a `role` or a `role_ref`"));
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

/// Substitute `{task}`, `{workspace_id}`, `{thread_slug}` and `{<role>_id}`
/// placeholders in a system prompt. Unknown placeholders are left literal so we
/// don't silently drop content the spell author cared about — bad data is more
/// recoverable than missing.
///
/// `{thread_slug}` is the per-direction blackboard segment (`main` for the main
/// thread). Role templates key their ledgers/signals as
/// `{workspace_id}/{thread_slug}/…` so two directions in one workspace don't
/// clobber each other; the SAME substitution is applied to `depends_on` and
/// `handoff_signal` keys (F2) so cross-agent wake matching stays exact-string.
pub fn render_prompt(
    prompt: &str,
    task: &str,
    workspace_id: &str,
    thread_slug: &str,
    role_to_id: &HashMap<String, String>,
) -> String {
    let mut out = prompt.replace("{task}", task);
    out = out.replace("{workspace_id}", workspace_id);
    out = out.replace("{thread_slug}", thread_slug);
    for (role, id) in role_to_id {
        let needle = format!("{{{}_id}}", role);
        out = out.replace(&needle, id);
    }
    out
}

/// Strip terminal-control / ANSI-escape bytes from text about to be PTY-injected
/// as a "paste" into an agent's TUI.
///
/// SECURITY (prompt-injection + terminal-escape injection): a bootstrap prompt is
/// machine-rendered from spell manifests, role templates and — for ad-hoc workers
/// — the orchestrator-supplied `system_prompt`. None of that is guaranteed free of
/// ESC / CSI / OSC / DCS sequences or other control bytes. If such bytes reach the
/// PTY verbatim they are interpreted by (a) the child CLI's TUI and (b) the user's
/// terminal renderer, enabling cursor/clear/title manipulation, OSC tricks, and —
/// most dangerously — *invisible* prompt injection: escape sequences that hide or
/// overwrite on-screen text so a human reviewing the pane cannot see what the model
/// was actually told. Call this on every prompt before it is written to a PTY.
///
/// We drop every C0 control char (0x00–0x1F) and DEL (0x7F) and the 8-bit C1 range
/// (U+0080–U+009F), with two deliberate exceptions that the paste protocol relies
/// on and that carry no escape semantics:
///   - `\n` (line feed) — prompt bodies are multi-line; the paste→settle→submit
///     split depends on newlines, and the standalone submit `\r` is sent separately.
///   - `\t` (tab) — benign indentation whitespace.
/// In particular `\r` (carriage return) IS stripped from the body: a stray `\r`
/// would prematurely submit the paste, and it has no place inside a prompt body.
/// Dropping the lone ESC defuses any sequence (residual ASCII such as `[2J` renders
/// as inert literal text rather than firing); we never delete spans of author-
/// intended visible text. All printable Unicode (incl. CJK) is preserved verbatim.
pub fn sanitize_pty_inject(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            c == '\n' || c == '\t' || !(c.is_control() || ('\u{80}'..='\u{9f}').contains(&c))
        })
        .collect()
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
    fn split_front_matter_ignores_diff_lines_in_triple_quoted_value() {
        // F21: a `+++ b/path` diff line inside a """...""" system_prompt must
        // NOT be mistaken for the closing fence (it has content after `+++`).
        let src = "+++\n\
name = \"x\"\n\
system_prompt = \"\"\"\n\
apply this patch:\n\
--- a/foo\n\
+++ b/foo\n\
@@ -1 +1 @@\n\
-old\n\
+new\n\
\"\"\"\n\
+++\n\
the real body\n";
        let (fm, body) = split_front_matter(src).unwrap();
        // The whole triple-quoted value (incl. the +++ diff line) stays in fm.
        assert!(
            fm.contains("+++ b/foo"),
            "diff line must remain inside front matter"
        );
        assert!(fm.contains("system_prompt"));
        assert_eq!(body, "the real body\n");
        // And it actually parses as TOML now (the real regression).
        let parsed: toml::Value = toml::from_str(fm).expect("fm parses as TOML");
        assert!(parsed
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("+++ b/foo"));
    }

    #[test]
    fn split_front_matter_fence_with_trailing_whitespace() {
        // A closing fence line with trailing spaces / CRLF is still a fence.
        let src = "+++\na = 1\n+++  \r\nbody\n";
        let (fm, body) = split_front_matter(src).unwrap();
        assert_eq!(fm.trim(), "a = 1");
        assert_eq!(body, "body\n");
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
        assert!(format!("{err:#}").contains("no `cli`"), "got: {err:#}");
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
            system_prompt_prefix: String::new(),
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
            system_prompt_prefix: String::new(),
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
            system_prompt_prefix: String::new(),
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
            system_prompt_prefix: String::new(),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(resolved.role, "writer");
        assert_eq!(resolved.cli, "claude");
        assert_eq!(resolved.system_prompt, "hello");
        assert!(resolved.depends_on.is_empty());
        // A truly inline agent (no role_ref) produces no handoff key.
        assert_eq!(resolved.handoff_signal, "");
    }

    #[test]
    fn resolve_agent_carries_handoff_signal_from_role() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("backend.md"),
            "+++\nid=\"backend\"\ndefault_cli=\"claude\"\nhandoff_signal=\"backend.done\"\nsystem_prompt_template=\"x\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();
        let agent = SpellAgentManifest {
            role: None,
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("backend".to_string()),
            depends_on: None,
            system_prompt_prefix: String::new(),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(resolved.role, "backend");
        assert_eq!(resolved.handoff_signal, "backend.done");
    }

    #[test]
    fn resolve_agent_carries_handoff_signal_when_role_is_renamed() {
        // BLIND-SPOT REGRESSION: a spell renames a role (`role = "be"`,
        // `role_ref = "backend"`). The resolved `role` is "be", which does NOT
        // exist in the registry — so the old `state.roles.get(&resolved.role)`
        // lookup in run_spell missed and dropped this producer's key from the
        // cycle graph + exit-key registration. Carrying it off the resolved
        // template (via role_ref) keeps it intact.
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("backend.md"),
            "+++\nid=\"backend\"\ndefault_cli=\"claude\"\nhandoff_signal=\"backend.done\"\nsystem_prompt_template=\"x\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();
        let agent = SpellAgentManifest {
            role: Some("be".to_string()),
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("backend".to_string()),
            depends_on: None,
            system_prompt_prefix: String::new(),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        // Symbolic role is the rename; handoff is still the registry role's key.
        assert_eq!(resolved.role, "be");
        assert_eq!(resolved.handoff_signal, "backend.done");
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
            system_prompt_prefix: String::new(),
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
            system_prompt_prefix: String::new(),
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
            system_prompt_prefix: String::new(),
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
            system_prompt_prefix: String::new(),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(resolved.depends_on, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn resolve_agent_prepends_system_prompt_prefix_to_role_template() {
        // The HITL-gate use case: a spell wants to inject a "wait until X
        // exists" gate in front of an UNCHANGED role prompt. The body
        // comes from the role; the prefix comes from the spell.
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("worker.md"),
            "+++\nid=\"worker\"\ndefault_cli=\"claude\"\nsystem_prompt_template=\"YOU ARE WORKER\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();

        let agent = SpellAgentManifest {
            role: None,
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("worker".to_string()),
            depends_on: None,
            system_prompt_prefix: "[GATE] wait for X.".to_string(),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert!(
            resolved.system_prompt.starts_with("[GATE] wait for X."),
            "prefix must come first: {}",
            resolved.system_prompt
        );
        assert!(
            resolved.system_prompt.contains("YOU ARE WORKER"),
            "role body must still be present: {}",
            resolved.system_prompt
        );
        // And they're separated by a blank line so they render as
        // distinct paragraphs in the bootstrap prompt.
        assert!(resolved.system_prompt.contains("\n\nYOU ARE WORKER"));
    }

    #[test]
    fn resolve_agent_empty_prefix_leaves_prompt_unchanged() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("worker.md"),
            "+++\nid=\"worker\"\ndefault_cli=\"claude\"\nsystem_prompt_template=\"BODY\"\n+++",
        )
        .unwrap();
        let roles = RoleRegistry::load_dir(dir.path()).unwrap();
        let agent = SpellAgentManifest {
            role: None,
            cli: None,
            system_prompt: String::new(),
            role_ref: Some("worker".to_string()),
            depends_on: None,
            system_prompt_prefix: String::new(),
        };
        let resolved = resolve_agent(&agent, &roles).unwrap();
        assert_eq!(
            resolved.system_prompt, "BODY",
            "empty prefix should NOT add separator chars"
        );
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
        assert!(
            format!("{err:#}").contains("duplicate role"),
            "got: {err:#}"
        );
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
    fn builtin_ships_init_spell() {
        // The compiled-in `init` spell is what makes "新建空间" work on a
        // packaged app with no reachable spells/ dir. If this ever fails, a
        // fresh install can't create a workspace — exactly the bug this guards.
        let reg = SpellRegistry::builtin();
        let init = reg.get("init").expect("builtin `init` spell must be embedded");
        assert_eq!(init.manifest.name, "init");
        assert!(
            !init.manifest.agents.is_empty(),
            "init spell must declare its orchestrator agent"
        );
    }

    #[test]
    fn builtin_survives_absent_dir_overlay() {
        // Simulate the user-machine condition: builtin base + an overlay dir
        // that doesn't exist. The `init` spell must still be present.
        let dir = tempdir().unwrap();
        let absent = dir.path().join("no-spells-here");
        let mut reg = SpellRegistry::builtin();
        reg.overlay(SpellRegistry::load_dir(&absent).unwrap());
        assert!(reg.get("init").is_some(), "init must survive an empty overlay");
    }

    #[test]
    fn overlay_dir_spell_overrides_builtin_by_name() {
        // A dev editing spells/init.md (or adding a same-named spell) must win
        // over the embedded copy — last-writer-wins by name.
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "init.md",
            "+++\nname = \"init\"\n[[agents]]\nrole = \"r\"\ncli = \"claude\"\nsystem_prompt = \"overridden\"\n+++\n",
        );
        let mut reg = SpellRegistry::builtin();
        reg.overlay(SpellRegistry::load_dir(dir.path()).unwrap());
        let init = reg.get("init").unwrap();
        assert_eq!(
            init.manifest.agents[0].system_prompt, "overridden",
            "dir overlay must shadow the builtin init"
        );
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
            "",
            "main",
            &map,
        );
        assert_eq!(
            rendered,
            "Task: build a parser. Writer is claude-aaa, critic is codex-bbb."
        );
    }

    #[test]
    fn render_prompt_substitutes_workspace_id() {
        let map = HashMap::new();
        let out = render_prompt(
            "write ledger to {workspace_id}/task.ledger.md",
            "",
            "abc123",
            "main",
            &map,
        );
        assert_eq!(out, "write ledger to abc123/task.ledger.md");
    }

    #[test]
    fn render_prompt_substitutes_thread_slug() {
        // The per-direction blackboard prefix: two directions in one workspace
        // resolve the SAME template to distinct keys via {thread_slug}.
        let map = HashMap::new();
        let tmpl = "ledger at {workspace_id}/{thread_slug}/task.ledger.md";
        let main = render_prompt(tmpl, "", "ws1", "main", &map);
        let dark = render_prompt(tmpl, "", "ws1", "dark-mode", &map);
        assert_eq!(main, "ledger at ws1/main/task.ledger.md");
        assert_eq!(dark, "ledger at ws1/dark-mode/task.ledger.md");
    }

    #[test]
    fn render_prompt_leaves_unknown_placeholders_literal() {
        let map = HashMap::new();
        let out = render_prompt("ref {unknown_id} here", "t", "", "main", &map);
        // We deliberately don't strip unknown {…_id} substrings — silent
        // dropping would hide spell author bugs.
        assert!(out.contains("{unknown_id}"));
    }

    #[test]
    fn sanitize_pty_inject_keeps_visible_text_and_newlines_tabs() {
        // Plain multi-line text with CJK + tabs is preserved byte-for-byte.
        let s = "第一行\n\tindented\nplain ascii line";
        assert_eq!(sanitize_pty_inject(s), s);
    }

    #[test]
    fn sanitize_pty_inject_strips_esc_and_csi_osc_sequences() {
        // ESC is dropped; the residual ASCII (e.g. `[2J`, `]0;title`) survives as
        // INERT literal text — it can no longer fire because the ESC is gone.
        let clear = "before\x1b[2Jafter";
        assert_eq!(sanitize_pty_inject(clear), "before[2Jafter");
        // OSC: ESC ] 0 ; title BEL  → ESC and BEL both stripped.
        let osc = "x\x1b]0;evil\x07y";
        assert_eq!(sanitize_pty_inject(osc), "x]0;evily");
        // No ESC byte survives anywhere.
        assert!(!sanitize_pty_inject(clear).contains('\x1b'));
        assert!(!sanitize_pty_inject(osc).contains('\x1b'));
    }

    #[test]
    fn sanitize_pty_inject_strips_carriage_return_and_bel_and_nul() {
        // CR would prematurely submit the paste; BEL/NUL/backspace are control
        // noise. All gone; the surrounding visible text stays.
        let s = "a\rb\x07c\x00d\x08e";
        assert_eq!(sanitize_pty_inject(s), "abcde");
    }

    #[test]
    fn sanitize_pty_inject_strips_c1_eight_bit_controls() {
        // U+0080–U+009F (8-bit C1, incl. the single-byte CSI U+009B) are control
        // codes a terminal can act on; strip them while keeping adjacent text.
        let s = "p\u{9b}2Jq\u{85}r"; // CSI (8-bit) and NEL
        assert_eq!(sanitize_pty_inject(s), "p2Jqr");
    }

    #[test]
    fn sanitize_pty_inject_empty_is_empty() {
        assert_eq!(sanitize_pty_inject(""), "");
    }
}
