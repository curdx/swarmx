//! Runtime binary lookup for GUI-launched desktop builds.
//!
//! A macOS `.app` started from Finder does not inherit the user's interactive
//! shell PATH. Homebrew / npm / cargo tools are usually installed outside the
//! small launchd PATH, so anything that probes or spawns `node`, `npx`,
//! `claude`, or `codex` must search the desktop-relevant directories too.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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

/// PATH used for child processes launched by flockmux. It preserves the parent
/// PATH, then appends common desktop install locations without importing broad
/// shell state or secrets.
pub fn augmented_path() -> OsString {
    let parent_path = std::env::var_os("PATH");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dirs = augmented_path_dirs(parent_path.clone(), home);
    std::env::join_paths(dirs).unwrap_or_else(|_| parent_path.unwrap_or_default())
}

/// Resolve an executable using the same augmented PATH we pass to children.
pub fn resolve_executable(name: &str) -> Option<PathBuf> {
    let parent_path = std::env::var_os("PATH");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dirs = augmented_path_dirs(parent_path, home);
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
}
