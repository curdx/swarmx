//! `GET /api/files/list` + `GET /api/files/read` — a minimal local file browser.
//!
//! flockmux is a local single-user dev tool; workers' shells can already read
//! the disk, and `GET /api/file` already serves images from anywhere. This adds
//! the same for *browsing*: list a directory, read a text file. Gated by the
//! global `require_local_origin` middleware (browser requests carry an Origin),
//! canonicalised, with size caps so a huge/binary file can't blow up the tab.
//!
//! Deliberately NOT chrooted to a workspace: a developer wants to peek at
//! config / logs / sibling repos too. The threat model is loopback + same
//! posture as the existing `/api/file`.

use axum::{extract::Query, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Max bytes returned by `read` — beyond this we truncate (a file browser
/// preview, not a download).
const MAX_READ_BYTES: usize = 512 * 1024;
/// Cap directory listings so a pathological dir (node_modules) can't flood.
const MAX_ENTRIES: usize = 2000;

#[derive(Deserialize)]
pub struct ListQuery {
    /// Absolute directory to list. Defaults to $HOME when absent/empty.
    dir: Option<String>,
}

#[derive(Serialize)]
struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Canonicalise a requested path. Returns an error string on a missing path so
/// the caller can 404 instead of leaking a panic.
fn canon(p: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))
}

pub async fn list_dir(Query(q): Query<ListQuery>) -> impl IntoResponse {
    let raw = match q.dir {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d),
        _ => home(),
    };
    let dir = match canon(&raw) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    };
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
}

pub async fn read_file(Query(q): Query<ReadQuery>) -> impl IntoResponse {
    let path = match canon(Path::new(&q.path)) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    };
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
