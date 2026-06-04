//! Git worktree helpers for "directions" (threads) — automatic, zero-config
//! file isolation so two directions in one project don't overwrite each other.
//!
//! Design goal (per product spec): the USER never sees git/branch/worktree/
//! checkout. When a direction wants isolation we silently:
//!   1. make the project a git repo if it isn't one yet (`git init` + a first
//!      commit so `worktree add` has a base), and
//!   2. `git worktree add <project>-<branch> -b <branch>` — a sibling dir next
//!      to the project the user can see in Finder/IDE.
//!
//! All git invocations are time-boxed on a worker thread (mirrors F17's
//! `binary_supports_flag` pattern) so a hung/slow git can never stall the async
//! spawn path. We NEVER touch global git config — only `-c user.*` inline when
//! a fresh repo has no identity, so the first commit doesn't fail on a machine
//! without `git config --global user.email`.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Hard ceiling for any single git invocation. `worktree add` on a large repo
/// can take a couple seconds; init/commit are fast. 30s is generous headroom
/// while still bounding a pathological hang.
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Output of a finished git command we care about.
struct GitOut {
    status_ok: bool,
    stdout: String,
    stderr: String,
}

/// Run `git -C <cwd> <args...>` with a hard timeout on a worker thread, so a
/// hung git can't block the caller (the async spawn path calls this via
/// `spawn_blocking`). Returns `Err` only when git couldn't be launched or the
/// timeout elapsed; a non-zero exit is reported via `GitOut.status_ok=false`.
fn git(cwd: &Path, args: &[&str]) -> Result<GitOut> {
    let cwd = cwd.to_path_buf();
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // output() drains stdout+stderr (so git can't deadlock on a full pipe).
        let out = Command::new("git").arg("-C").arg(&cwd).args(&owned).output();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(GIT_TIMEOUT) {
        Ok(Ok(o)) => Ok(GitOut {
            status_ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }),
        Ok(Err(e)) => Err(anyhow!("git spawn failed: {e}")),
        Err(_) => Err(anyhow!("git command timed out after {GIT_TIMEOUT:?}")),
    }
}

/// Is `dir` inside a git work tree? Used to decide whether a direction can get
/// a worktree immediately or whether we need to `git init` first. A timeout /
/// launch error is treated as "not a repo" (we'll try to init, which is safe —
/// `git init` on an existing repo is a no-op).
#[allow(dead_code)] // documented helper; current flow uses idempotent init
pub fn is_git_repo(dir: &Path) -> bool {
    match git(dir, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(o) => o.status_ok && o.stdout.trim() == "true",
        Err(_) => false,
    }
}

/// The branch currently checked out in `dir`, for the sidebar's live branch
/// chip. Returns `None` for a detached HEAD (rev-parse yields the literal
/// "HEAD"), a non-git dir, or any git error/timeout — the caller renders no
/// chip in that case. Blocking (shells out to git); call off the async path.
pub fn current_branch(dir: &Path) -> Option<String> {
    let o = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    if !o.status_ok {
        return None;
    }
    let b = o.stdout.trim();
    if b.is_empty() || b == "HEAD" {
        return None; // detached / no commit yet
    }
    Some(b.to_string())
}

/// Does `dir`'s work tree have uncommitted changes — staged, unstaged, or
/// untracked? `git status --porcelain` prints one line per change, so any
/// non-empty output means dirty. This is **read-only** (never touches the work
/// tree or index), so it's safe to call while an agent is mid-edit — surfacing
/// "this direction has unsaved work" without disturbing it. Returns `false` for
/// a non-git dir / error / timeout: we'd rather show no marker than a false
/// "dirty". Blocking (shells out to git); call off the async path.
pub fn working_dirty(dir: &Path) -> bool {
    match git(dir, &["status", "--porcelain"]) {
        Ok(o) => o.status_ok && !o.stdout.trim().is_empty(),
        Err(_) => false,
    }
}

/// Make `dir` a git repo and create an initial commit so `worktree add` has a
/// base to branch from. Idempotent-ish: `git init` on an existing repo is a
/// no-op; if there's already a commit we skip committing. Uses inline
/// `-c user.*` ONLY when the repo has no identity, so a machine without a
/// global git identity can still get its first commit — global config is never
/// modified.
pub fn git_init_with_commit(dir: &Path) -> Result<()> {
    // 1) init (no-op if already a repo).
    let init = git(dir, &["init"]).context("git init")?;
    if !init.status_ok {
        return Err(anyhow!("git init failed: {}", init.stderr.trim()));
    }

    // 2) already have a commit? then we're done — don't churn history.
    if git(dir, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status_ok)
        .unwrap_or(false)
    {
        return Ok(());
    }

    // 3) stage everything. Empty dir is fine (commit --allow-empty below).
    let add = git(dir, &["add", "-A"]).context("git add -A")?;
    if !add.status_ok {
        return Err(anyhow!("git add failed: {}", add.stderr.trim()));
    }

    // 4) commit. Supply an inline identity so the commit doesn't fail on a box
    //    with no `git config --global user.email`. `--allow-empty` covers a
    //    brand-new empty project dir.
    let commit = git(
        dir,
        &[
            "-c",
            "user.name=flockmux",
            "-c",
            "user.email=flockmux@localhost",
            "commit",
            "--allow-empty",
            "-m",
            "flockmux: initial commit (enables parallel directions)",
        ],
    )
    .context("git commit")?;
    if !commit.status_ok {
        return Err(anyhow!("git commit failed: {}", commit.stderr.trim()));
    }
    Ok(())
}

/// Sanitize a human/AI-suggested branch name into the dir suffix we append to
/// the project basename. Git branch names are fairly permissive but a sibling
/// DIRECTORY name should be filesystem-friendly: keep `[a-z0-9._-]`, collapse
/// the rest to `-`, trim leading/trailing separators, lowercase. Empty → "dir".
/// Reused by the thread REST layer to derive a stable blackboard slug + branch
/// from a human/AI-supplied direction name.
pub(crate) fn sanitize_suffix(branch: &str) -> String {
    let mut s: String = branch
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches(|c| c == '-' || c == '.' || c == '_').to_string();
    if s.is_empty() {
        // A fully non-ASCII name (e.g. Chinese "深色模式") would otherwise
        // collapse to the SAME literal for every such name, so distinct
        // directions would collide on one branch/worktree (only kept apart by
        // unique_thread_slug's `-2/-3` suffixing) and read as meaningless.
        // Derive a stable short hash of the original so each distinct name maps
        // to its own slug/branch. DefaultHasher has a fixed (seedless) initial
        // state, so this is deterministic across runs.
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        branch.hash(&mut h);
        format!("dir-{:06x}", (h.finish() as u32) & 0x00ff_ffff)
    } else {
        s
    }
}

/// Compute the sibling worktree path for a project + branch: `<parent>/<base>-<suffix>`.
/// Pure (no IO) so it's unit-testable and the caller can show a path preview.
pub fn worktree_dest(project_cwd: &Path, branch: &str) -> PathBuf {
    let parent = project_cwd.parent().unwrap_or_else(|| Path::new("."));
    let base = project_cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    parent.join(format!("{base}-{}", sanitize_suffix(branch)))
}

/// Create a git worktree for `branch` as a sibling of `repo_cwd`. Returns the
/// destination path on success. If `branch` already exists, retries without
/// `-b` (attach the existing branch instead of creating it). The caller has
/// already ensured `repo_cwd` is a git repo (via `git_init_with_commit`).
pub fn worktree_add(repo_cwd: &Path, branch: &str) -> Result<PathBuf> {
    let dest = worktree_dest(repo_cwd, branch);
    let dest_str = dest.to_string_lossy().into_owned();

    // First try creating a fresh branch.
    let with_new = git(repo_cwd, &["worktree", "add", &dest_str, "-b", branch])
        .context("git worktree add -b")?;
    if with_new.status_ok {
        return Ok(dest);
    }

    // Branch may already exist → attach it instead of -b. (Also covers a stale
    // dest dir from a prior run; surfaced via stderr if it still fails.)
    let attach = git(repo_cwd, &["worktree", "add", &dest_str, branch])
        .context("git worktree add (existing branch)")?;
    if attach.status_ok {
        return Ok(dest);
    }

    Err(anyhow!(
        "git worktree add failed for branch `{branch}` at {dest_str}: {} / {}",
        with_new.stderr.trim(),
        attach.stderr.trim()
    ))
}

/// Local branches of the repo at `dir`, each flagged whether it's currently
/// checked out in some worktree (the main one or a direction's). A checked-out
/// branch can't be attached to a new worktree, so the "open existing branch"
/// picker disables those. Empty for a non-git dir / error / timeout. Blocking
/// (shells out to git); call off the async path.
pub fn list_branches(dir: &Path) -> Vec<(String, bool)> {
    let names = match git(dir, &["branch", "--format=%(refname:short)"]) {
        Ok(o) if o.status_ok => o.stdout,
        _ => return Vec::new(),
    };
    // Branches checked out in any worktree — `worktree list --porcelain` emits a
    // `branch refs/heads/<name>` line per attached worktree.
    let mut checked_out: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(o) = git(dir, &["worktree", "list", "--porcelain"]) {
        if o.status_ok {
            for line in o.stdout.lines() {
                if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    checked_out.insert(b.trim().to_string());
                }
            }
        }
    }
    names
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|name| (name.to_string(), checked_out.contains(name)))
        .collect()
}

/// Remove a worktree (best-effort — caller logs but doesn't fail the request).
/// `--force` so an uncommitted-changes worktree still gets cleaned up when the
/// user deletes the direction (they chose to discard it).
pub fn worktree_remove(repo_cwd: &Path, dest: &Path) -> Result<()> {
    let dest_str = dest.to_string_lossy().into_owned();
    let out = git(repo_cwd, &["worktree", "remove", "--force", &dest_str])
        .context("git worktree remove")?;
    if out.status_ok {
        Ok(())
    } else {
        Err(anyhow!("git worktree remove failed: {}", out.stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_suffix_makes_fs_friendly_names() {
        assert_eq!(sanitize_suffix("dark-mode"), "dark-mode");
        assert_eq!(sanitize_suffix("Dark Mode!!"), "dark-mode");
        assert_eq!(sanitize_suffix("feature/api v2"), "feature-api-v2");
        // Non-ascii names no longer all collapse to a single "dir": each gets a
        // stable, distinct `dir-<hash>` so directions don't share a branch.
        assert!(sanitize_suffix("深色").starts_with("dir-"));
        assert_eq!(sanitize_suffix("深色"), sanitize_suffix("深色")); // deterministic
        assert_ne!(sanitize_suffix("深色"), sanitize_suffix("浅色")); // distinct names → distinct slugs
        assert_eq!(sanitize_suffix("深色 mode"), "mode"); // ascii kept when present
        assert_eq!(sanitize_suffix("--x--"), "x");
        assert!(sanitize_suffix("").starts_with("dir-"));
    }

    #[test]
    fn worktree_dest_is_sibling_of_project() {
        let p = worktree_dest(Path::new("/home/me/code/myproj"), "dark-mode");
        assert_eq!(p, Path::new("/home/me/code/myproj-dark-mode"));
    }

    #[test]
    fn worktree_dest_handles_trailing_slash_and_messy_branch() {
        let p = worktree_dest(Path::new("/tmp/proj"), "Feature/API v2");
        assert_eq!(p, Path::new("/tmp/proj-feature-api-v2"));
    }

    // End-to-end git test: init a throwaway repo, add a worktree, remove it.
    // Skipped automatically if `git` isn't on PATH.
    #[test]
    fn init_add_remove_roundtrip_on_real_git() {
        if Command::new("git").arg("--version").output().is_err() {
            return; // no git → skip
        }
        let dir = tempfile::TempDir::new().unwrap();
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("README.md"), b"hi").unwrap();

        assert!(!is_git_repo(&proj), "fresh dir is not a repo");
        assert_eq!(current_branch(&proj), None, "non-repo has no branch");
        git_init_with_commit(&proj).expect("init+commit");
        assert!(is_git_repo(&proj), "now a repo");
        // The default branch name varies (main/master) by git version/config,
        // so just assert we read *some* non-empty branch on the committed repo.
        assert!(
            current_branch(&proj).is_some_and(|b| !b.is_empty()),
            "committed repo reports a branch",
        );

        let wt = worktree_add(&proj, "dark-mode").expect("worktree add");
        assert!(wt.exists(), "worktree dir created");
        assert!(wt.join("README.md").exists(), "worktree has the files");
        assert_eq!(wt.file_name().unwrap(), "proj-dark-mode");
        assert_eq!(
            current_branch(&wt).as_deref(),
            Some("dark-mode"),
            "worktree is on its own branch",
        );

        worktree_remove(&proj, &wt).expect("worktree remove");
        assert!(!wt.exists(), "worktree dir gone after remove");
    }
}
