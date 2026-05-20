//! Integration tests for `flockmux_storage::Store`. Each test owns its own
//! `TempDir` so they parallelise safely.

use flockmux_storage::{
    ListMessagesOpts, NewAgent, NewBlackboardOp, NewMessage, NewRecording, Store,
};
use tempfile::TempDir;

async fn fresh_store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("flockmux.db");
    let store = Store::open(&path).await.expect("open store");
    (dir, store)
}

fn ts(base: i64) -> i64 {
    1_700_000_000_000 + base
}

// ── agents ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn schema_bootstraps_on_open() {
    let (_dir, store) = fresh_store().await;
    let agents = store.list_agents().await.unwrap();
    assert!(agents.is_empty());
}

#[tokio::test]
async fn agent_spawn_then_list() {
    let (_dir, store) = fresh_store().await;
    store
        .record_agent_spawn(NewAgent {
            id: "a-1".into(),
            cli: "claude".into(),
            role: "explorer".into(),
            workspace: "/tmp/a".into(),
            spawned_at: ts(0),
        })
        .await
        .unwrap();
    let agents = store.list_agents().await.unwrap();
    assert_eq!(agents.len(), 1);
    let a = &agents[0];
    assert_eq!(a.id, "a-1");
    assert_eq!(a.cli, "claude");
    assert!(a.killed_at.is_none());
    assert!(a.shim_ready_at.is_none());
}

#[tokio::test]
async fn agent_lifecycle_updates_idempotent() {
    let (_dir, store) = fresh_store().await;
    store
        .record_agent_spawn(NewAgent {
            id: "a-2".into(),
            cli: "codex".into(),
            role: "codex".into(),
            workspace: "/tmp/b".into(),
            spawned_at: ts(0),
        })
        .await
        .unwrap();
    store
        .record_shim_ready("a-2".into(), ts(10))
        .await
        .unwrap();
    // Second call must be a no-op (idempotent — first non-NULL wins).
    store
        .record_shim_ready("a-2".into(), ts(99))
        .await
        .unwrap();
    store
        .record_shim_exit("a-2".into(), 0, ts(20))
        .await
        .unwrap();
    store.record_agent_kill("a-2".into(), ts(30)).await.unwrap();

    let a = store.list_agents().await.unwrap().pop().unwrap();
    assert_eq!(a.shim_ready_at, Some(ts(10)), "ready timestamp pinned");
    assert_eq!(a.shim_exit_code, Some(0));
    assert_eq!(a.killed_at, Some(ts(30)));
}

// ── messages ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn message_insert_list_filter() {
    let (_dir, store) = fresh_store().await;
    let m1 = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "hello b".into(),
            sent_at: ts(1),
        })
        .await
        .unwrap();
    let _m2 = store
        .insert_message(NewMessage {
            from_agent: "b".into(),
            to_agent: "a".into(),
            kind: "note".into(),
            body: "hi a".into(),
            sent_at: ts(2),
        })
        .await
        .unwrap();

    let to_b = store
        .list_messages(ListMessagesOpts {
            to_agent: Some("b".into()),
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(to_b.len(), 1);
    assert_eq!(to_b[0].id, m1.id);
    assert_eq!(to_b[0].body, "hello b");
    assert!(to_b[0].delivered_at.is_none());
}

#[tokio::test]
async fn message_search_fts5() {
    let (_dir, store) = fresh_store().await;
    store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "schedule a meeting tomorrow about the planner".into(),
            sent_at: ts(1),
        })
        .await
        .unwrap();
    store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "just a chatty hello, nothing planned".into(),
            sent_at: ts(2),
        })
        .await
        .unwrap();

    let hits = store.search_messages("planner".into()).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].body.contains("planner"));

    // porter stem: "planning" should fold to the same stem.
    let hits = store.search_messages("planned".into()).await.unwrap();
    assert_eq!(hits.len(), 1);
}

#[tokio::test]
async fn mark_delivered_only_undelivered() {
    let (_dir, store) = fresh_store().await;
    let m1 = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "one".into(),
            sent_at: ts(1),
        })
        .await
        .unwrap();
    let m2 = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "two".into(),
            sent_at: ts(2),
        })
        .await
        .unwrap();

    store
        .mark_delivered(vec![m1.id], ts(10))
        .await
        .unwrap();

    let pending = store
        .list_messages(ListMessagesOpts {
            to_agent: Some("b".into()),
            only_undelivered: true,
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, m2.id);
}

// ── blackboard ───────────────────────────────────────────────────────────

#[tokio::test]
async fn blackboard_insert_and_history() {
    let (_dir, store) = fresh_store().await;
    store
        .insert_blackboard_op(NewBlackboardOp {
            agent_id: Some("a".into()),
            op: "write".into(),
            path: "tasks.md".into(),
            content: "- [ ] first\n".into(),
            sha256: "abc".into(),
            at: ts(1),
        })
        .await
        .unwrap();
    store
        .insert_blackboard_op(NewBlackboardOp {
            agent_id: Some("a".into()),
            op: "write".into(),
            path: "tasks.md".into(),
            content: "- [x] first\n- [ ] second\n".into(),
            sha256: "def".into(),
            at: ts(2),
        })
        .await
        .unwrap();
    // Different file.
    store
        .insert_blackboard_op(NewBlackboardOp {
            agent_id: None,
            op: "external".into(),
            path: "notes/scratch.md".into(),
            content: "scratch".into(),
            sha256: "ghi".into(),
            at: ts(3),
        })
        .await
        .unwrap();

    let history = store
        .list_blackboard_ops(Some("tasks.md".into()))
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    // ORDER BY id DESC — newest first.
    assert_eq!(history[0].sha256, "def");

    let latest = store.list_blackboard_ops(None).await.unwrap();
    assert_eq!(latest.len(), 2, "one row per distinct path");
    let paths: Vec<&str> = latest.iter().map(|r| r.path.as_str()).collect();
    assert!(paths.contains(&"tasks.md"));
    assert!(paths.contains(&"notes/scratch.md"));
}

#[tokio::test]
async fn blackboard_search_fts5() {
    let (_dir, store) = fresh_store().await;
    store
        .insert_blackboard_op(NewBlackboardOp {
            agent_id: None,
            op: "write".into(),
            path: "spec.md".into(),
            content: "the swarm dispatch protocol talks about envelopes".into(),
            sha256: "x".into(),
            at: ts(1),
        })
        .await
        .unwrap();
    store
        .insert_blackboard_op(NewBlackboardOp {
            agent_id: None,
            op: "write".into(),
            path: "log.md".into(),
            content: "boring noise".into(),
            sha256: "y".into(),
            at: ts(2),
        })
        .await
        .unwrap();

    let hits = store
        .search_blackboard("envelopes".into())
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "spec.md");
}

// ── recordings ───────────────────────────────────────────────────────────

#[tokio::test]
async fn recording_start_then_finalize() {
    let (_dir, store) = fresh_store().await;
    store
        .record_recording_start(NewRecording {
            id: "rec-1".into(),
            agent_id: "a-1".into(),
            path: "/tmp/rec-1.cast".into(),
            started_at: ts(0),
            cols: 120,
            rows: 32,
        })
        .await
        .unwrap();

    let r = store.get_recording("rec-1".into()).await.unwrap().unwrap();
    assert_eq!(r.agent_id, "a-1");
    assert_eq!(r.cols, 120);
    assert!(r.finalized_at.is_none());
    assert!(r.duration_ms.is_none());

    store
        .record_recording_finalize("rec-1".into(), ts(100), 100, 17)
        .await
        .unwrap();
    let r = store.get_recording("rec-1".into()).await.unwrap().unwrap();
    assert_eq!(r.finalized_at, Some(ts(100)));
    assert_eq!(r.duration_ms, Some(100));
    assert_eq!(r.last_seq, Some(17));

    // Idempotent: second finalize is a no-op (first non-NULL wins).
    store
        .record_recording_finalize("rec-1".into(), ts(999), 999, 999)
        .await
        .unwrap();
    let r = store.get_recording("rec-1".into()).await.unwrap().unwrap();
    assert_eq!(r.finalized_at, Some(ts(100)));
}

#[tokio::test]
async fn recordings_listed_by_agent() {
    let (_dir, store) = fresh_store().await;
    for (i, agent) in [("rec-1", "a-1"), ("rec-2", "a-1"), ("rec-3", "a-2")] {
        store
            .record_recording_start(NewRecording {
                id: i.into(),
                agent_id: agent.into(),
                path: format!("/tmp/{}.cast", i),
                started_at: ts(0),
                cols: 80,
                rows: 24,
            })
            .await
            .unwrap();
    }
    let for_a1 = store.list_recordings(Some("a-1".into())).await.unwrap();
    assert_eq!(for_a1.len(), 2);
    assert!(for_a1.iter().all(|r| r.agent_id == "a-1"));

    let all = store.list_recordings(None).await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn store_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("flockmux.db");
    {
        let store = Store::open(&db_path).await.unwrap();
        store
            .record_agent_spawn(NewAgent {
                id: "persist-1".into(),
                cli: "claude".into(),
                role: "x".into(),
                workspace: "/tmp/x".into(),
                spawned_at: ts(0),
            })
            .await
            .unwrap();
    }
    let store = Store::open(&db_path).await.unwrap();
    let agents = store.list_agents().await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, "persist-1");
}
