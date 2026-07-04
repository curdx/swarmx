//! Comate Zulu license storage. Unlike claude/codex (OAuth reused from
//! `~/.claude`/`~/.codex`) or reasonix (a `DEEPSEEK_API_KEY` env var), zulu needs
//! an explicit Comate SaaS license. The packaged sidecar runs with no env
//! (CWD=`/`, per the project's zero-config principle), so the license is
//! persisted to `~/.swarmx/comate.json` via the settings page and read at spawn.
//! `COMATE_LICENSE` in the environment still wins (dev / CI override).

use std::path::PathBuf;

/// `~/.swarmx/comate.json`. HOME→USERPROFILE fallback so the Windows installed
/// app (no HOME, CWD=`/`) writes/reads the right place — mirrors
/// `pricing_config_path`/`models_config_path`.
fn config_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(home).join(".swarmx").join("comate.json");
    }
    PathBuf::from(".swarmx/comate.json")
}

/// Read the license from disk (`{"license": "..."}`), or "" if absent/malformed.
fn file_license() -> String {
    let path = config_path();
    let Ok(txt) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    serde_json::from_str::<serde_json::Value>(&txt)
        .ok()
        .and_then(|v| v.get("license").and_then(|l| l.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// Where a resolved license came from (for the settings page).
pub enum Source {
    Env,
    File,
    None,
}

/// The effective license for spawning a zulu agent: `COMATE_LICENSE` env wins
/// (dev/CI override), else the settings-page value on disk, else empty.
pub fn load_license() -> String {
    match std::env::var("COMATE_LICENSE") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => file_license(),
    }
}

/// `(license, source)` — the resolved license plus where it came from.
pub fn resolve() -> (String, Source) {
    if let Ok(v) = std::env::var("COMATE_LICENSE") {
        if !v.trim().is_empty() {
            return (v, Source::Env);
        }
    }
    let f = file_license();
    if f.is_empty() {
        (f, Source::None)
    } else {
        (f, Source::File)
    }
}

/// Persist the license to `~/.swarmx/comate.json` (atomic temp→rename). An empty
/// string clears it. Errors bubble to the caller (surfaced as a 500).
pub fn save_license(license: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&serde_json::json!({ "license": license.trim() }))?;
    let tmp = path.with_extension(crate::models_config::unique_tmp_ext());
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Mask all but the last 4 chars of a credential for display.
pub fn mask(s: &str) -> String {
    let s = s.trim();
    let n = s.chars().count();
    if n <= 4 {
        return "•".repeat(n);
    }
    let tail: String = s.chars().skip(n - 4).collect();
    format!("{}{}", "•".repeat(n - 4), tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_keeps_last_four() {
        assert_eq!(mask("5ed59fa0-c9e7-4823-96c8-42aa83d3a044"), {
            let n = "5ed59fa0-c9e7-4823-96c8-42aa83d3a044".chars().count();
            format!("{}a044", "•".repeat(n - 4))
        });
        assert_eq!(mask("abcd"), "••••");
        assert_eq!(mask(""), "");
    }
}
