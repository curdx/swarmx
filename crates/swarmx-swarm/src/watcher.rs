//! Blackboard filesystem watcher. Detects edits made by external tools
//! (the user editing markdown in their normal editor) and persists those
//! changes to SQLite + broadcasts them on `SwarmEvent::BlackboardChanged`.
//!
//! Self-loops (`write_blackboard` → fs::write → watcher fires) are
//! suppressed by the caller-side SHA-256 cache in [`crate::swarm::Swarm`].
//! By the time the debouncer's 150ms window expires, the cache already
//! has the post-write hash so the watcher just sees its own bytes and
//! skips the insert.

use crate::swarm::Swarm;
use anyhow::{Context, Result};
use notify_debouncer_full::{
    new_debouncer,
    notify::{ErrorKind, EventKind, RecursiveMode},
    DebounceEventResult, Debouncer, FileIdMap, RecommendedCache,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;

/// Held alive for the lifetime of the watcher subscription. Dropping it
/// terminates the debouncer's background thread cleanly.
pub struct WatcherHandle {
    // The exact debouncer type depends on the platform's `notify` backend.
    // We keep it boxed-but-typed so callers don't need to know.
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
}

impl WatcherHandle {
    /// Start watching `root` recursively. The debouncer fires its callback
    /// every ~150ms; each call may contain a batch of events the
    /// debouncer has collapsed.
    pub fn spawn(root: PathBuf, swarm: Arc<Swarm>) -> Result<Self> {
        // The notify-debouncer-full callback runs on its own dedicated
        // thread, which is *not* a tokio worker, so we capture a runtime
        // handle here and dispatch the async reconcile work through it.
        let rt = Handle::try_current()
            .context("WatcherHandle::spawn must be called from a Tokio runtime")?;

        // notify-debouncer-full 0.7 takes (timeout, tick_rate, callback).
        // 150ms timeout is short enough to feel live in the UI but long
        // enough that "save in vim" (which can be 2-3 fs events) collapses
        // into a single delivery.
        let debouncer = new_debouncer(
            Duration::from_millis(150),
            None, // default tick rate
            move |result: DebounceEventResult| {
                let swarm = swarm.clone();
                handle_batch(&rt, &swarm, result);
            },
        )
        .context("create notify-debouncer")?;

        let mut debouncer = debouncer;
        debouncer
            .watch(&root, RecursiveMode::Recursive)
            .with_context(|| format!("watch blackboard root {}", root.display()))?;
        tracing::info!(root = %root.display(), "blackboard watcher running");
        Ok(Self {
            _debouncer: debouncer,
        })
    }
}

fn handle_batch(rt: &Handle, swarm: &Arc<Swarm>, result: DebounceEventResult) {
    let events = match result {
        Ok(v) => v,
        Err(errors) => {
            for e in errors {
                let recoverable = matches!(e.kind, ErrorKind::PathNotFound | ErrorKind::Generic(_));
                if recoverable {
                    tracing::debug!(?e, "blackboard watcher: recoverable error");
                } else {
                    tracing::warn!(?e, "blackboard watcher: error");
                }
            }
            return;
        }
    };

    for ev in events {
        // We only care about content-affecting events. Create + Modify (data)
        // + Remove. Access events spam the channel on read-heavy filesystems.
        let kind = &ev.event.kind;
        let interested = matches!(
            kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        );
        if !interested {
            continue;
        }

        for path in &ev.event.paths {
            if is_ignored(path) {
                continue;
            }
            // We may receive the directory itself when create_dir_all
            // fires — only act on regular files.
            if path.is_dir() {
                continue;
            }
            let swarm = swarm.clone();
            let path = path.clone();
            rt.spawn(async move {
                if let Err(e) = swarm.reconcile_external(&path).await {
                    tracing::warn!(?e, path = %path.display(), "reconcile_external failed");
                }
            });
        }
    }
}

/// Filter out editor temp / VCS noise that would otherwise create phantom
/// blackboard ops every save.
fn is_ignored(p: &Path) -> bool {
    let s = match p.file_name().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return true,
    };
    if s.starts_with('.') {
        // .swp, .DS_Store, .git/*, etc.
        return true;
    }
    if s.ends_with('~') {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    if lower.ends_with(".swp") || lower.ends_with(".tmp") {
        return true;
    }
    // Any path segment of ".git" disqualifies (git index updates etc.).
    p.components().any(|c| c.as_os_str() == ".git")
}

/// Re-export for the doc-link in swarm.rs.
#[allow(dead_code)]
pub(crate) type _FileIdMapAlias = FileIdMap;
