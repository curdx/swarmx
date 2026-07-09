//! Runtime binary lookup for GUI-launched desktop builds.
//!
//! A macOS `.app` started from Finder does not inherit the user's interactive
//! shell PATH. Homebrew / npm / cargo tools are usually installed outside the
//! small launchd PATH, so anything that probes or spawns `node`, `npx`,
//! `claude`, or `codex` must search the desktop-relevant directories too.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// The user's home directory, resolved for a desktop app that may run with a
/// stripped environment. `HOME` (unix) → `USERPROFILE` (Windows, where `HOME`
/// is usually unset) → `None`. `/` and empty are rejected: a Finder-launched
/// `.app` or a Windows sidecar with no home var must NOT silently anchor data
/// and config at `/` or a stray relative `.swarmx/`.
///
/// This is the single source of truth for "where is the user's home"; every
/// data-dir / config-path resolver routes through it so we never again ship a
/// build where half the modules understand Windows and half fall back to a
/// bogus relative path (see the `home()` variants this replaced).
pub fn swarmx_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty() && p.as_os_str() != "/")
}

/// Build a `std::process::Command` for a non-PTY **tool** subprocess — `zulu`,
/// `git`, an `sh -c` objective check, a `--help` flag probe, `<cli> mcp add`,
/// etc. — with the desktop-augmented PATH (and resolved HOME) baked in.
///
/// A `.app` launched from Finder inherits only launchd's minimal PATH, so
/// Homebrew/npm/cargo-installed tools are invisible to a bare `Command::new`.
/// Constructing every tool child through this makes "tool subprocesses run with
/// the augmented PATH" an invariant enforced *by construction* rather than a
/// line each call site must remember to add — which several forgot (fusion's
/// `zulu`, the fusion objective gate, the CLI flag probe), silently killing
/// those features in packaged builds while they worked on the dev machine.
///
/// This is only for utility shell-outs; real agent CLIs spawn through the PTY
/// path (`spawn.rs`), which sets a per-agent environment of its own.
pub fn tool_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.env("PATH", augmented_path());
    if let Some(home) = swarmx_home() {
        cmd.env("HOME", home);
    }
    cmd
}

/// `tokio::process` variant of [`tool_command`], for async shell-outs.
pub fn tool_command_async(program: impl AsRef<OsStr>) -> tokio::process::Command {
    tokio::process::Command::from(tool_command(program))
}

const UNIX_RUNTIME_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/opt/homebrew/opt/node/bin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/local/opt/node/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// PATH used for child processes launched by swarmx. It preserves the parent
/// PATH, then appends common desktop install locations without importing broad
/// shell state or secrets.
pub fn augmented_path() -> OsString {
    let parent_path = std::env::var_os("PATH");
    let dirs = augmented_path_dirs(parent_path.clone(), swarmx_home());
    // `join_paths` fails if any dir contains the platform separator (`:` on
    // unix). Falling back to the bare parent PATH would drop every desktop
    // dir this module exists to add — the worst outcome exactly when the .app
    // was launched from Finder with a stripped PATH. Instead skip the offending
    // dir(s) and re-join, so the useful ones survive.
    std::env::join_paths(&dirs).unwrap_or_else(|_| {
        let list_sep = if cfg!(windows) { ';' } else { ':' };
        let clean = dirs.iter().filter(|d| !d.to_string_lossy().contains(list_sep));
        std::env::join_paths(clean).unwrap_or_else(|_| parent_path.clone().unwrap_or_default())
    })
}

/// Resolve an executable using the same augmented PATH we pass to children.
pub fn resolve_executable(name: &str) -> Option<PathBuf> {
    let parent_path = std::env::var_os("PATH");
    let dirs = augmented_path_dirs(parent_path, swarmx_home());
    resolve_in_dirs(name, &dirs)
}

fn augmented_path_dirs(parent_path: Option<OsString>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path) = parent_path {
        for dir in std::env::split_paths(&path) {
            push_unique(&mut out, &mut seen, dir);
        }
    }

    if let Some(home) = home.as_deref() {
        for rel in [
            ".local/bin",
            ".cargo/bin",
            ".volta/bin",
            ".asdf/shims",
            ".local/share/mise/shims",
            ".bun/bin",
            ".deno/bin",
            ".npm-global/bin",
        ] {
            push_unique(&mut out, &mut seen, home.join(rel));
        }
        push_node_bins(&mut out, &mut seen, &home.join(".nvm/versions/node"));
        push_fnm_bins(&mut out, &mut seen, &home.join(".fnm/node-versions"));
    }

    if let Some(nvm_dir) = std::env::var_os("NVM_DIR").map(PathBuf::from) {
        push_node_bins(&mut out, &mut seen, &nvm_dir.join("versions/node"));
    }

    for dir in UNIX_RUNTIME_DIRS {
        push_unique(&mut out, &mut seen, PathBuf::from(dir));
    }

    out
}

fn push_node_bins(out: &mut Vec<PathBuf>, seen: &mut HashSet<OsString>, root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let bin = entry.path().join("bin");
        if bin.is_dir() {
            push_unique(out, seen, bin);
        }
    }
}

fn push_fnm_bins(out: &mut Vec<PathBuf>, seen: &mut HashSet<OsString>, root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let bin = entry.path().join("installation/bin");
        if bin.is_dir() {
            push_unique(out, seen, bin);
        }
    }
}

fn push_unique(out: &mut Vec<PathBuf>, seen: &mut HashSet<OsString>, path: PathBuf) {
    if path.as_os_str().is_empty() {
        return;
    }
    if seen.insert(path.as_os_str().to_os_string()) {
        out.push(path);
    }
}

fn resolve_in_dirs(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if has_path_separator(name) || Path::new(name).is_absolute() {
        let path = PathBuf::from(name);
        return is_executable_file(&path).then_some(path);
    }

    for dir in dirs {
        for file in command_filenames(name) {
            let cand = dir.join(&file);
            if is_executable_file(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

fn has_path_separator(name: &str) -> bool {
    name.contains('/') || name.contains('\\')
}

fn command_filenames(name: &str) -> Vec<String> {
    if !cfg!(windows) || Path::new(name).extension().is_some() {
        return vec![name.to_string()];
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut out = vec![name.to_string()];
    for ext in pathext.split(';').filter(|s| !s.is_empty()) {
        out.push(format!("{name}{ext}"));
        out.push(format!("{name}{}", ext.to_ascii_lowercase()));
    }
    out
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augmented_path_preserves_parent_and_adds_desktop_dirs() {
        let parent = OsString::from("/custom/bin:/usr/bin");
        let home = PathBuf::from("/Users/example");
        let dirs = augmented_path_dirs(Some(parent), Some(home));
        assert_eq!(dirs.first(), Some(&PathBuf::from("/custom/bin")));
        assert!(dirs.contains(&PathBuf::from("/Users/example/.local/bin")));
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert_eq!(
            dirs.iter()
                .filter(|p| p.as_path() == Path::new("/usr/bin"))
                .count(),
            1
        );
    }

    #[test]
    fn resolve_in_dirs_finds_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("node");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            let mut perms = std::fs::metadata(&bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&bin, perms).unwrap();
        }

        let found = resolve_in_dirs("node", &[dir.path().to_path_buf()]);
        assert_eq!(found, Some(bin));
    }

    #[test]
    fn resolve_in_dirs_ignores_non_executable_files_on_unix() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("node");
        std::fs::write(&bin, b"not executable").unwrap();

        #[cfg(unix)]
        assert_eq!(resolve_in_dirs("node", &[dir.path().to_path_buf()]), None);
    }

    #[test]
    fn tool_command_bakes_in_augmented_path() {
        // A tool subprocess must carry the augmented PATH so a Finder-launched
        // .app (stripped launchd PATH) can still find Homebrew/npm/cargo tools;
        // relying on each call site to remember `.env("PATH", …)` is what let
        // fusion/zulu and the objective gate silently die when packaged.
        let cmd = tool_command("zulu");
        let has_path = cmd
            .get_envs()
            .any(|(k, v)| k == OsStr::new("PATH") && v.is_some());
        assert!(has_path, "tool_command must set PATH by construction");
    }
}
