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
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(unix)]
use std::time::Duration;

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
    let dirs = augmented_path_dirs(parent_path.clone(), swarmx_home(), login_shell_path());
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
    let dirs = augmented_path_dirs(parent_path, swarmx_home(), login_shell_path());
    resolve_in_dirs(name, &dirs)
}

fn augmented_path_dirs(
    parent_path: Option<OsString>,
    home: Option<PathBuf>,
    shell_dirs: Vec<PathBuf>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path) = parent_path {
        for dir in std::env::split_paths(&path) {
            push_unique(&mut out, &mut seen, dir);
        }
    }

    // The user's login-shell PATH — authoritative for wherever their node/npm
    // toolchain actually lives, whichever version manager put it there. Ranked
    // right after the inherited PATH and ahead of the curated guesses below,
    // which only cover the layouts we happen to know by name.
    for dir in shell_dirs {
        push_unique(&mut out, &mut seen, dir);
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
            ".n/bin",                // n (tj/n), default N_PREFIX=~/.n
            ".nodenv/shims",         // nodenv
            ".nodebrew/current/bin", // nodebrew
        ] {
            push_unique(&mut out, &mut seen, home.join(rel));
        }
        push_node_bins(&mut out, &mut seen, &home.join(".nvm/versions/node"));
        push_fnm_bins(&mut out, &mut seen, &home.join(".fnm/node-versions"));
    }

    if let Some(nvm_dir) = std::env::var_os("NVM_DIR").map(PathBuf::from) {
        push_node_bins(&mut out, &mut seen, &nvm_dir.join("versions/node"));
    }
    // `n` honours $N_PREFIX for a non-default install root.
    if let Some(n_prefix) = std::env::var_os("N_PREFIX").map(PathBuf::from) {
        push_unique(&mut out, &mut seen, n_prefix.join("bin"));
    }

    for dir in UNIX_RUNTIME_DIRS {
        push_unique(&mut out, &mut seen, PathBuf::from(dir));
    }

    out
}

/// PATH as the user's login shell assembles it — the authoritative location of
/// their toolchain. A Finder-launched `.app` inherits only launchd's minimal
/// PATH; the user's node/npm (via nvm, `n`, fnm, Homebrew, nodenv, or a bespoke
/// prefix) lives on whatever PATH their shell rc builds. Rather than enumerate
/// every version manager's on-disk layout — a losing game — run the login shell
/// once and read the PATH it exports. Cached for the process lifetime (PATH is
/// stable per login). Empty on Windows (GUI processes inherit the user PATH
/// there) and whenever the shell can't be run or returns nothing, in which case
/// the curated directory list above is the fallback.
#[cfg(unix)]
fn login_shell_path() -> Vec<PathBuf> {
    static CACHE: OnceLock<Vec<PathBuf>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            probe_login_shell_path()
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default()
        })
        .clone()
}

#[cfg(not(unix))]
fn login_shell_path() -> Vec<PathBuf> {
    Vec::new()
}

/// Run `$SHELL -ilc` once and lift its `$PATH` out of the output. `-l` sources
/// the login files, `-i` the interactive rc where nvm/`n`/fnm mutate PATH. A
/// worker thread drains stdout so a wedged rc file times out (3s) rather than
/// hanging startup; stderr is dropped (a tty-less interactive shell is noisy but
/// still prints PATH on stdout).
#[cfg(unix)]
fn probe_login_shell_path() -> Option<OsString> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let shell = std::env::var_os("SHELL")
        .filter(|s| {
            let p = Path::new(s);
            p.is_absolute() && p.file_name() != Some(OsStr::new("false"))
        })
        .unwrap_or_else(|| OsString::from("/bin/zsh"));

    // Lowercase, non-`SWARMX_` sentinel: unique enough to bracket PATH, and
    // deliberately not matching the `SWARMX_[A-Z_]+` env-var shape the harness
    // check scans for (it is a printf marker, not an environment variable).
    const MARKER: &str = "__swmx_path_edge__";
    // printf (no trailing newline) wrapped in markers so PATH survives even when
    // rc files print their own banners to stdout.
    let script = format!("printf '{MARKER}%s{MARKER}' \"$PATH\"");

    let mut child = Command::new(&shell)
        .arg("-ilc")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    let buf = match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(buf) => buf,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let _ = child.wait();

    extract_marked(&buf, MARKER).map(OsString::from)
}

/// Pull the payload between the first two `marker` occurrences; `None` if the
/// markers are absent or wrap an empty string.
#[cfg(unix)]
fn extract_marked(buf: &str, marker: &str) -> Option<String> {
    let inner = buf.split(marker).nth(1)?;
    (!inner.is_empty()).then(|| inner.to_string())
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
        let dirs = augmented_path_dirs(Some(parent), Some(home), Vec::new());
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
    fn shell_dirs_rank_after_parent_and_before_curated() {
        let parent = OsString::from("/usr/bin");
        let home = PathBuf::from("/Users/example");
        let shell = vec![PathBuf::from("/Users/example/.n/bin")];
        let dirs = augmented_path_dirs(Some(parent), Some(home), shell);
        let pos = |p: &str| dirs.iter().position(|d| d.as_path() == Path::new(p)).unwrap();
        // inherited PATH < login-shell PATH < curated guesses
        assert!(pos("/usr/bin") < pos("/Users/example/.n/bin"));
        assert!(pos("/Users/example/.n/bin") < pos("/opt/homebrew/bin"));
        // a shell dir that also appears in the curated list is not duplicated
        assert_eq!(
            dirs.iter()
                .filter(|d| d.as_path() == Path::new("/Users/example/.n/bin"))
                .count(),
            1
        );
    }

    #[test]
    fn curated_dirs_cover_common_node_managers() {
        let home = PathBuf::from("/Users/example");
        let dirs = augmented_path_dirs(None, Some(home), Vec::new());
        for expect in [
            "/Users/example/.n/bin",      // n
            "/Users/example/.nodenv/shims",
            "/Users/example/.nodebrew/current/bin",
            "/Users/example/.volta/bin",
        ] {
            assert!(dirs.contains(&PathBuf::from(expect)), "missing {expect}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn extract_marked_lifts_path_between_markers() {
        assert_eq!(
            extract_marked("banner\n__M__/a/bin:/usr/bin__M__", "__M__").as_deref(),
            Some("/a/bin:/usr/bin")
        );
        assert_eq!(extract_marked("no markers here", "__M__"), None);
        assert_eq!(extract_marked("__M____M__", "__M__"), None); // empty payload
    }

    // Real-environment check: simulate a Finder-launched .app (minimal launchd
    // PATH, no node on it) and confirm the login-shell PATH import still locates
    // the user's node — whatever version manager installed it. Ignored by
    // default (touches the real shell + filesystem); run manually with:
    //   cargo test -p swarmx-server packaged_env_finds_real_node -- --ignored --nocapture
    #[cfg(unix)]
    #[test]
    #[ignore = "hits the real login shell + filesystem"]
    fn packaged_env_finds_real_node() {
        let minimal = OsString::from("/usr/bin:/bin:/usr/sbin:/sbin");
        let dirs = augmented_path_dirs(Some(minimal), swarmx_home(), login_shell_path());
        let node = resolve_in_dirs("node", &dirs);
        println!("resolved node = {node:?}");
        assert!(
            node.is_some(),
            "login-shell PATH import should find the user's node even under a stripped launchd PATH"
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
