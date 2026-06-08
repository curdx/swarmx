//! `GET /api/files/list` + `GET /api/files/read` — a minimal local file browser.
//!
//! flockmux is a local single-user dev tool; workers' shells can already read
//! the disk, and `GET /api/file` already serves images from anywhere. This adds
//! the same for *browsing*: list a directory, read a text file. Gated by the
//! global `require_local_origin` middleware (browser requests carry an Origin),
//! canonicalised, with size caps so a huge/binary file can't blow up the tab.
//!
//! Jail: when a `workspace_id` is supplied the browser is chrooted to that
//! workspace's roots (its `cwd` + any attached roots) — listing/reading a path
//! that escapes them returns 403. The UI passes `all=1` (the "browse whole
//! filesystem" toggle) to opt out, restoring the original posture where a
//! developer can peek at sibling repos / config / logs. A bare call with no
//! `workspace_id` is unrestricted (loopback + same posture as `/api/file`).

use axum::{extract::Query, extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

use crate::AppState;

/// Max bytes returned by `read` — beyond this we truncate (a file browser
/// preview, not a download).
const MAX_READ_BYTES: usize = 512 * 1024;
/// Cap directory listings so a pathological dir (node_modules) can't flood.
const MAX_ENTRIES: usize = 2000;

#[derive(Deserialize)]
pub struct ListQuery {
    /// Absolute directory to list. Defaults to the workspace `cwd` when a
    /// `workspace_id` is given, else $HOME.
    dir: Option<String>,
    /// Jail to this workspace's roots unless `all` is truthy.
    workspace_id: Option<String>,
    /// Escape hatch: `1`/`true` disables the workspace jail for this request.
    all: Option<String>,
}

#[derive(Serialize)]
struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Canonicalise a requested path. Returns an error string on a missing path so
/// the caller can 404 instead of leaking a panic.
fn canon(p: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// `all=1` / `all=true` ⇒ disable the jail (serde won't coerce a query string
/// into a bool, so we parse it ourselves).
fn truthy(o: &Option<String>) -> bool {
    matches!(o.as_deref(), Some("1") | Some("true"))
}

/// The canonicalised roots a workspace is allowed to browse: its `cwd` plus any
/// attached roots. Roots that fail to canonicalise (e.g. a deleted dependency
/// dir) are skipped rather than erroring. First entry, if any, is the `cwd` —
/// used as the default directory for an empty `dir`.
async fn allowed_roots(state: &AppState, ws_id: &str) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(Some(ws)) = state.store.get_workspace_by_id(ws_id.to_string()).await {
        if let Ok(p) = std::fs::canonicalize(&ws.cwd) {
            roots.push(p);
        }
    }
    if let Ok(rs) = state.store.list_workspace_roots(ws_id.to_string()).await {
        for r in rs {
            if let Ok(p) = std::fs::canonicalize(&r.path) {
                if !roots.contains(&p) {
                    roots.push(p);
                }
            }
        }
    }
    roots
}

/// True if a canonical absolute path is inside (or equal to) any allowed root.
/// `Path::starts_with` is component-wise, so `/a/bc` is NOT inside `/a/b`.
fn is_within_any(target: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| target.starts_with(r))
}

fn jail_denied() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "path outside workspace; enable \"browse whole filesystem\" to view"
        })),
    )
        .into_response()
}

pub async fn list_dir(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let ws_id = q.workspace_id.as_deref().filter(|s| !s.is_empty());
    // Fetch the jail roots once; reuse them for both the default dir and the gate.
    let roots = match ws_id {
        Some(id) => allowed_roots(&state, id).await,
        None => Vec::new(),
    };
    let raw = match q.dir {
        Some(ref d) if !d.trim().is_empty() => PathBuf::from(d),
        // No dir: default to the workspace cwd (first root) when scoped, else $HOME.
        _ => roots.first().cloned().unwrap_or_else(home),
    };
    let dir = match canon(&raw) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    };
    // Jail gate: scoped + not opted out + outside every root ⇒ 403.
    if ws_id.is_some() && !truthy(&q.all) && !is_within_any(&dir, &roots) {
        return jail_denied();
    }
    if !dir.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("not a directory: {}", dir.display()) })),
        )
            .into_response();
    }
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": format!("read_dir {}: {e}", dir.display()) })),
            )
                .into_response()
        }
    };
    let mut entries: Vec<Entry> = Vec::new();
    for ent in rd.flatten().take(MAX_ENTRIES) {
        let name = ent.file_name().to_string_lossy().into_owned();
        let meta = ent.metadata().ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        entries.push(Entry { name, is_dir, size });
    }
    // Dirs first, then files; each alphabetical (case-insensitive).
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let parent = dir.parent().map(|p| p.to_string_lossy().into_owned());
    Json(json!({
        "dir": dir.to_string_lossy(),
        "parent": parent,
        "entries": entries,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct ReadQuery {
    path: String,
    /// Jail to this workspace's roots unless `all` is truthy.
    workspace_id: Option<String>,
    /// Escape hatch: `1`/`true` disables the workspace jail for this request.
    all: Option<String>,
}

pub async fn read_file(
    State(state): State<AppState>,
    Query(q): Query<ReadQuery>,
) -> impl IntoResponse {
    let path = match canon(Path::new(&q.path)) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    };
    let ws_id = q.workspace_id.as_deref().filter(|s| !s.is_empty());
    if ws_id.is_some() && !truthy(&q.all) {
        let roots = allowed_roots(&state, ws_id.unwrap()).await;
        if !is_within_any(&path, &roots) {
            return jail_denied();
        }
    }
    if !path.is_file() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("not a file: {}", path.display()) })),
        )
            .into_response();
    }
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": format!("read {}: {e}", path.display()) })),
            )
                .into_response()
        }
    };
    let total = bytes.len();
    let truncated = total > MAX_READ_BYTES;
    let slice = &bytes[..total.min(MAX_READ_BYTES)];
    // Heuristic: a NUL in the head ⇒ binary; don't return garbage as text.
    let binary = slice.iter().take(8192).any(|&b| b == 0);
    if binary {
        return Json(json!({
            "path": path.to_string_lossy(),
            "binary": true,
            "size": total,
            "content": serde_json::Value::Null,
            "truncated": truncated,
        }))
        .into_response();
    }
    Json(json!({
        "path": path.to_string_lossy(),
        "binary": false,
        "size": total,
        "content": String::from_utf8_lossy(slice),
        "truncated": truncated,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_within_any_component_wise() {
        let roots = vec![PathBuf::from("/a/b")];
        assert!(is_within_any(Path::new("/a/b"), &roots)); // the root itself
        assert!(is_within_any(Path::new("/a/b/c"), &roots)); // a child
        assert!(!is_within_any(Path::new("/a/bc"), &roots)); // sibling sharing a string prefix
        assert!(!is_within_any(Path::new("/a"), &roots)); // a parent
        assert!(!is_within_any(Path::new("/x/y"), &roots)); // unrelated
    }

    #[test]
    fn is_within_any_multi_root() {
        let roots = vec![PathBuf::from("/proj"), PathBuf::from("/deps/lib")];
        assert!(is_within_any(Path::new("/deps/lib/src"), &roots));
        assert!(is_within_any(Path::new("/proj/x"), &roots));
        assert!(!is_within_any(Path::new("/deps/other"), &roots));
        assert!(!is_within_any(Path::new("/etc/passwd"), &roots));
        assert!(!is_within_any(Path::new("/etc"), &[]));
    }

    #[test]
    fn truthy_parses_query_strings() {
        assert!(truthy(&Some("1".into())));
        assert!(truthy(&Some("true".into())));
        assert!(!truthy(&Some("0".into())));
        assert!(!truthy(&Some(String::new())));
        assert!(!truthy(&None));
    }
}
