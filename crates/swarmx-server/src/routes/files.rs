//! `GET /api/files/list` + `GET /api/files/read` — a minimal local file browser.
//!
//! swarmx is a local single-user dev tool; workers' shells can already read
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

use axum::{
    extract::Query, extract::State, http::header, http::HeaderMap, http::StatusCode,
    response::IntoResponse, Json,
};
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

/// Resolve the user's home dir. `None` when neither `HOME` (unix) nor
/// `USERPROFILE` (Windows — `HOME` is often unset there) is set; callers must
/// treat `None` as "can't anchor $HOME-relative credential checks" and fall
/// back to name-based matching rather than trusting a bogus `/` prefix.
/// Delegates to the single home resolver so this and the data-dir defaults
/// can't drift apart.
fn home() -> Option<PathBuf> {
    crate::runtime_path::swarmx_home()
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

/// A request is "from the app UI" if it is a browser request from a local page,
/// as opposed to a headless local client (`curl`, the MCP subprocess via
/// reqwest, a sandboxed/landed exploit that can only speak HTTP to loopback).
/// Only the UI may do *unscoped* full-disk reads; a native client is confined to
/// a workspace's roots — otherwise `/api/files/read` becomes an arbitrary-file
/// oracle for any local process. Two browser shapes qualify:
///
/// 1. A **local `Origin`** — a *cross-origin* fetch, e.g. the Tauri webview
///    (`tauri://localhost`) calling the sidecar on `127.0.0.1:7777`, or the Vite
///    page hitting the API on a different port. The original signal.
/// 2. **`Sec-Fetch-Site: same-origin`** — a *same-origin* fetch sends NO Origin
///    (browsers omit it on same-origin GET), so (1) misses the in-app file
///    browser and the create-workspace path-probe whenever page and API share an
///    origin (Vite dev, or the bundle on `:7777`). This Fetch-Metadata header is
///    attached by the browser and is a forbidden header name (page JS can't
///    forge it); curl/reqwest send neither Origin nor `Sec-Fetch-*`, so the
///    anti-native-client bar holds. Paired with a loopback `Host` so a no-Origin
///    DNS-rebind navigation (`Host: attacker.com`, already rejected upstream by
///    `require_local_origin`) can't reach a full-disk read even if that
///    middleware is reordered.
///
/// NOTE: a process that forges these headers, or a same-origin XSS inside the
/// webview, still slips past — the irreducible limit of a token-less loopback
/// tool; `is_sensitive` is the remaining backstop.
fn is_ui_request(headers: &HeaderMap) -> bool {
    if headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(origin_host_is_local)
        .unwrap_or(false)
    {
        return true;
    }
    let same_origin_fetch = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("same-origin"))
        .unwrap_or(false);
    let host_loopback = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(crate::host_is_loopback)
        .unwrap_or(false);
    same_origin_fetch && host_loopback
}

/// True if an `Origin` value (`http://localhost:5173`, `tauri://localhost`,
/// `https://tauri.localhost`, `http://[::1]:7777`) names a loopback host.
fn origin_host_is_local(origin: &str) -> bool {
    let host = origin.split_once("://").map(|(_, r)| r).unwrap_or(origin);
    let host = host.split('/').next().unwrap_or(host);
    // strip a trailing `:port` (but keep IPv6 inside brackets intact first).
    let host = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host)
    };
    host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host.ends_with(".localhost")
}

/// 403 for an unscoped (full-disk) read attempted by a non-UI client.
fn unscoped_denied() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "full-disk file access is limited to the swarmx UI; \
                      pass a workspace_id to browse within a workspace"
        })),
    )
        .into_response()
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

/// A scoped request whose workspace resolved to zero browsable roots — its
/// `cwd` was deleted/moved (canonicalize failed) and no attached root resolves.
/// This is a *missing directory*, NOT a jail escape: report it plainly (naming
/// the vanished path) so a user who moved their project folder isn't told to
/// "enable browse whole filesystem", which would not help. Keeps the two failure
/// modes — "your folder is gone" vs "that path is out of bounds" — distinct.
async fn missing_workspace_dir(state: &AppState, ws_id: &str) -> axum::response::Response {
    let cwd = state
        .store
        .get_workspace_by_id(ws_id.to_string())
        .await
        .ok()
        .flatten()
        .map(|w| w.cwd);
    let error = match cwd {
        Some(c) => format!("workspace directory no longer exists on disk: {c}"),
        None => "workspace directory is unavailable".to_string(),
    };
    (StatusCode::NOT_FOUND, Json(json!({ "error": error }))).into_response()
}

/// Hard denylist enforced on EVERY request — regardless of `workspace_id` or
/// the `all=1` "browse whole filesystem" toggle. The file browser is a dev
/// convenience, not a credential-exfiltration oracle: a local process (a rogue
/// MCP child, a malicious dependency, a landed XSS) must never be able to turn
/// `/api/files/read` into a reader for SSH keys, cloud creds, or the OAuth
/// token in `~/.claude.json`. Matched on the CANONICAL path (callers canon
/// first), so `..` / symlink tricks can't dodge it.
pub(crate) fn is_sensitive(path: &Path) -> bool {
    // Specific high-value files under $HOME: creds, tokens, and shell/REPL
    // histories (which routinely leak secrets typed on a command line).
    const FILES: &[&str] = &[
        ".claude.json",
        ".netrc",
        ".pgpass",
        ".git-credentials",
        ".npmrc",
        ".pypirc",
        ".bash_history",
        ".zsh_history",
        ".sh_history",
        ".python_history",
        ".node_repl_history",
        ".mysql_history",
        ".psql_history",
    ];
    // Credential directories — no legitimate "browse my code" reason to enter.
    const DIRS: &[&str] = &[
        ".ssh",
        ".aws",
        ".gnupg",
        ".kube",
        ".azure",
        ".docker",          // config.json holds registry auth tokens
        ".config/gcloud",
        ".config/gh",       // GitHub CLI OAuth token
        ".config/git",      // may hold credential stores
        "Library/Keychains", // macOS keychains
    ];
    if let Some(home) = home() {
        for rel in DIRS {
            if path.starts_with(home.join(rel)) {
                return true;
            }
        }
        for rel in FILES {
            if path == home.join(rel) {
                return true;
            }
        }
    }
    // Fail-closed backstop: when $HOME can't be resolved (packaged Windows
    // sidecar with neither HOME nor USERPROFILE, etc.) the anchored checks
    // above can't run, so match the $HOME FILES by bare basename anywhere —
    // `.claude.json` and friends are never a legitimate "browse my code"
    // target regardless of directory. This closes the credential-read hole
    // that would otherwise open when the process has no home.
    if let Some(fname) = path.file_name().and_then(|s| s.to_str()) {
        if FILES.contains(&fname) {
            return true;
        }
    }
    // Name-based denylist (covers private keys / env / credential stores
    // anywhere on disk). Kept narrow to avoid breaking legit source browsing:
    // `tokenizer.json`, `token.ts`, `secrets.example.ts` stay readable.
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name == ".env"
        || (name.starts_with(".env.") && !name.ends_with(".example") && !name.ends_with(".sample"))
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
        || name.ends_with(".ppk")
        || name.ends_with(".keystore")
        || name.ends_with(".jks")
        || name.contains("credential")
        || name == "id_rsa"
        || name == "id_dsa"
        || name == "id_ecdsa"
        || name == "id_ed25519"
}

fn sensitive_denied() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "path is on the sensitive-files denylist (credentials/keys are never served)"
        })),
    )
        .into_response()
}

pub async fn list_dir(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let ws_id = q.workspace_id.as_deref().filter(|s| !s.is_empty());
    // Unscoped (no workspace jail, or `all=1` escape) ⇒ full-disk reach. Only
    // the UI may do that; a headless local process is confined to a workspace.
    if (ws_id.is_none() || truthy(&q.all)) && !is_ui_request(&headers) {
        return unscoped_denied();
    }
    // Fetch the jail roots once; reuse them for both the default dir and the gate.
    let roots = match ws_id {
        Some(id) => allowed_roots(&state, id).await,
        None => Vec::new(),
    };
    // Scoped but no browsable root on disk ⇒ the workspace folder is gone. Report
    // that distinctly instead of falling through to the misleading jail error.
    if let Some(id) = ws_id {
        if !truthy(&q.all) && roots.is_empty() {
            return missing_workspace_dir(&state, id).await;
        }
    }
    let raw = match q.dir {
        Some(ref d) if !d.trim().is_empty() => PathBuf::from(d),
        // No dir: default to the workspace cwd (first root) when scoped, else
        // $HOME (falling back to `/` when home can't be resolved).
        _ => roots
            .first()
            .cloned()
            .or_else(home)
            .unwrap_or_else(|| PathBuf::from("/")),
    };
    let dir = match canon(&raw) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    };
    // Hard denylist first — never list a credential directory, even with all=1.
    if is_sensitive(&dir) {
        return sensitive_denied();
    }
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

/// Heuristic binary sniff for the text preview: any NUL byte ⇒ binary, so we
/// don't ship a non-text file back as mojibake. Scans the WHOLE preview slice
/// (caller passes `bytes[..min(len, MAX_READ_BYTES)]`, i.e. up to 512 KB), NOT a
/// fixed head window. The previous code only checked the first 8 KB, so a binary
/// whose first NUL fell past that head — a long ASCII/text preamble ahead of the
/// binary payload (PDF, many container formats), or simply NUL-free for >8 KB —
/// slipped through and was returned as garbage "text".
fn looks_binary(slice: &[u8]) -> bool {
    slice.contains(&0)
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
    headers: HeaderMap,
    Query(q): Query<ReadQuery>,
) -> impl IntoResponse {
    let ws_id = q.workspace_id.as_deref().filter(|s| !s.is_empty());
    // Unscoped (no workspace jail, or `all=1` escape) ⇒ arbitrary absolute
    // path. Only the UI may do that; a headless local process (curl, a rogue
    // MCP child, a landed exploit) is confined to a workspace's roots — this
    // is what closes the arbitrary-file-read oracle.
    if (ws_id.is_none() || truthy(&q.all)) && !is_ui_request(&headers) {
        return unscoped_denied();
    }
    let path = match canon(Path::new(&q.path)) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response(),
    };
    // Hard denylist first — credentials/keys are never served, even with all=1
    // or no workspace_id (the previously-unrestricted bare-call path).
    if is_sensitive(&path) {
        return sensitive_denied();
    }
    if let Some(id) = ws_id {
        if !truthy(&q.all) {
            let roots = allowed_roots(&state, id).await;
            if roots.is_empty() {
                return missing_workspace_dir(&state, id).await;
            }
            if !is_within_any(&path, &roots) {
                return jail_denied();
            }
        }
    }
    if !path.is_file() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("not a file: {}", path.display()) })),
        )
            .into_response();
    }
    // Bound the read to MAX_READ_BYTES *before* allocating: a multi-GB regular
    // file legitimately living in a workspace root (a log, DB dump, model
    // weight, video) would otherwise be slurped whole by `fs::read` and OOM the
    // server — killing every agent + the sidecar — before the truncation below
    // ever ran. Report the file's true size; only ship a capped preview.
    let real_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let bytes = {
        use std::io::Read;
        match std::fs::File::open(&path) {
            Ok(f) => {
                let mut buf = Vec::new();
                if let Err(e) = f.take(MAX_READ_BYTES as u64).read_to_end(&mut buf) {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({ "error": format!("read {}: {e}", path.display()) })),
                    )
                        .into_response();
                }
                buf
            }
            Err(e) => {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": format!("read {}: {e}", path.display()) })),
                )
                    .into_response()
            }
        }
    };
    let total = (real_size as usize).max(bytes.len());
    let truncated = total > MAX_READ_BYTES;
    let slice = &bytes[..bytes.len().min(MAX_READ_BYTES)];
    let binary = looks_binary(slice);
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
    fn looks_binary_scans_whole_slice_not_just_head() {
        // Pure text ⇒ not binary.
        assert!(!looks_binary(b"hello world, just text\n"));
        // NUL in the head ⇒ binary (the case the old code already caught).
        assert!(looks_binary(b"\0\x01\x02PNG"));
        // Regression: a NUL *past* the old 8 KB head window must still be caught.
        // 16 KB of ASCII, then a NUL — the old `take(8192)` check returned false.
        let mut buf = vec![b'a'; 16 * 1024];
        buf.push(0);
        assert!(looks_binary(&buf));
        // A long NUL-free ASCII run stays text (no false positive).
        assert!(!looks_binary(&vec![b'a'; 32 * 1024]));
        // Empty slice is text, not binary.
        assert!(!looks_binary(b""));
    }

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
    fn origin_host_is_local_distinguishes_ui_from_attacker() {
        // Real UI origins (vite dev, bundle on :7777, Tauri webview).
        assert!(origin_host_is_local("http://localhost:5173"));
        assert!(origin_host_is_local("http://127.0.0.1:7777"));
        assert!(origin_host_is_local("http://[::1]:7777"));
        assert!(origin_host_is_local("tauri://localhost"));
        assert!(origin_host_is_local("https://tauri.localhost"));
        // Anything off-box is not a UI request.
        assert!(!origin_host_is_local("http://evil.com"));
        assert!(!origin_host_is_local("https://attacker.example:443"));
        assert!(!origin_host_is_local("http://localhost.evil.com"));
    }

    /// Build a `HeaderMap` from `(name, value)` pairs for the guard tests.
    fn hdrs(pairs: &[(&str, &str)]) -> HeaderMap {
        use axum::http::{HeaderName, HeaderValue};
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn is_ui_request_accepts_browser_rejects_native_client() {
        // (1) Cross-origin fetch with a local Origin — Vite page / Tauri webview.
        assert!(is_ui_request(&hdrs(&[("origin", "http://localhost:5173")])));
        assert!(is_ui_request(&hdrs(&[("origin", "tauri://localhost")])));

        // (2) Same-origin browser fetch: no Origin, but Fetch-Metadata says
        // same-origin and Host is loopback (the create-workspace path-probe and
        // the in-app file browser — the case this fix restores).
        assert!(is_ui_request(&hdrs(&[
            ("sec-fetch-site", "same-origin"),
            ("host", "127.0.0.1:7777"),
        ])));
        assert!(is_ui_request(&hdrs(&[
            ("sec-fetch-site", "same-origin"),
            ("host", "localhost:5173"),
        ])));

        // A bare native client (curl / reqwest) sends neither Origin nor
        // Sec-Fetch-* — it must stay confined and NOT get full-disk reads.
        assert!(!is_ui_request(&hdrs(&[("host", "127.0.0.1:7777")])));
        assert!(!is_ui_request(&HeaderMap::new()));

        // Non-same-origin fetch metadata is not the in-app UI.
        assert!(!is_ui_request(&hdrs(&[
            ("sec-fetch-site", "cross-site"),
            ("host", "127.0.0.1:7777"),
        ])));
        assert!(!is_ui_request(&hdrs(&[
            ("sec-fetch-site", "same-site"),
            ("host", "127.0.0.1:7777"),
        ])));

        // Defense-in-depth: same-origin metadata with a NON-loopback Host (a
        // no-Origin DNS-rebind shape) must never unlock full-disk reads.
        assert!(!is_ui_request(&hdrs(&[
            ("sec-fetch-site", "same-origin"),
            ("host", "attacker.com"),
        ])));

        // A non-local Origin is never the UI (require_local_origin 403s it too).
        assert!(!is_ui_request(&hdrs(&[("origin", "http://evil.com")])));
    }

    #[test]
    fn env_variants_blocked_but_examples_readable() {
        assert!(is_sensitive(Path::new("/proj/.env")));
        assert!(is_sensitive(Path::new("/proj/.env.local")));
        assert!(is_sensitive(Path::new("/proj/.env.production")));
        assert!(!is_sensitive(Path::new("/proj/.env.example")));
        assert!(!is_sensitive(Path::new("/proj/.env.sample")));
    }

    #[test]
    fn truthy_parses_query_strings() {
        assert!(truthy(&Some("1".into())));
        assert!(truthy(&Some("true".into())));
        assert!(!truthy(&Some("0".into())));
        assert!(!truthy(&Some(String::new())));
        assert!(!truthy(&None));
    }

    #[test]
    fn is_sensitive_blocks_credentials_not_source() {
        let home = home().unwrap_or_else(|| PathBuf::from("/home/tester"));
        // Credential dirs / files denied wherever they resolve under $HOME.
        assert!(is_sensitive(&home.join(".ssh/id_rsa")));
        assert!(is_sensitive(&home.join(".aws/credentials")));
        assert!(is_sensitive(&home.join(".claude.json")));
        assert!(is_sensitive(&home.join(".git-credentials")));
        // Fail-closed backstop: $HOME creds caught by basename anywhere, so a
        // missing/bogus HOME can't turn them into a readable oracle.
        assert!(is_sensitive(Path::new("/.claude.json")));
        assert!(is_sensitive(Path::new("/tmp/whatever/.claude.json")));
        // Name-based: private keys / env / credential stores anywhere on disk.
        assert!(is_sensitive(Path::new("/anywhere/server.pem")));
        assert!(is_sensitive(Path::new("/x/private.key")));
        assert!(is_sensitive(Path::new("/x/cert.p12")));
        assert!(is_sensitive(Path::new("/proj/.env")));
        assert!(is_sensitive(Path::new("/x/aws-credentials.txt")));
        assert!(is_sensitive(Path::new("/somewhere/id_ed25519")));
        // Legit source browsing must NOT be broken.
        assert!(!is_sensitive(Path::new("/proj/src/main.rs")));
        assert!(!is_sensitive(Path::new("/proj/tokenizer.json")));
        assert!(!is_sensitive(Path::new("/proj/README.md")));
        assert!(!is_sensitive(Path::new("/proj/.env.example")));
    }
}
