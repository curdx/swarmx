//! Path-traversal defence. Every blackboard read/write resolves the rel-path
//! against the configured root and asserts the result stays inside the root.
//!
//! `canonicalize` would refuse to handle paths that don't exist yet — which
//! is exactly the write case. So for writes we canonicalize the *parent*
//! and re-join the leaf; for reads we canonicalize the full path. Both
//! cases end with the same `starts_with(root)` check.

use anyhow::{anyhow, bail, Result};
use std::path::{Component, Path, PathBuf};

/// Resolve `rel` under `root`, expecting the target to already exist
/// (e.g. for read). Returns the canonical absolute path.
pub fn resolve_existing(root: &Path, rel: &str) -> Result<PathBuf> {
    reject_traversal_lexically(rel)?;
    let canon_root = root
        .canonicalize()
        .map_err(|e| anyhow!("canonicalize root {}: {}", root.display(), e))?;
    let joined = canon_root.join(rel);
    let canon = joined
        .canonicalize()
        .map_err(|e| anyhow!("canonicalize target {}: {}", joined.display(), e))?;
    if !canon.starts_with(&canon_root) {
        bail!("path {} escapes blackboard root {}", canon.display(), canon_root.display());
    }
    Ok(canon)
}

/// Resolve `rel` under `root`, allowing the target file to not exist yet
/// (write case). The *parent* directory must exist and lie under root;
/// the file name is appended afterwards.
pub fn resolve_for_write(root: &Path, rel: &str) -> Result<PathBuf> {
    reject_traversal_lexically(rel)?;
    let canon_root = root
        .canonicalize()
        .map_err(|e| anyhow!("canonicalize root {}: {}", root.display(), e))?;
    let joined = canon_root.join(rel);
    let parent = joined
        .parent()
        .ok_or_else(|| anyhow!("path {} has no parent", joined.display()))?;
    // Create the parent under root so first-write of "subdir/foo.md" works.
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow!("create_dir_all {}: {}", parent.display(), e))?;
    let canon_parent = parent
        .canonicalize()
        .map_err(|e| anyhow!("canonicalize parent {}: {}", parent.display(), e))?;
    if !canon_parent.starts_with(&canon_root) {
        bail!(
            "parent {} escapes blackboard root {}",
            canon_parent.display(),
            canon_root.display()
        );
    }
    let file_name = joined
        .file_name()
        .ok_or_else(|| anyhow!("path {} has no file name", joined.display()))?;
    Ok(canon_parent.join(file_name))
}

/// First-pass lexical reject: `..`, absolute paths, root-anchored, or
/// Windows-drive components. canonicalize would catch most, but a clean
/// early error keeps the failure mode obvious.
fn reject_traversal_lexically(rel: &str) -> Result<()> {
    let p = Path::new(rel);
    if p.is_absolute() {
        bail!("rel path must not be absolute: {rel}");
    }
    for c in p.components() {
        match c {
            Component::ParentDir => bail!("rel path must not contain ..: {rel}"),
            Component::RootDir | Component::Prefix(_) => {
                bail!("rel path must not be root-anchored: {rel}")
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn rejects_parent_dir() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        assert!(resolve_for_write(root, "../etc/passwd").is_err());
        assert!(resolve_for_write(root, "a/../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_absolute() {
        let dir = TempDir::new().unwrap();
        assert!(resolve_for_write(dir.path(), "/etc/passwd").is_err());
    }

    #[test]
    fn accepts_nested_write_path() {
        let dir = TempDir::new().unwrap();
        let p = resolve_for_write(dir.path(), "sub/dir/notes.md").unwrap();
        assert!(p.starts_with(dir.path().canonicalize().unwrap()));
        assert!(p.ends_with("sub/dir/notes.md"));
    }

    #[test]
    fn resolve_existing_rejects_missing() {
        let dir = TempDir::new().unwrap();
        assert!(resolve_existing(dir.path(), "missing.md").is_err());
    }

    #[test]
    fn resolve_existing_ok_for_real_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("ok.md"), b"hi").unwrap();
        let p = resolve_existing(dir.path(), "ok.md").unwrap();
        assert!(p.starts_with(dir.path().canonicalize().unwrap()));
    }
}
