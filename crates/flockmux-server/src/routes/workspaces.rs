//! Workspace, root, and direction REST endpoints.
//!
//! This module owns the workspace-as-first-class surface and the git-worktree
//! direction lifecycle. Agent spawning and spell execution stay in `rest`; this
//! module calls those explicit seams when a workspace action needs them.

use super::rest::{
    run_spell, spawn_bootstrap_inject, spawn_with_bookkeeping, teardown_agent, BootstrapCtx,
};
use crate::spawn::WorkspaceLayout;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use flockmux_protocol::rest::{
    BranchInfo, CreateThreadRequest, CreateWorkspaceRequest, RunSpellRequest, ThreadInfo,
    UpdateThreadRequest, Workspace, WorkspaceRoot,
};
use flockmux_protocol::ws_swarm::SwarmEvent;
use flockmux_storage::{NewThread, NewWorkspace, NewWorkspaceRoot, ThreadRecord};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Map a storage `ThreadRecord` onto the wire `ThreadInfo` (drops the internal
/// `deleted_at`; the API only ever surfaces alive threads).
fn thread_record_to_info(t: ThreadRecord) -> ThreadInfo {
    ThreadInfo {
        id: t.id,
        workspace_id: t.workspace_id,
        slug: t.slug,
        name: t.name,
        isolation: t.isolation,
        branch: t.branch,
        cwd: t.cwd,
        state: t.state,
        model_tier: t.model_tier,
        reasoning_effort: t.reasoning_effort,
        dirty: false, // live value folded in by list_workspaces_handler
        ahead: None,  // live values folded in by list_workspaces_handler
        behind: None,
        created_at: t.created_at,
    }
}

// ────────────────────────────────────────────────────────────────────────
// Workspace endpoints (Step 2 of workspace-as-first-class rollout)
// ────────────────────────────────────────────────────────────────────────

/// `POST /api/workspaces` — create a new workspace and return the
/// persisted row. CreateWizard calls this *before* launching the `init`
/// spell so the spell's spawned scout already carries `workspace_id`.
pub async fn create_workspace_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<Json<Workspace>, (StatusCode, Json<serde_json::Value>)> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must be non-empty"})),
        ));
    }
    if req.cwd.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "cwd must be non-empty"})),
        ));
    }
    // Validate the cwd BEFORE persisting the row. Otherwise we'd create the
    // workspace, then the `init` spell's "create shared workspace" step fails
    // because the directory can't be entered — leaving the user with a dead,
    // 0-member ghost workspace pointing at a bad path. A 4xx here keeps the DB
    // clean and surfaces a plain "doesn't exist" message instead of a 500.
    {
        let path = std::path::Path::new(req.cwd.trim());
        if !path.exists() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("directory does not exist: {}", req.cwd.trim())})),
            ));
        }
        if !path.is_dir() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("not a directory: {}", req.cwd.trim())})),
            ));
        }
    }
    let rec = state
        .store
        .create_workspace(
            NewWorkspace {
                name: req.name,
                cwd: req.cwd,
                accent: req.accent,
            },
            now_ms(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    // Auto-create the workspace's `main` direction so every workspace owns at
    // least one thread from birth. Zero-friction: shared isolation, cwd = the
    // workspace cwd, ready immediately. A store failure here is non-fatal —
    // agents simply fall back to the legacy `thread_id = None` (= main) path —
    // so we log and return an empty thread list rather than 500 the creation.
    let threads: Vec<ThreadInfo> = match state
        .store
        .create_thread(
            NewThread {
                workspace_id: rec.id.clone(),
                slug: "main".to_string(),
                name: Some("main".to_string()),
                isolation: "shared".to_string(),
                branch: None,
                cwd: rec.cwd.clone(),
                state: "ready".to_string(),
            },
            now_ms(),
        )
        .await
    {
        Ok(t) => vec![thread_record_to_info(t)],
        Err(e) => {
            tracing::warn!(?e, workspace = %rec.id, "create main thread failed; agents fall back to main = None");
            Vec::new()
        }
    };

    // Attach any dependency-source roots the wizard sent. Each is validated
    // the same way as the primary cwd above (exists + is a dir → 4xx), then
    // persisted. The workspace row already exists at this point; a failed root
    // insert returns 500 without rolling back the workspace (acceptable — the
    // user can re-attach the root). Empty/whitespace paths are skipped.
    let mut roots: Vec<WorkspaceRoot> = Vec::new();
    for root in req.roots {
        let p = root.path.trim();
        if p.is_empty() {
            continue;
        }
        let path = std::path::Path::new(p);
        if !path.exists() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("dependency directory does not exist: {}", p)})),
            ));
        }
        if !path.is_dir() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("dependency path is not a directory: {}", p)})),
            ));
        }
        // The wizard only ever creates the primary + peers + under-primary
        // deps, so every root it sends is a top-level node (parent_id=None).
        // Any client-supplied id is ignored — the server mints it.
        let saved = state
            .store
            .add_workspace_root(
                NewWorkspaceRoot {
                    workspace_id: rec.id.clone(),
                    path: p.to_string(),
                    role: root.role,
                    label: root.label,
                    parent_id: None,
                },
                now_ms(),
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;
        roots.push(WorkspaceRoot {
            id: saved.id,
            path: saved.path,
            role: saved.role,
            label: saved.label,
            parent_id: saved.parent_id,
            branch: None, // filled at list time
        });
    }

    // If any tree nodes were attached, write a flockmux-managed context block
    // into the primary project dir so the spawned orchestrator (claude →
    // CLAUDE.md, codex → AGENTS.md) reads the attached source directly instead
    // of decompiling/guessing. Best-effort; never fatal.
    if !roots.is_empty() {
        write_workspace_deps_context(rec.cwd.trim(), &rec.name, &roots);
    }

    Ok(Json(Workspace {
        id: rec.id,
        slug: rec.slug,
        name: rec.name,
        cwd: rec.cwd,
        cwd_branch: None, // filled at list time
        accent: rec.accent,
        created_at: rec.created_at,
        member_count: 0,
        roots,
        threads,
    }))
}

/// Write/refresh a flockmux-managed "workspace structure" block into the
/// workspace's CLAUDE.md and AGENTS.md so the orchestrator reads the attached
/// source directly (best practice: a per-project context file) instead of
/// decompiling/guessing. The block renders the workspace's user-defined
/// LOGICAL tree: the primary project (`cwd` + `name`) plus every attached
/// node, nested by `parent_id`. Idempotent: the block is delimited by
/// HTML-comment markers and replaced in place on re-write; any user content
/// outside the markers is preserved. When `roots` is empty the managed block
/// is STRIPPED instead of written (the inverse path — used when the last
/// attached node is removed), leaving any surrounding user content intact.
/// Best-effort — failures are logged, never fatal.
fn write_workspace_deps_context(cwd: &str, name: &str, roots: &[WorkspaceRoot]) {
    use std::fmt::Write as _;
    const START: &str = "<!-- flockmux:deps:start -->";
    const END: &str = "<!-- flockmux:deps:end -->";

    // No roots left → strip the managed block (and trailing blank lines)
    // from each context file if present. We never create a file here; only
    // existing files with a managed block are rewritten.
    if roots.is_empty() {
        for fname in ["CLAUDE.md", "AGENTS.md"] {
            let path = std::path::Path::new(cwd).join(fname);
            let existing = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue, // file doesn't exist / unreadable — nothing to strip
            };
            if let (Some(s), Some(e)) = (existing.find(START), existing.find(END)) {
                let end_full = e + END.len();
                // Drop the block plus any trailing newlines that followed it
                // so we don't leave a dangling blank gap behind.
                let after = existing[end_full..].trim_start_matches(['\n', '\r']);
                let before = &existing[..s];
                // If the block was the only content, `before` is empty / blank
                // and the file becomes empty — that's fine per spec.
                let stripped = if after.is_empty() {
                    before.trim_end().to_string()
                } else {
                    format!("{}{}", before, after)
                };
                if stripped.trim().is_empty() {
                    // The managed block was the file's only content — delete the
                    // now-empty file instead of leaving a dangling 0-byte
                    // CLAUDE.md/AGENTS.md behind (flockmux created it, flockmux
                    // cleans it up). If the user had their own content around the
                    // block, `stripped` is non-empty and we keep+rewrite it.
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            tracing::info!(file = %path.display(), "removed empty workspace deps context file (no roots left)")
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            tracing::warn!(?e, file = %path.display(), "failed removing empty workspace deps context file")
                        }
                    }
                } else if let Err(e) = std::fs::write(&path, stripped) {
                    tracing::warn!(?e, file = %path.display(), "failed stripping workspace deps context");
                } else {
                    tracing::info!(file = %path.display(), "stripped workspace deps context (no roots left)");
                }
            }
        }
        return;
    }

    // Render the prefix label for one tree node by role.
    fn node_label(role: &str) -> &'static str {
        match role {
            "project" => "项目",
            "tool" => "[工具]",
            _ => "[依赖]",
        }
    }
    // Emit `node` (and recurse into its children) at the given depth. Children
    // are the roots whose parent_id == node.id, in slice order (already sorted
    // by created_at by the caller). `depth` controls the 2-space indent.
    fn emit_node(block: &mut String, node: &WorkspaceRoot, roots: &[WorkspaceRoot], depth: usize) {
        let indent = "  ".repeat(depth);
        let label = node_label(&node.role);
        let name = node.label.as_deref().unwrap_or("");
        let _ = if name.is_empty() {
            writeln!(block, "{indent}- {label} `{}`", node.path)
        } else {
            writeln!(block, "{indent}- {label} {name} `{}`", node.path)
        };
        for child in roots
            .iter()
            .filter(|r| r.parent_id.as_deref() == Some(node.id.as_str()))
        {
            emit_node(block, child, roots, depth + 1);
        }
    }

    let mut block = String::new();
    let _ = writeln!(block, "{START}");
    let _ = writeln!(block, "## 工作空间结构 (flockmux managed)");
    let _ = writeln!(block);
    let _ = writeln!(
        block,
        "下面是本工作空间的项目与它们挂载的依赖源码（树中父子表示\"依赖/归属\"，物理\
         路径见每行）。开发时直接阅读/按需修改这些源码——不要反编译 jar/包、不要凭\
         猜测。改动跨项目的共享库时注意它可能被多处使用。"
    );
    let _ = writeln!(block);

    // The PRIMARY project = (cwd, name, role="project"), implicit root.
    // Its children = roots with parent_id=None && role!="project".
    // Top-level peer projects = roots with parent_id=None && role=="project".
    let _ = writeln!(block, "- 项目 {name} `{cwd}`   (primary)");
    for r in roots
        .iter()
        .filter(|r| r.parent_id.is_none() && r.role != "project")
    {
        emit_node(&mut block, r, roots, 1);
    }
    for r in roots
        .iter()
        .filter(|r| r.parent_id.is_none() && r.role == "project")
    {
        emit_node(&mut block, r, roots, 0);
    }
    let _ = write!(block, "{END}");

    for fname in ["CLAUDE.md", "AGENTS.md"] {
        let path = std::path::Path::new(cwd).join(fname);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        // Is everything OUTSIDE our managed block blank? Then flockmux authored
        // the whole file (created it or it was empty before) — safe to local-
        // exclude so a multi-root workspace doesn't show a perpetual false dirty
        // dot. If the user had their own content (append case), DON'T exclude —
        // we'd hide their real CLAUDE.md.
        let flockmux_only = match (existing.find(START), existing.find(END)) {
            (Some(s), Some(e)) => {
                existing[..s].trim().is_empty() && existing[e + END.len()..].trim().is_empty()
            }
            _ => existing.trim().is_empty(),
        };
        let next = if let (Some(s), Some(e)) = (existing.find(START), existing.find(END)) {
            // replace existing managed block in place
            let end_full = e + END.len();
            format!("{}{}{}", &existing[..s], block, &existing[end_full..])
        } else if existing.trim().is_empty() {
            block.clone()
        } else {
            format!("{}\n\n{}\n", existing.trim_end(), block)
        };
        if let Err(e) = std::fs::write(&path, next) {
            tracing::warn!(?e, file = %path.display(), "failed writing workspace deps context");
        } else {
            tracing::info!(file = %path.display(), roots = roots.len(), "wrote workspace deps context");
            if flockmux_only {
                crate::worktree::ignore_paths_locally(std::path::Path::new(cwd), &[fname]);
            }
        }
    }
}

/// `GET /api/workspaces` — list alive workspaces with their live member
/// counts (alive agents whose `workspace_id` points here). Soft-deleted
/// rows are excluded.
pub async fn list_workspaces_handler(State(state): State<AppState>) -> impl IntoResponse {
    let rows = match state.store.list_workspaces(false).await {
        Ok(rs) => rs,
        Err(e) => {
            tracing::warn!(?e, "list_workspaces failed");
            return Json(Vec::<Workspace>::new());
        }
    };
    // Compute member_count from list_agents instead of per-workspace SQL
    // queries — there are typically <100 agents total, so a single pass
    // beats N+1 SELECTs and keeps the store API smaller.
    let agents = state.store.list_agents().await.unwrap_or_default();
    let mut counts: HashMap<String, i64> = HashMap::new();
    for a in agents {
        if a.killed_at.is_some() {
            continue;
        }
        if let Some(ws_id) = a.workspace_id {
            *counts.entry(ws_id).or_insert(0) += 1;
        }
    }
    // Fetch every attached root in one shot and group by workspace_id (rows
    // come back ordered by created_at ASC, so each group preserves attach
    // order). Same single-pass rationale as member_count above — avoids N+1.
    let mut roots_by_ws: HashMap<String, Vec<WorkspaceRoot>> = HashMap::new();
    for r in state
        .store
        .list_all_workspace_roots()
        .await
        .unwrap_or_default()
    {
        roots_by_ws
            .entry(r.workspace_id)
            .or_default()
            .push(WorkspaceRoot {
                id: r.id,
                path: r.path,
                role: r.role,
                label: r.label,
                parent_id: r.parent_id,
                branch: None, // filled below from branch_map
            });
    }
    // Threads (directions) per workspace. Unlike roots there's no "list all"
    // query; with a handful of workspaces locally, one list_threads each is
    // cheap and keeps the store API minimal. Oldest-first (main leads).
    let mut threads_by_ws: HashMap<String, Vec<ThreadInfo>> = HashMap::new();
    for r in &rows {
        let list = state
            .store
            .list_threads(r.id.clone())
            .await
            .unwrap_or_default();
        threads_by_ws.insert(
            r.id.clone(),
            list.into_iter().map(thread_record_to_info).collect(),
        );
    }
    // Live git branch per path (workspace cwds + every attached root), for the
    // sidebar's branch chips. Batched off the async runtime (git shells out and
    // blocks) and memoized with a short TTL so the frequent workspaces refetch
    // doesn't re-run git every time. Best-effort: on a join error every chip is
    // simply absent.
    let mut paths: Vec<PathBuf> = rows.iter().map(|r| PathBuf::from(&r.cwd)).collect();
    for roots in roots_by_ws.values() {
        paths.extend(roots.iter().map(|rt| PathBuf::from(&rt.path)));
    }
    // Thread cwds too — a worktree direction has its own dir, so each direction
    // gets a live dirty flag (not just the shared workspace cwd).
    for threads in threads_by_ws.values() {
        paths.extend(threads.iter().map(|t| PathBuf::from(&t.cwd)));
    }
    let git_map = tokio::task::spawn_blocking(move || git_status_for_paths(&paths))
        .await
        .unwrap_or_default();
    // Fold the computed branch into the attached roots.
    for roots in roots_by_ws.values_mut() {
        for rt in roots.iter_mut() {
            rt.branch = git_map
                .get(std::path::Path::new(&rt.path))
                .and_then(|g| g.branch.clone());
        }
    }
    // Fold the live dirty flag + branch into each direction (keyed by its cwd).
    // dirty is always live; branch is only filled when the DB row hasn't already
    // recorded one (an isolated worktree keeps its stored branch — its cwd's git
    // HEAD is the same branch anyway, but the stored value is canonical).
    for threads in threads_by_ws.values_mut() {
        for t in threads.iter_mut() {
            if let Some(g) = git_map.get(std::path::Path::new(&t.cwd)) {
                t.dirty = g.dirty;
                if t.branch.is_none() {
                    t.branch = g.branch.clone();
                }
            }
        }
    }
    // ahead/behind: how far each isolated direction's branch has diverged from
    // its workspace's base branch (the main worktree's current branch). Purely
    // local (no fetch). Only worktree directions with a branch != base qualify,
    // so a plain main-only workspace adds ZERO git calls (the common case stays
    // free); the rev-list runs off the async runtime, keyed by thread id.
    let ab_input: Vec<(String, String, String, String)> = rows
        .iter()
        .filter_map(|r| {
            let base = git_map
                .get(std::path::Path::new(&r.cwd))
                .and_then(|g| g.branch.clone())?;
            let threads = threads_by_ws.get(&r.id)?;
            Some(threads.iter().filter_map(move |t| {
                let br = t.branch.as_deref().filter(|b| !b.is_empty())?;
                (t.isolation == "worktree" && br != base)
                    .then(|| (r.cwd.clone(), base.clone(), br.to_string(), t.id.clone()))
            }))
        })
        .flatten()
        .collect();
    if !ab_input.is_empty() {
        let ab_map = tokio::task::spawn_blocking(move || {
            let mut m: std::collections::HashMap<String, (i64, i64)> =
                std::collections::HashMap::new();
            for (cwd, base, branch, tid) in ab_input {
                if let Some(ab) =
                    crate::worktree::ahead_behind(std::path::Path::new(&cwd), &base, &branch)
                {
                    m.insert(tid, ab);
                }
            }
            m
        })
        .await
        .unwrap_or_default();
        for threads in threads_by_ws.values_mut() {
            for t in threads.iter_mut() {
                if let Some(&(ahead, behind)) = ab_map.get(&t.id) {
                    t.ahead = Some(ahead);
                    t.behind = Some(behind);
                }
            }
        }
    }
    let items: Vec<Workspace> = rows
        .into_iter()
        .map(|r| Workspace {
            member_count: counts.get(&r.id).copied().unwrap_or(0),
            roots: roots_by_ws.remove(&r.id).unwrap_or_default(),
            threads: threads_by_ws.remove(&r.id).unwrap_or_default(),
            cwd_branch: git_map
                .get(std::path::Path::new(&r.cwd))
                .and_then(|g| g.branch.clone()),
            id: r.id,
            slug: r.slug,
            name: r.name,
            cwd: r.cwd,
            accent: r.accent,
            created_at: r.created_at,
        })
        .collect();
    Json(items)
}

/// Live git facts for a path: the checked-out branch (for the sidebar chip) and
/// whether the work tree is dirty (uncommitted changes). Both computed at list
/// time, never persisted.
#[derive(Clone, Default)]
pub(crate) struct PathGit {
    branch: Option<String>,
    dirty: bool,
}

/// Live git status for each of `paths`, memoized with a short TTL. Shelling out
/// to git is blocking, so this runs under `spawn_blocking`; the TTL keeps the
/// hot workspaces-list refetch (fired on every thread/agent event) from
/// re-invoking git each time. `branch` is `Some` only for a git work tree on a
/// named branch (else no chip); `dirty` is best-effort (`false` on any error).
fn git_status_for_paths(paths: &[PathBuf]) -> HashMap<PathBuf, PathGit> {
    use parking_lot::Mutex;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};
    const TTL: Duration = Duration::from_secs(3);
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (PathGit, Instant)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let mut out: HashMap<PathBuf, PathGit> = HashMap::new();
    for p in paths {
        if out.contains_key(p) {
            continue; // de-dup: a path can appear as both a cwd and a root
        }
        if let Some((g, at)) = cache.lock().get(p) {
            if at.elapsed() < TTL {
                out.insert(p.clone(), g.clone());
                continue;
            }
        }
        // One git call each — both are read-only; `working_dirty` never
        // disturbs an agent mid-edit. status --porcelain is heavier than
        // rev-parse, but the TTL bounds how often it runs per path.
        let g = PathGit {
            branch: crate::worktree::current_branch(p),
            dirty: crate::worktree::working_dirty(p),
        };
        cache.lock().insert(p.clone(), (g.clone(), Instant::now()));
        out.insert(p.clone(), g);
    }
    out
}

// ── threads (directions) ─────────────────────────────────────────────────

/// Ensure `base` is unique among a workspace's ALIVE thread slugs, appending
/// `-2`, `-3`, … on collision. Best-effort: on a list error we return `base`
/// and let the DB's unique index reject a genuine dup.
async fn unique_thread_slug(state: &AppState, workspace_id: &str, base: &str) -> String {
    let existing: std::collections::HashSet<String> = state
        .store
        .list_threads(workspace_id.to_string())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.slug)
        .collect();
    if !existing.contains(base) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let cand = format!("{base}-{n}");
        if !existing.contains(&cand) {
            return cand;
        }
        n += 1;
    }
}

/// `GET /api/workspaces/:id/threads` — list a workspace's directions
/// (oldest-first; the first entry is the auto-created `main`).
pub async fn list_threads_handler(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> impl IntoResponse {
    let list = state
        .store
        .list_threads(workspace_id)
        .await
        .unwrap_or_default();
    Json(
        list.into_iter()
            .map(thread_record_to_info)
            .collect::<Vec<_>>(),
    )
}

/// `GET /api/workspaces/:id/branches` — local git branches of the workspace
/// cwd, each flagged whether it's already checked out (so the "open existing
/// branch" picker can disable those — a checked-out branch can't be attached to
/// a new worktree). Empty for a non-git workspace. Off the async runtime (git
/// blocks).
pub async fn list_branches_handler(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> impl IntoResponse {
    let ws = match state.store.get_workspace_by_id(workspace_id).await {
        Ok(Some(w)) => w,
        _ => return Json(Vec::<BranchInfo>::new()),
    };
    let cwd = PathBuf::from(ws.cwd);
    let branches = tokio::task::spawn_blocking(move || crate::worktree::list_branches(&cwd))
        .await
        .unwrap_or_default();
    Json(
        branches
            .into_iter()
            .map(|(name, checked_out)| BranchInfo { name, checked_out })
            .collect::<Vec<_>>(),
    )
}

/// `POST /api/workspaces/:id/threads` — open a new direction. Zero-friction:
/// `name` optional; created `shared` + `ready` in the workspace's own cwd. Git
/// isolation is DEFERRED until the direction is named (see
/// `update_thread_handler`) so clicking "+ new direction" never blocks on git.
/// The slug is FIXED at creation and never changes — renaming only moves the
/// display name + git branch, so already-spawned agents' blackboard keys
/// (`{workspace_id}/{slug}/…`) stay valid.
pub async fn create_thread_handler(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateThreadRequest>,
) -> Result<Json<ThreadInfo>, (StatusCode, Json<serde_json::Value>)> {
    let ws = state
        .store
        .get_workspace_by_id(workspace_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown workspace_id: {workspace_id}")})),
            )
        })?;
    let name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    // "Open an existing branch as a direction" — flockmux's worktree-native take
    // on switching branches. When set, the direction's display name/slug default
    // to the branch and the worktree ATTACHES this exact branch instead of
    // creating a fresh one (see `branch` below; `worktree_add` falls back to
    // attach when `-b` fails because the branch exists).
    let open_branch = req
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let display_name = name.or(open_branch);
    // A name (if any) yields a readable, stable slug; otherwise a short random
    // placeholder. Slug is fixed for the thread's life (see doc above).
    let base_slug = match display_name {
        Some(n) => crate::worktree::sanitize_suffix(n),
        None => format!("t-{}", &Uuid::new_v4().to_string()[..6]),
    };
    let slug = unique_thread_slug(&state, &workspace_id, &base_slug).await;
    // A direction named up-front (the "新方向" dialog's name field) must isolate
    // exactly like one the orchestrator names from the first message —
    // otherwise filling that field silently defeats worktree isolation (thread
    // stays `shared`, both directions edit one cwd and clobber each other).
    // Named, non-main → create `preparing` + kick off background `worktree add`;
    // the frontend waits for `ready` before spawning the orchestrator so it
    // lands in the worktree. Unnamed keeps the zero-friction shared/ready path
    // (orchestrator isolates on its first-message naming).
    let will_isolate = display_name.is_some();
    // Worktree binds to the exact existing branch when opening one; otherwise a
    // fresh branch named after the slug.
    let branch = open_branch
        .map(|b| b.to_string())
        .unwrap_or_else(|| slug.clone());
    let rec = state
        .store
        .create_thread(
            NewThread {
                workspace_id: workspace_id.clone(),
                slug,
                name: display_name.map(|s| s.to_string()),
                isolation: "shared".to_string(),
                branch: None,
                cwd: ws.cwd.clone(),
                state: if will_isolate { "preparing" } else { "ready" }.to_string(),
            },
            now_ms(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    let info = thread_record_to_info(rec);
    if will_isolate {
        // Background git takeover + `worktree add` → flips isolation/cwd/state
        // to worktree/<dir>/ready on success, degrades to shared/ready on any
        // failure (so a non-git project or a git hiccup never blocks the
        // direction — it just stays unisolated, surfaced in the sidebar).
        spawn_thread_worktree(
            state.clone(),
            info.id.clone(),
            workspace_id.clone(),
            ws.cwd.clone(),
            branch,
        );
    }
    publish_thread_changed(&state, &workspace_id, &info.id, "created");
    Ok(Json(info))
}

/// Broadcast that a workspace's direction (thread) list changed so subscribers
/// (the sidebar) refetch `/api/workspaces`. The REST snapshot stays the source
/// of truth; this is just the "now" signal for a change the snapshot can't push
/// itself — notably `swarm_name_thread` → background worktree isolation, which
/// renames + flips the branch icon without any UI-initiated request.
fn publish_thread_changed(state: &AppState, workspace_id: &str, thread_id: &str, op: &str) {
    state.swarm.publish_event(SwarmEvent::ThreadChanged {
        workspace_id: workspace_id.to_string(),
        thread_id: thread_id.to_string(),
        op: op.to_string(),
    });
}

// ── merge a direction back into the main line ────────────────────────────────

#[derive(serde::Serialize)]
pub struct ThreadDiffResponse {
    /// Base branch = the branch currently checked out at the workspace cwd.
    pub base: Option<String>,
    /// The direction's own branch (None for a shared/non-isolated direction).
    pub branch: Option<String>,
    /// Repo-relative files this direction changed vs the merge-base.
    pub files: Vec<String>,
    /// Whether the base work tree has uncommitted changes — a merge would be
    /// refused, so the UI warns and disables the merge button.
    pub base_dirty: bool,
}

/// `GET /api/workspaces/:id/threads/:tid/diff` — preview "what this direction
/// changed" before merging. Read-only; never touches the work tree.
pub async fn thread_diff_handler(
    State(state): State<AppState>,
    Path((workspace_id, thread_id)): Path<(String, String)>,
) -> Result<Json<ThreadDiffResponse>, (StatusCode, Json<serde_json::Value>)> {
    let ws = require_workspace(&state, &workspace_id).await?;
    let th = require_thread(&state, &thread_id).await?;
    let branch = th.branch.clone().filter(|b| !b.is_empty());
    let cwd = std::path::PathBuf::from(&ws.cwd);
    let branch_for_diff = branch.clone();
    let (base, files, base_dirty) = tokio::task::spawn_blocking(move || {
        let base = crate::worktree::current_branch(&cwd);
        let files = match (&base, &branch_for_diff) {
            (Some(b), Some(f)) => crate::worktree::diff_summary(&cwd, b, f),
            _ => Vec::new(),
        };
        let dirty = crate::worktree::working_dirty(&cwd);
        (base, files, dirty)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;
    Ok(Json(ThreadDiffResponse {
        base,
        branch,
        files,
        base_dirty,
    }))
}

#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MergeResponse {
    /// Merged cleanly into `base`; `files` is the direction's changed-file count.
    Merged { base: String, files: usize },
    /// Conflicts; an AI resolver agent was spawned to finish the merge.
    Resolving {
        agent_id: String,
        files: Vec<String>,
    },
}

/// `POST /api/workspaces/:id/threads/:tid/merge` — merge a direction's branch
/// back into the main line. Clean → done. Conflict → spawn an AI agent in the
/// main worktree to resolve + commit (the user just sees "AI is reconciling").
pub async fn merge_thread_handler(
    State(state): State<AppState>,
    Path((workspace_id, thread_id)): Path<(String, String)>,
) -> Result<Json<MergeResponse>, (StatusCode, Json<serde_json::Value>)> {
    let ws = require_workspace(&state, &workspace_id).await?;
    let th = require_thread(&state, &thread_id).await?;
    let branch = match th.branch.clone().filter(|b| !b.is_empty()) {
        Some(b) if th.isolation == "worktree" => b,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "该方向没有独立分支，无需合并"})),
            ));
        }
    };
    let cwd = std::path::PathBuf::from(&ws.cwd);

    // All git work on a blocking thread (shells out). Returns the base branch +
    // the merge outcome, or a user-facing refusal string.
    let branch_for_git = branch.clone();
    let result = tokio::task::spawn_blocking(move || {
        if crate::worktree::working_dirty(&cwd) {
            return Err("主线有未提交改动，请先提交或暂存后再合并".to_string());
        }
        let base = crate::worktree::current_branch(&cwd)
            .ok_or_else(|| "主线处于游离 HEAD，无法合并".to_string())?;
        let outcome = crate::worktree::merge_into_base(&cwd, &base, &branch_for_git);
        Ok((base, outcome))
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;
    let (base, outcome) = match result {
        Ok(v) => v,
        // A clean refusal (dirty base / detached HEAD) — 409 so the UI shows the
        // reason rather than a generic error.
        Err(msg) => return Err((StatusCode::CONFLICT, Json(json!({"error": msg})))),
    };

    match outcome {
        crate::worktree::MergeOutcome::Clean { files } => {
            Ok(Json(MergeResponse::Merged { base, files }))
        }
        crate::worktree::MergeOutcome::Conflict { files } => {
            let agent_id = spawn_merge_resolver(&state, &ws, &base, &branch, &files)
                .await
                .map_err(|(s, m)| (s, Json(json!({"error": m}))))?;
            Ok(Json(MergeResponse::Resolving { agent_id, files }))
        }
        crate::worktree::MergeOutcome::Error { msg } => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("合并失败：{msg}")})),
        )),
    }
}

async fn require_workspace(
    state: &AppState,
    workspace_id: &str,
) -> Result<flockmux_storage::WorkspaceRecord, (StatusCode, Json<serde_json::Value>)> {
    state
        .store
        .get_workspace_by_id(workspace_id.to_string())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown workspace_id: {workspace_id}")})),
            )
        })
}

async fn require_thread(
    state: &AppState,
    thread_id: &str,
) -> Result<flockmux_storage::ThreadRecord, (StatusCode, Json<serde_json::Value>)> {
    state
        .store
        .get_thread(thread_id.to_string())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown thread_id: {thread_id}")})),
            )
        })
}

/// Pick the CLI for the merge-resolver: prefer this workspace's live
/// orchestrator (so it matches what the user configured), else any live agent,
/// else "claude" (the safe default — conflict resolution wants a strong model).
async fn resolver_cli(state: &AppState, workspace_id: &str) -> String {
    if let Ok(agents) = state.store.list_agents().await {
        let alive = |a: &&flockmux_storage::AgentRecord| {
            a.workspace_id.as_deref() == Some(workspace_id)
                && a.killed_at.is_none()
                && a.shim_exit_at.is_none()
        };
        if let Some(a) = agents.iter().find(|a| alive(a) && a.role == "orchestrator") {
            return a.cli.clone();
        }
        if let Some(a) = agents.iter().find(alive) {
            return a.cli.clone();
        }
    }
    "claude".to_string()
}

/// Spawn a one-shot agent in the MAIN worktree (ws.cwd, currently mid-merge with
/// conflict markers + MERGE_HEAD) to resolve the conflicts and commit. Lands it
/// in the main direction (thread_id = main) so it operates on the primary
/// worktree, not the direction being merged.
async fn spawn_merge_resolver(
    state: &AppState,
    ws: &flockmux_storage::WorkspaceRecord,
    base: &str,
    branch: &str,
    files: &[String],
) -> Result<String, (StatusCode, String)> {
    let main_thread_id = state
        .store
        .list_threads(ws.id.clone())
        .await
        .ok()
        .and_then(|ts| ts.into_iter().find(|t| t.slug == "main").map(|t| t.id));
    let cli = resolver_cli(state, &ws.id).await;
    let files_list = files.join("、");
    let system_prompt = format!(
        "你正在解决一次 git merge 冲突：把分支 `{branch}` 合并进 `{base}` 时与主线已有改动撞车了。\n\
         冲突文件：{files_list}。\n\
         逐个打开这些文件，理解两边各自想做什么，消除所有 `<<<<<<<` / `=======` / `>>>>>>>` 冲突标记，\
         保留两边都合理的意图（不是无脑选一边）。改完后对这些文件 `git add`，再 `git commit`（保留默认的 \
         merge commit 信息）完成合并。\n\
         完成后用 swarm_send_message 给 `user` 发一句话，说明你把什么和什么调和了；若某处你不确定，也在这句话里点出来。然后停止。\n\
         只动这些冲突文件，不要改与本次冲突无关的任何东西。"
    );
    let layout = WorkspaceLayout::Shared {
        dir: std::path::PathBuf::from(&ws.cwd),
    };
    let spawn_ms = now_ms();
    let out = spawn_with_bookkeeping(
        state,
        &cli,
        Some("merge-resolver".to_string()),
        None,
        None,
        layout,
        ws.id.clone(),
        None,
        main_thread_id,
    )
    .await?;
    // Inject immediately (no dependency gate — there's nothing to wait on).
    spawn_bootstrap_inject(
        state.registry.clone(),
        out.lifecycle_rx.resubscribe(),
        out.agent_id.clone(),
        system_prompt,
        BootstrapCtx {
            source: "worker",
            spell: String::new(),
            role_keys: Vec::new(),
        },
        Vec::new(),
        state.swarm.clone(),
        spawn_ms,
    );
    Ok(out.agent_id)
}

/// `PATCH /api/workspaces/:id/threads/:tid` — (re)name a direction. Naming a
/// previously-unnamed, non-`main` direction is ALSO the trigger for automatic
/// git isolation: we persist the name + branch, flip state to `preparing`,
/// return immediately, and a background task does git takeover + `worktree add`
/// + repoints the thread cwd, finally → `ready`. If git isolation fails (or the
/// project can't be a repo) the direction degrades gracefully to `shared` and
/// stays usable. The `main` direction and already-isolated threads are a pure
/// rename. The slug NEVER changes (keeps blackboard keys stable).
///
/// NOTE (P3 boundary): repointing the cwd does not migrate an ALREADY-running
/// agent's process cwd. The intended flow names the direction from the first
/// message (before file work) and the frontend gates orchestrator/worker spawns
/// on `state == "ready"`, restarting into the new cwd. Sequencing lives in P4/P5.
pub async fn update_thread_handler(
    State(state): State<AppState>,
    Path((workspace_id, thread_id)): Path<(String, String)>,
    Json(req): Json<UpdateThreadRequest>,
) -> Result<Json<ThreadInfo>, (StatusCode, Json<serde_json::Value>)> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must be non-empty"})),
        ));
    }
    let thread = state
        .store
        .get_thread(thread_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .filter(|t| t.workspace_id == workspace_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown thread: {thread_id}")})),
            )
        })?;

    // Kick off git isolation only for a not-yet-isolated, non-main direction
    // that isn't already mid-isolation. The `state != "preparing"` guard makes
    // a second concurrent PATCH (double-click / retried swarm_name_thread) a
    // pure rename instead of racing a second `git worktree add` on the same
    // dest. A degraded thread is back at `ready`, so it can still retry.
    let should_isolate =
        thread.slug != "main" && thread.isolation != "worktree" && thread.state != "preparing";

    if should_isolate {
        // Branch (and thus the worktree dir `<project>-<branch>`) is derived
        // from the STABLE per-workspace-unique slug, NOT the raw name: two
        // directions sharing a display name still get distinct slugs, so they
        // can't collide on the same branch/worktree dir. `name` only updates
        // the display label.
        let branch = thread.slug.clone();
        // Phase 1 (sync, fast): name + `preparing`. branch/isolation/cwd are
        // persisted in phase 2 only once the worktree actually exists, so a
        // failed isolation never leaves a stale branch on a shared thread.
        state
            .store
            .update_thread(
                thread_id.clone(),
                Some(name.to_string()),
                None, // slug stays stable
                None, // isolation flips in phase 2 on success
                None, // branch persisted in phase 2 on success
                None, // cwd repoints in phase 2 on success
                Some("preparing".to_string()),
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;
        // Phase 2 (background): git takeover + worktree add → ready.
        spawn_thread_worktree(
            state.clone(),
            thread_id.clone(),
            workspace_id.clone(),
            thread.cwd.clone(),
            branch,
        );
    } else {
        state
            .store
            .update_thread(
                thread_id.clone(),
                Some(name.to_string()),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
            })?;
    }

    // Phase-1 rename / `preparing` flip is persisted now — tell the sidebar.
    // (Isolation success/degrade fires a second `ThreadChanged` from phase 2.)
    publish_thread_changed(&state, &workspace_id, &thread_id, "updated");

    let updated = state
        .store
        .get_thread(thread_id.clone())
        .await
        .ok()
        .flatten()
        .map(thread_record_to_info)
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "thread vanished after update"})),
            )
        })?;
    Ok(Json(updated))
}

#[derive(serde::Deserialize)]
pub struct SetThreadModelRequest {
    /// Abstract tier (opus|sonnet|haiku) or a concrete model id. Empty / null =
    /// clear (use the global default). The body carries the COMPLETE desired
    /// state — both fields are always written — so there's no absent-vs-clear
    /// ambiguity.
    #[serde(default)]
    pub tier: Option<String>,
    /// Abstract reasoning effort (low|medium|high|max). Empty / null = clear
    /// (the model's own default).
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// PUT /api/workspaces/:id/threads/:tid/model — set (or clear) this direction's
/// model AND reasoning effort (the body is the full desired state). Persists
/// only; takes effect on the NEXT spawn in the direction (the client restarts
/// the orchestrator to apply it now). Returns the updated thread.
pub async fn set_thread_model_handler(
    State(state): State<AppState>,
    Path((workspace_id, thread_id)): Path<(String, String)>,
    Json(req): Json<SetThreadModelRequest>,
) -> Result<Json<ThreadInfo>, (StatusCode, Json<serde_json::Value>)> {
    // Confirm the thread exists and belongs to this workspace.
    let thread = state
        .store
        .get_thread(thread_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .filter(|t| t.workspace_id == workspace_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown thread: {thread_id}")})),
            )
        })?;
    let _ = thread;
    // Normalize empty → None (clear). A concrete tier/model/effort is stored
    // verbatim and resolved per-CLI at spawn time.
    let tier = req
        .tier
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let reasoning = req
        .reasoning
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // Validate effort here too (parity with PUT /api/models) — a bogus level
    // would otherwise reach `--effort` at spawn. (tier is a free model id /
    // alias, validated per-CLI at resolve time, so it's not constrained here.)
    if let Some(r) = reasoning.as_deref() {
        if !["low", "medium", "high", "xhigh", "max"].contains(&r) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": format!("invalid reasoning effort '{r}' — valid: low|medium|high|xhigh|max")}),
                ),
            ));
        }
    }
    state
        .store
        .set_thread_model_tier(thread_id.clone(), tier)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    state
        .store
        .set_thread_reasoning_effort(thread_id.clone(), reasoning)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    publish_thread_changed(&state, &workspace_id, &thread_id, "updated");
    let updated = state
        .store
        .get_thread(thread_id.clone())
        .await
        .ok()
        .flatten()
        .map(thread_record_to_info)
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "thread vanished after update"})),
            )
        })?;
    Ok(Json(updated))
}

/// Background git isolation for a freshly-named direction. The git calls block
/// (each internally time-boxed at 30s) so they run on a blocking thread off the
/// async runtime. On success the thread is repointed to the worktree dir and
/// marked `worktree`/`ready`; on ANY failure it degrades to `shared`/`ready`
/// (cwd unchanged) so the direction stays usable, just not isolated.
fn spawn_thread_worktree(
    state: AppState,
    thread_id: String,
    workspace_id: String,
    project_cwd: String,
    branch: String,
) {
    tokio::spawn(async move {
        let cwd_for_git = project_cwd.clone();
        let branch_for_git = branch.clone();
        let git_result = tokio::task::spawn_blocking(move || {
            let p = std::path::Path::new(&cwd_for_git);
            crate::worktree::git_init_with_commit(p)?;
            crate::worktree::worktree_add(p, &branch_for_git)
        })
        .await;
        match git_result {
            Ok(Ok(dest)) => {
                let dest_str = dest.to_string_lossy().into_owned();
                let update = state
                    .store
                    .update_thread(
                        thread_id.clone(),
                        None,
                        None,
                        Some("worktree".to_string()),
                        Some(branch.clone()),
                        Some(dest_str.clone()),
                        Some("ready".to_string()),
                    )
                    .await;
                // Was the direction soft-deleted while we were isolating?
                // `update_thread` guards on `deleted_at IS NULL`, so the write
                // above no-ops on a mid-flight delete — detect it by re-reading
                // and DON'T re-root into / leak a dead direction.
                let deleted = !matches!(
                    state.store.get_thread(thread_id.clone()).await,
                    Ok(Some(t)) if t.deleted_at.is_none()
                );
                if deleted {
                    tracing::info!(thread = %thread_id, "direction deleted during isolation; removing orphaned worktree");
                    let repo = project_cwd.clone();
                    let d = dest_str.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::worktree::worktree_remove(
                            std::path::Path::new(&repo),
                            std::path::Path::new(&d),
                        )
                    })
                    .await;
                } else if let Err(e) = update {
                    // The worktree exists on disk but we couldn't record it.
                    // Degrade to shared/ready so the direction is never left
                    // stuck in `preparing` (the worktree dir is orphaned —
                    // acceptable; it's reusable on a later same-slug retry).
                    tracing::warn!(?e, thread = %thread_id, "worktree built but thread update failed; degrading to shared");
                    degrade_thread_to_shared(&state, &thread_id).await;
                    publish_thread_changed(&state, &workspace_id, &thread_id, "updated");
                } else {
                    tracing::info!(thread = %thread_id, dest = %dest_str, "direction isolated in git worktree");
                    // Re-emit the workspace deps-context into the worktree so the
                    // re-rooted orchestrator (whose new cwd is the worktree, a
                    // copy of the cwd repo only) can still see the peer/dependency
                    // projects at their real paths — otherwise it loses sight of
                    // repos the user may be asking it to work on.
                    write_deps_context_into_dir(&state, &workspace_id, &dest_str).await;
                    // P5-D: re-root the orchestrator into the fresh worktree.
                    // The orchestrator that named the direction is still
                    // running in the OLD (shared) cwd, so its own edits + any
                    // workers it dispatches would split-brain across two dirs.
                    reroot_thread_orchestrator(&state, &workspace_id, &thread_id, &dest_str).await;
                    // worktree/ready + cwd repointed → sidebar flips to the
                    // branch icon + worktree path live (no reload).
                    publish_thread_changed(&state, &workspace_id, &thread_id, "isolated");
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(?e, thread = %thread_id, "git isolation failed; direction stays shared");
                degrade_thread_to_shared(&state, &thread_id).await;
                publish_thread_changed(&state, &workspace_id, &thread_id, "updated");
            }
            Err(e) => {
                tracing::warn!(?e, thread = %thread_id, "worktree task panicked; direction stays shared");
                degrade_thread_to_shared(&state, &thread_id).await;
                publish_thread_changed(&state, &workspace_id, &thread_id, "updated");
            }
        }
    });
}

/// Mark a direction `degraded`/`ready` after a failed isolation attempt. We use
/// a distinct `isolation = "degraded"` (not plain `shared`) so the sidebar can
/// SIGNAL that isolation was attempted and failed — otherwise it looks identical
/// to a not-yet-isolated direction and the user wrongly believes their work is
/// isolated when it's actually sharing the main cwd (two directions' agents then
/// clobber each other's files). `degraded != "worktree"`, so a later rename
/// still retries isolation, and the delete path still skips worktree removal.
async fn degrade_thread_to_shared(state: &AppState, thread_id: &str) {
    let _ = state
        .store
        .update_thread(
            thread_id.to_string(),
            None,
            None,
            Some("degraded".to_string()),
            None,
            None,
            Some("ready".to_string()),
        )
        .await;
}

/// P5-D: after a direction is isolated into a worktree, re-root its orchestrator
/// there. The orchestrator that named the direction is still running in the OLD
/// (shared) cwd; restarting it in the worktree keeps its self-edits + any
/// workers it dispatches from splitting across two directories. We kill the
/// direction's live agents (naming happens BEFORE any worker is dispatched, per
/// orchestrator.md, so this is usually just the orchestrator) and re-run `init`
/// in the new cwd — the fresh orchestrator reads the existing ledger (Phase A
/// short-circuit) and continues. Only ever fires for a git-isolated direction.
async fn reroot_thread_orchestrator(
    state: &AppState,
    workspace_id: &str,
    thread_id: &str,
    new_cwd: &str,
) {
    // Agents we tear down here — their as-yet-unread user messages get
    // re-addressed to the fresh orchestrator below so nothing is dropped.
    let mut killed_ids: Vec<String> = Vec::new();
    match state.store.list_agents().await {
        Ok(rows) => {
            for a in rows {
                if a.thread_id.as_deref() == Some(thread_id)
                    && a.killed_at.is_none()
                    && a.shim_exit_at.is_none()
                {
                    teardown_agent(state, &a.id).await;
                    killed_ids.push(a.id);
                }
            }
            // Naming is supposed to happen BEFORE any worker is dispatched
            // (orchestrator.md), so this should usually kill just the
            // orchestrator. If it killed more, a worker was torn down mid-task
            // (its old-cwd work is abandoned) — surface the invariant breach.
            if killed_ids.len() > 1 {
                tracing::warn!(
                    thread = %thread_id, killed = killed_ids.len(),
                    "re-root tore down >1 agent — a worker was dispatched before naming"
                );
            }
        }
        Err(e) => {
            tracing::warn!(?e, thread = %thread_id, "re-root: list_agents failed; old agents may linger in the shared cwd");
        }
    }
    // The first orchestrator read the user's opening request to NAME this
    // direction, then was torn down (above) BEFORE it wrote a ledger — so the
    // fresh orchestrator would otherwise have neither the ledger nor the
    // (already-read) message and would re-onboard from scratch. Seed its
    // `{task}` with that request so its first turn acts on it instead of asking
    // the user what they want all over again. Best-effort: empty seed on any
    // error just reverts to the old first-wake greeting.
    let seed_task = state
        .store
        .latest_user_message_for_agents(killed_ids.clone())
        .await
        .unwrap_or(None)
        .unwrap_or_default();
    let req = RunSpellRequest {
        name: "init".into(),
        task: seed_task,
        workspace_dir: Some(new_cwd.to_string()),
        workspace_id: Some(workspace_id.to_string()),
        caller_agent_id: None,
        thread_id: Some(thread_id.to_string()),
    };
    match run_spell(State(state.clone()), Json(req)).await {
        Ok(Json(resp)) => {
            // Hand the killed orchestrator's unanswered user messages to the
            // fresh one (new agent id) so the first message that triggered the
            // rename isn't stranded on a dead inbox.
            if let Some(orch) = resp.agents.iter().find(|a| a.role == "orchestrator") {
                match state
                    .store
                    .reassign_unread_user_messages(killed_ids.clone(), orch.agent_id.clone())
                    .await
                {
                    Ok(n) if n > 0 => tracing::info!(
                        thread = %thread_id, moved = n, new_orch = %orch.agent_id,
                        "re-root: moved unread user messages to the new orchestrator"
                    ),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(
                        ?e, thread = %thread_id,
                        "re-root: reassigning unread user messages failed"
                    ),
                }
            }
        }
        Err((status, _)) => {
            tracing::warn!(
                %status, thread = %thread_id,
                "re-root orchestrator after isolation failed (revive on demand)"
            );
        }
    }
}

/// `DELETE /api/workspaces/:id/threads/:tid` — soft-delete a direction (its
/// slug becomes reusable). A git worktree, if any, is removed best-effort in
/// the background. The `main` direction cannot be deleted.
pub async fn delete_thread_handler(
    State(state): State<AppState>,
    Path((workspace_id, thread_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let thread = state
        .store
        .get_thread(thread_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .filter(|t| t.workspace_id == workspace_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("unknown thread: {thread_id}")})),
            )
        })?;
    if thread.slug == "main" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the main direction cannot be deleted"})),
        ));
    }
    // Kill the direction's live agents FIRST. Otherwise, once the thread row is
    // gone, those agents are orphaned: P4's strict thread-scoping hides them
    // from chat/DAG/members so the user can't kill them, and `git worktree
    // remove --force` below would yank a still-running agent's cwd out from
    // under it. (Workspace delete deliberately does NOT kill its agents — a
    // direction is the opposite: its agents have nowhere left to live.)
    match state.store.list_agents().await {
        Ok(rows) => {
            for a in rows {
                if a.thread_id.as_deref() == Some(thread_id.as_str())
                    && a.killed_at.is_none()
                    && a.shim_exit_at.is_none()
                {
                    teardown_agent(&state, &a.id).await;
                }
            }
        }
        Err(e) => {
            // Don't soft-delete + force-remove the worktree while we may have
            // failed to kill live agents in it — fail loud instead of orphaning.
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({"error": format!("could not enumerate agents to stop before delete: {e}")}),
                ),
            ));
        }
    }
    state
        .store
        .soft_delete_thread(thread_id.clone(), now_ms())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    publish_thread_changed(&state, &workspace_id, &thread_id, "deleted");

    // Drop the direction's blackboard ledgers (`<ws>/<slug>/…`) so they don't
    // orphan in the panel once the slug is gone. Applies to shared directions
    // too (they still wrote ledgers under the prefix). Best-effort: DB rows
    // first, then the on-disk dir — the notify watcher just sees a removal.
    let bb_prefix = format!("{}/{}", workspace_id, thread.slug);
    if let Err(e) = state
        .store
        .delete_blackboard_prefix(bb_prefix.clone())
        .await
    {
        tracing::warn!(?e, prefix = %bb_prefix, "failed to delete direction blackboard ops");
    }
    let bb_dir = state.blackboard_root.join(&workspace_id).join(&thread.slug);
    let _ = tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(&bb_dir);
    })
    .await;

    // Best-effort worktree cleanup. repo = the workspace's primary cwd; dest =
    // the thread's worktree dir.
    if thread.isolation == "worktree" {
        if let Ok(Some(ws)) = state.store.get_workspace_by_id(workspace_id).await {
            let repo = ws.cwd.clone();
            let dest = thread.cwd.clone();
            let branch = thread.branch.clone();
            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    let repo_path = std::path::Path::new(&repo);
                    // Remove the worktree FIRST — git refuses to delete a branch
                    // still checked out in one. Then drop the now-orphaned branch
                    // so a same-named direction recreated later starts fresh
                    // instead of re-attaching this branch's history.
                    let _ =
                        crate::worktree::worktree_remove(repo_path, std::path::Path::new(&dest));
                    if let Some(b) = branch.as_deref().filter(|b| !b.is_empty()) {
                        let _ = crate::worktree::delete_branch(repo_path, b);
                    }
                })
                .await;
            });
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/workspaces/:id` — soft-delete a workspace. Live agents
/// in the workspace are intentionally NOT killed; the row just stops
/// showing up in `GET /api/workspaces` so the left nav loses it. Anyone
/// still attached via the WS keeps their PTY alive, by design.
pub async fn delete_workspace_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state
        .store
        .soft_delete_workspace(id.clone(), now_ms())
        .await
    {
        Ok(0) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("workspace {id} not found or already deleted")})),
        ),
        Ok(_) => (StatusCode::NO_CONTENT, Json(json!({"ok": true}))),
        Err(e) => {
            tracing::warn!(?e, ws_id = %id, "soft_delete_workspace failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
    }
}

/// Re-derive and rewrite the workspace's flockmux-managed deps context
/// block from the current set of attached roots. Call this after any
/// add/delete so CLAUDE.md / AGENTS.md stay in sync (and the block is
/// stripped once the last root is removed). Best-effort: store errors are
/// logged and swallowed — the membership change already committed and the
/// context file is advisory, never load-bearing.
async fn refresh_workspace_deps_context(state: &AppState, workspace_id: &str) {
    let ws = match state
        .store
        .get_workspace_by_id(workspace_id.to_string())
        .await
    {
        Ok(Some(ws)) => ws,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(?e, ws_id = %workspace_id, "refresh deps context: get_workspace_by_id failed");
            return;
        }
    };
    // Don't touch the context file of a soft-deleted workspace.
    if ws.deleted_at.is_some() {
        return;
    }
    let roots: Vec<WorkspaceRoot> = match state
        .store
        .list_workspace_roots(workspace_id.to_string())
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| WorkspaceRoot {
                id: r.id,
                path: r.path,
                role: r.role,
                label: r.label,
                parent_id: r.parent_id,
                branch: None, // deps-context writer ignores branch
            })
            .collect(),
        Err(e) => {
            tracing::warn!(?e, ws_id = %workspace_id, "refresh deps context: list_workspace_roots failed");
            return;
        }
    };
    write_workspace_deps_context(ws.cwd.trim(), &ws.name, &roots);
}

/// Write the managed deps-context block (CLAUDE.md / AGENTS.md) into an
/// ARBITRARY directory — used when a direction is isolated into a git worktree.
/// The worktree is a copy of the cwd repo ONLY; the peer/dependency roots live
/// at their original absolute paths and are NOT carried in. Without re-emitting
/// the context here, the re-rooted orchestrator's new cwd (the worktree) has no
/// record that the peer projects exist, so it loses sight of repos the user may
/// actually be asking it to work on. We list the worktree as the primary and the
/// peers at their real paths (still readable), restoring multi-root visibility.
async fn write_deps_context_into_dir(state: &AppState, workspace_id: &str, target_dir: &str) {
    let ws = match state
        .store
        .get_workspace_by_id(workspace_id.to_string())
        .await
    {
        Ok(Some(ws)) => ws,
        _ => return,
    };
    if ws.deleted_at.is_some() {
        return;
    }
    let roots: Vec<WorkspaceRoot> = match state
        .store
        .list_workspace_roots(workspace_id.to_string())
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| WorkspaceRoot {
                id: r.id,
                path: r.path,
                role: r.role,
                label: r.label,
                parent_id: r.parent_id,
                branch: None,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(?e, ws_id = %workspace_id, "worktree deps context: list_workspace_roots failed");
            return;
        }
    };
    // No peer/dependency roots → nothing worth re-emitting in the worktree.
    if roots.is_empty() {
        return;
    }
    write_workspace_deps_context(target_dir, &ws.name, &roots);
}

/// `POST /api/workspaces/:id/roots` — attach a dependency-source root to an
/// existing workspace. Mirrors the per-root validation in
/// `create_workspace_handler` (exists + is a dir → 4xx) and rejects
/// duplicates already attached to this workspace. On success the managed
/// context block in CLAUDE.md / AGENTS.md is refreshed.
pub async fn add_workspace_root_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<WorkspaceRoot>,
) -> Result<Json<WorkspaceRoot>, (StatusCode, Json<serde_json::Value>)> {
    // 404 if the workspace is missing or soft-deleted.
    let ws = state
        .store
        .get_workspace_by_id(id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .filter(|ws| ws.deleted_at.is_none())
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("workspace {id} not found")})),
            )
        })?;

    let path = req.path.trim();
    if path.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "dependency path must be non-empty"})),
        ));
    }
    {
        let p = std::path::Path::new(path);
        if !p.exists() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("dependency directory does not exist: {}", path)})),
            ));
        }
        if !p.is_dir() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("dependency path is not a directory: {}", path)})),
            ));
        }
    }

    // Reject a duplicate already attached to this workspace.
    let existing = state
        .store
        .list_workspace_roots(ws.id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;
    if existing.iter().any(|r| r.path == path) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("dependency already attached: {}", path)})),
        ));
    }

    // If a parent was supplied, it must be an existing node in THIS
    // workspace's tree. A parent in another workspace (or a stale id) is a
    // client bug — 400. A genuinely-missing id is a 404.
    let parent_id = match req
        .parent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(pid) => {
            let parent = state
                .store
                .get_workspace_root(pid.to_string())
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": e.to_string()})),
                    )
                })?
                .ok_or_else(|| {
                    (
                        StatusCode::NOT_FOUND,
                        Json(json!({"error": format!("parent root {pid} not found")})),
                    )
                })?;
            if parent.workspace_id != ws.id {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!(
                        "parent root {pid} belongs to a different workspace"
                    )})),
                ));
            }
            Some(pid.to_string())
        }
        None => None,
    };

    let saved = state
        .store
        .add_workspace_root(
            NewWorkspaceRoot {
                workspace_id: ws.id.clone(),
                path: path.to_string(),
                role: req.role,
                label: req.label,
                parent_id,
            },
            now_ms(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    refresh_workspace_deps_context(&state, &id).await;

    Ok(Json(WorkspaceRoot {
        id: saved.id,
        path: saved.path,
        role: saved.role,
        label: saved.label,
        parent_id: saved.parent_id,
        branch: None, // filled on the next workspaces list refetch
    }))
}

/// `DELETE /api/workspaces/:id/roots?id=<root_id>` — detach a node from the
/// workspace's logical tree, CASCADING to all of its descendants. The node id
/// rides in the query string (DELETE has no body in the frontend's fetch).
/// Refreshes the managed context block afterwards (stripping it if this
/// removed the last node).
pub async fn delete_workspace_root_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let root_id = match params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing required query param 'id'"})),
            ))
        }
    };

    let n = state
        .store
        .delete_workspace_root(id.clone(), root_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    refresh_workspace_deps_context(&state, &id).await;

    Ok(Json(json!({"deleted": n})))
}

/// `GET /api/workspaces/:id/root-suggestions[?path=<dir>]` — scan a project
/// dir for manifest-declared LOCAL PATH dependencies (package.json file:/link:,
/// Cargo.toml path deps, go.mod replace directives, pyproject.toml uv sources)
/// and return them as attachable root suggestions. `?path=` selects which dir
/// to scan (e.g. a peer project's dir when adding a child under it); it
/// defaults to the workspace's primary `cwd`. Best-effort: parse errors and
/// missing files are swallowed — this only ever feeds an optional picker.
/// Excludes the scanned dir itself and any path already attached anywhere in
/// the workspace.
pub async fn suggest_workspace_roots_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Vec<WorkspaceRoot>> {
    let ws = match state.store.get_workspace_by_id(id.clone()).await {
        Ok(Some(ws)) => ws,
        _ => return Json(Vec::new()),
    };
    // Scan the dir named by ?path= (a specific node's project dir) or fall
    // back to the workspace's primary cwd.
    let scan_dir = params
        .get("path")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ws.cwd.trim())
        .to_string();
    let cwd = std::path::Path::new(&scan_dir);

    // Canonical cwd (used to exclude the project itself from suggestions).
    let cwd_canon = std::fs::canonicalize(cwd).ok();

    // Canonical set of already-attached roots — suggestions never repeat
    // what's mounted. We canonicalize each so a `./foo` vs `/abs/foo`
    // mismatch still dedups.
    let mut already: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    if let Ok(rows) = state.store.list_workspace_roots(id.clone()).await {
        for r in rows {
            if let Ok(c) = std::fs::canonicalize(&r.path) {
                already.insert(c);
            }
        }
    }

    // (relative-or-abs path string from the manifest, label) pairs.
    let mut candidates: Vec<(String, String)> = Vec::new();

    // package.json — dependencies / devDependencies values starting with
    // `file:` or `link:` point at a local path.
    if let Ok(txt) = std::fs::read_to_string(cwd.join("package.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
            for section in ["dependencies", "devDependencies"] {
                if let Some(map) = v.get(section).and_then(|s| s.as_object()) {
                    for (name, val) in map {
                        if let Some(spec) = val.as_str() {
                            for prefix in ["file:", "link:"] {
                                if let Some(rest) = spec.strip_prefix(prefix) {
                                    candidates.push((rest.to_string(), name.clone()));
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Cargo.toml — line scan for inline `path = "..."` (covers both
    // `name = { path = "..." }` and a `path = "..."` line inside a
    // `[dependencies.name]` table). Label is best-effort: the crate name
    // to the left of `=` if present, else the path basename.
    if let Ok(txt) = std::fs::read_to_string(cwd.join("Cargo.toml")) {
        for line in txt.lines() {
            let trimmed = line.trim();
            if let Some(rel) = extract_quoted_after(trimmed, "path") {
                let name = trimmed
                    .split('=')
                    .next()
                    .map(|s| s.trim().trim_matches(['{', ' ']))
                    .filter(|s| !s.is_empty() && !s.starts_with('['))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| basename_of(&rel));
                candidates.push((rel, name));
            }
        }
    }

    // go.mod — `replace <module> => <target> [version]` where the target is
    // a local path (./ ../ or absolute). Label = path basename.
    if let Ok(txt) = std::fs::read_to_string(cwd.join("go.mod")) {
        for line in txt.lines() {
            let trimmed = line.trim();
            let body = trimmed.strip_prefix("replace ").unwrap_or(trimmed);
            if let Some((_, rhs)) = body.split_once("=>") {
                if let Some(target) = rhs.split_whitespace().next() {
                    if target.starts_with("./")
                        || target.starts_with("../")
                        || target.starts_with('/')
                    {
                        candidates.push((target.to_string(), basename_of(target)));
                    }
                }
            }
        }
    }

    // pyproject.toml — under `[tool.uv.sources]`, lines of the form
    // `name = { path = "..." }`. Label = name (else basename).
    if let Ok(txt) = std::fs::read_to_string(cwd.join("pyproject.toml")) {
        let mut in_uv_sources = false;
        for line in txt.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_uv_sources = trimmed == "[tool.uv.sources]";
                continue;
            }
            if in_uv_sources {
                if let Some(rel) = extract_quoted_after(trimmed, "path") {
                    let name = trimmed
                        .split('=')
                        .next()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| basename_of(&rel));
                    candidates.push((rel, name));
                }
            }
        }
    }

    // pom.xml — Maven deps are jar coordinates (groupId/artifactId/version),
    // not local paths, so we can't read a path out of the manifest. Instead we
    // LOCATE local Maven projects on disk whose own `artifactId` matches a
    // declared dependency, covering the two common local layouts. Only runs
    // when the scanned dir actually has a pom.xml. Candidates are pushed as
    // absolute paths so they flow through the same canonicalize/exclude/dedup
    // pipeline below as every other ecosystem.
    if let Ok(pom) = std::fs::read_to_string(cwd.join("pom.xml")) {
        // (1) Multi-module reactor: each <module>REL</module> is a local
        // subdir scanDir/REL. Suggest it if scanDir/REL/pom.xml exists. Label
        // = the module's own artifactId if cheaply available, else REL.
        for rel in xml_tag_values(&pom, "module") {
            let module_dir = cwd.join(&rel);
            let module_pom = module_dir.join("pom.xml");
            if module_pom.is_file() {
                let label = std::fs::read_to_string(&module_pom)
                    .ok()
                    .and_then(|m| own_artifact_id(&m))
                    .unwrap_or(rel);
                candidates.push((module_dir.to_string_lossy().into_owned(), label));
            }
        }

        // (2) Sibling projects checked out next to this one. Collect every
        // <artifactId> referenced anywhere in the scanned pom (over-collecting
        // our own/parent/plugin ids is fine — they just won't match a real
        // sibling project, or if they do the user simply won't click). Then
        // scan the parent dir's immediate children for Maven projects whose
        // OWN artifactId is in that referenced set.
        let referenced: std::collections::HashSet<String> =
            xml_tag_values(&pom, "artifactId").into_iter().collect();
        if !referenced.is_empty() {
            if let Some(parent) = cwd.parent() {
                if let Ok(entries) = std::fs::read_dir(parent) {
                    // Bound the scan so a huge parent dir can't blow up the
                    // request — only the first 200 child dirs are considered.
                    for entry in entries.flatten().take(200) {
                        let child = entry.path();
                        if !child.is_dir() {
                            continue;
                        }
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        // Skip the scanned dir itself, hidden dirs, and the
                        // usual build/vendor noise.
                        if name.starts_with('.') || name == "target" || name == "node_modules" {
                            continue;
                        }
                        // The scanned dir itself is among these children but is
                        // excluded downstream by the cwd_canon check, so we
                        // needn't special-case it here.
                        let child_pom = child.join("pom.xml");
                        if !child_pom.is_file() {
                            continue;
                        }
                        if let Ok(child_xml) = std::fs::read_to_string(&child_pom) {
                            if let Some(aid) = own_artifact_id(&child_xml) {
                                if referenced.contains(&aid) {
                                    candidates.push((child.to_string_lossy().into_owned(), aid));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Resolve each candidate relative to cwd, canonicalize, keep only
    // existing dirs, drop the cwd itself + already-attached + dupes.
    let mut seen: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    let mut out: Vec<WorkspaceRoot> = Vec::new();
    for (rel, label) in candidates {
        let raw = std::path::Path::new(&rel);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            cwd.join(raw)
        };
        let canon = match std::fs::canonicalize(&joined) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !canon.is_dir() {
            continue;
        }
        if Some(&canon) == cwd_canon.as_ref() {
            continue;
        }
        if already.contains(&canon) {
            continue;
        }
        if !seen.insert(canon.clone()) {
            continue;
        }
        out.push(WorkspaceRoot {
            id: String::new(),
            path: canon.to_string_lossy().into_owned(),
            role: "dependency".to_string(),
            label: Some(label),
            parent_id: None,
            branch: None, // suggestion only — not attached yet
        });
    }

    Json(out)
}

/// Pull the first `"..."`-quoted value that follows `<key>` (optionally with
/// `=`) on a single manifest line, e.g. `extract_quoted_after("foo = { path
/// = \"../bar\" }", "path")` → `Some("../bar")`. Returns `None` if the key or
/// a quoted value isn't present. Deliberately simple — these are best-effort
/// suggestion parsers, not a TOML implementation.
fn extract_quoted_after(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let after_key = &line[idx + key.len()..];
    // Require an `=` between the key and the opening quote so we don't match
    // e.g. a `paths = [...]` array as a single path.
    let eq = after_key.find('=')?;
    let after_eq = &after_key[eq + 1..];
    let start = after_eq.find('"')? + 1;
    let rest = &after_eq[start..];
    let end = rest.find('"')?;
    let val = &rest[..end];
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

/// Last path component of a path string, used as a fallback dependency label.
fn basename_of(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// Crude XML scan: return the trimmed inner text of every `<tag>...</tag>`
/// occurrence in `xml`. Used for Maven pom.xml `<artifactId>` and `<module>`
/// extraction. Deliberately not a real XML parser — these are best-effort
/// suggestion inputs, so namespaces, comments, and attributes are ignored.
fn xml_tag_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        let val = after[..end].trim();
        if !val.is_empty() {
            out.push(val.to_string());
        }
        rest = &after[end + close.len()..];
    }
    out
}

/// Extract a Maven pom's OWN `artifactId` (not its parent's). A `<parent>`
/// block carries its own `<artifactId>`; to skip it we start searching after
/// the first `</parent>` (if any), else from the start, then take the first
/// `<artifactId>...</artifactId>`.
fn own_artifact_id(xml: &str) -> Option<String> {
    let search_from = xml
        .find("</parent>")
        .map(|i| i + "</parent>".len())
        .unwrap_or(0);
    xml_tag_values(&xml[search_from..], "artifactId")
        .into_iter()
        .next()
}
