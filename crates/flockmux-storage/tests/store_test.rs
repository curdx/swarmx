//! Integration tests for `flockmux_storage::Store`. Each test owns its own
//! `TempDir` so they parallelise safely.

use flockmux_storage::{
    ListMessagesOpts, MessageRecord, NewAgent, NewBlackboardOp, NewMessage, NewRecording, Store,
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
            workspace_id: None,
            spell_run_id: None,
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
            workspace_id: None,
            spell_run_id: None,
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
            in_reply_to: None,
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
            in_reply_to: None,
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
            in_reply_to: None,
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
            in_reply_to: None,
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
            in_reply_to: None,
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
            in_reply_to: None,
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

#[tokio::test]
async fn mark_read_sets_timestamp_and_returns_ids() {
    let (_dir, store) = fresh_store().await;
    let m1 = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "one".into(),
            sent_at: ts(1),
            in_reply_to: None,
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
            in_reply_to: None,
        })
        .await
        .unwrap();

    let marked = store
        .mark_read(vec![m1.id, m2.id], "b".into(), ts(10))
        .await
        .unwrap();
    assert_eq!(marked.len(), 2);
    assert!(marked.contains(&m1.id) && marked.contains(&m2.id));

    let rows = store
        .list_messages(ListMessagesOpts {
            to_agent: Some("b".into()),
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(rows.iter().all(|r| r.read_at == Some(ts(10))));

    // Idempotent: second call returns empty (read_at already set).
    let again = store
        .mark_read(vec![m1.id, m2.id], "b".into(), ts(99))
        .await
        .unwrap();
    assert!(again.is_empty());
}

#[tokio::test]
async fn mark_read_refuses_cross_agent() {
    let (_dir, store) = fresh_store().await;
    let m1 = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "for b only".into(),
            sent_at: ts(1),
            in_reply_to: None,
        })
        .await
        .unwrap();

    // Wrong recipient: c tries to mark b's mail read.
    let marked = store
        .mark_read(vec![m1.id], "c".into(), ts(10))
        .await
        .unwrap();
    assert!(marked.is_empty(), "cross-agent mark must be a no-op");

    let row = store
        .list_messages(ListMessagesOpts {
            to_agent: Some("b".into()),
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(row.read_at.is_none(), "row stayed unread");
}

#[tokio::test]
async fn count_unread_excludes_read() {
    let (_dir, store) = fresh_store().await;
    let m1 = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "one".into(),
            sent_at: ts(1),
            in_reply_to: None,
        })
        .await
        .unwrap();
    let _m2 = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "two".into(),
            sent_at: ts(2),
            in_reply_to: None,
        })
        .await
        .unwrap();
    // Unrelated recipient — must not count.
    let _other = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "c".into(),
            kind: "note".into(),
            body: "for c".into(),
            sent_at: ts(3),
            in_reply_to: None,
        })
        .await
        .unwrap();

    assert_eq!(store.count_unread("b".into()).await.unwrap(), 2);

    store
        .mark_read(vec![m1.id], "b".into(), ts(10))
        .await
        .unwrap();
    assert_eq!(store.count_unread("b".into()).await.unwrap(), 1);
    assert_eq!(store.count_unread("c".into()).await.unwrap(), 1);
}

#[tokio::test]
async fn consume_wakes_atomically_returns_wake_ids_and_marks_read() {
    // M6f: consume_wakes is the wake_check primary signal. It must
    // (a) only touch kind="wake", (b) only touch unread, (c) only
    // touch this agent, (d) mark read in the same transaction as the
    // SELECT, and (e) be idempotent (second call returns []).
    let (_dir, store) = fresh_store().await;
    let wake1 = store.insert_message(NewMessage {
        from_agent: "system".into(),
        to_agent: "critic".into(),
        kind: "wake".into(),
        body: "blackboard `frontend.done` updated".into(),
        sent_at: ts(1),
        in_reply_to: None,
    }).await.unwrap();
    let _note = store.insert_message(NewMessage {
        from_agent: "frontend".into(),
        to_agent: "critic".into(),
        kind: "note".into(),
        body: "fyi".into(),
        sent_at: ts(2),
        in_reply_to: None,
    }).await.unwrap();
    let wake2 = store.insert_message(NewMessage {
        from_agent: "system".into(),
        to_agent: "critic".into(),
        kind: "wake".into(),
        body: "blackboard `backend.done` updated".into(),
        sent_at: ts(3),
        in_reply_to: None,
    }).await.unwrap();
    // Different agent — must not be touched.
    let other_wake = store.insert_message(NewMessage {
        from_agent: "system".into(),
        to_agent: "test".into(),
        kind: "wake".into(),
        body: "for test".into(),
        sent_at: ts(4),
        in_reply_to: None,
    }).await.unwrap();

    let ids = store.consume_wakes("critic".into(), ts(100)).await.unwrap();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&wake1.id));
    assert!(ids.contains(&wake2.id));

    // Second call: nothing left to consume.
    let ids_again = store.consume_wakes("critic".into(), ts(101)).await.unwrap();
    assert!(ids_again.is_empty(), "consume_wakes must be idempotent");

    // Other agent's wake is untouched.
    let other_ids = store.consume_wakes("test".into(), ts(102)).await.unwrap();
    assert_eq!(other_ids, vec![other_wake.id]);

    // count_unread still sees the note (kind="note") for critic.
    assert_eq!(store.count_unread("critic".into()).await.unwrap(), 1);
}

#[tokio::test]
async fn insert_and_list_round_trip_in_reply_to() {
    let (_dir, store) = fresh_store().await;
    let parent = store
        .insert_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "first ping".into(),
            sent_at: ts(1),
            in_reply_to: None,
        })
        .await
        .unwrap();
    let reply = store
        .insert_message(NewMessage {
            from_agent: "b".into(),
            to_agent: "a".into(),
            kind: "note".into(),
            body: "pong".into(),
            sent_at: ts(2),
            in_reply_to: Some(parent.id),
        })
        .await
        .unwrap();
    assert_eq!(reply.in_reply_to, Some(parent.id));

    let listed = store
        .list_messages(ListMessagesOpts {
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap();
    let reply_row = listed.iter().find(|r| r.id == reply.id).unwrap();
    assert_eq!(reply_row.in_reply_to, Some(parent.id));
    let parent_row = listed.iter().find(|r| r.id == parent.id).unwrap();
    assert!(parent_row.in_reply_to.is_none());
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
                workspace_id: None,
                spell_run_id: None,
            })
            .await
            .unwrap();
    }
    let store = Store::open(&db_path).await.unwrap();
    let agents = store.list_agents().await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, "persist-1");
}

// ── orphan settlement on server restart ──────────────────────────────────

#[tokio::test]
async fn mark_orphan_agents_killed_only_alive_rows() {
    let (_dir, store) = fresh_store().await;
    store
        .record_agent_spawn(NewAgent {
            id: "alive-1".into(),
            cli: "claude".into(),
            role: "claude".into(),
            workspace: "/tmp/x".into(),
            spawned_at: ts(0),
            workspace_id: None,
            spell_run_id: None,
        })
        .await
        .unwrap();
    store
        .record_agent_spawn(NewAgent {
            id: "killed-1".into(),
            cli: "claude".into(),
            role: "claude".into(),
            workspace: "/tmp/y".into(),
            spawned_at: ts(0),
            workspace_id: None,
            spell_run_id: None,
        })
        .await
        .unwrap();
    store
        .record_agent_kill("killed-1".into(), ts(50))
        .await
        .unwrap();

    let n = store.mark_orphan_agents_killed(ts(100)).await.unwrap();
    assert_eq!(n, 1, "only the alive-1 row should be updated");

    let agents = store.list_agents().await.unwrap();
    let alive = agents.iter().find(|a| a.id == "alive-1").unwrap();
    let killed = agents.iter().find(|a| a.id == "killed-1").unwrap();
    assert_eq!(alive.killed_at, Some(ts(100)));
    assert_eq!(killed.killed_at, Some(ts(50)), "prior kill timestamp wins");
}

#[tokio::test]
async fn mark_orphan_recordings_finalized_only_live_rows() {
    let (_dir, store) = fresh_store().await;
    store
        .record_recording_start(NewRecording {
            id: "live".into(),
            agent_id: "a".into(),
            path: "/tmp/live.cast".into(),
            started_at: ts(0),
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();
    store
        .record_recording_start(NewRecording {
            id: "done".into(),
            agent_id: "a".into(),
            path: "/tmp/done.cast".into(),
            started_at: ts(0),
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();
    store
        .record_recording_finalize("done".into(), ts(50), 50, 7)
        .await
        .unwrap();

    let n = store
        .mark_orphan_recordings_finalized(ts(100))
        .await
        .unwrap();
    assert_eq!(n, 1, "only the live row should be settled");

    let live = store.get_recording("live".into()).await.unwrap().unwrap();
    assert_eq!(live.finalized_at, Some(ts(100)));
    // duration_ms is backfilled from the wall-clock span (finalized_at -
    // started_at = 100) so restart-orphan recordings aren't shown with no
    // duration in the Replays list; last_seq stays NULL (reattach treats
    // NULL as "replay from head"). See mark_orphan_recordings_finalized.
    assert_eq!(live.duration_ms, Some(100));
    assert!(live.last_seq.is_none());

    let done = store.get_recording("done".into()).await.unwrap().unwrap();
    assert_eq!(done.finalized_at, Some(ts(50)));
    assert_eq!(done.duration_ms, Some(50));
    assert_eq!(done.last_seq, Some(7));
}

// ── retention / prune (F5) ──────────────────────────────────────────────────

fn bb(path: &str, content: &str, at: i64) -> NewBlackboardOp {
    NewBlackboardOp {
        agent_id: Some("a".into()),
        op: "write".into(),
        path: path.into(),
        content: content.into(),
        sha256: format!("sha-{content}"),
        at,
    }
}

#[tokio::test]
async fn prune_keeps_every_load_bearing_row() {
    let (dir, store) = fresh_store().await;
    let cutoff = ts(1000); // rows with timestamp < cutoff are "old"

    // ── blackboard ──────────────────────────────────────────────────────
    // path "p": two old superseded rows + one recent latest → 2 deletable.
    store.insert_blackboard_op(bb("p", "p-v0", ts(0))).await.unwrap();
    store.insert_blackboard_op(bb("p", "p-v1", ts(100))).await.unwrap();
    store.insert_blackboard_op(bb("p", "p-v2", ts(2000))).await.unwrap();
    // path "q": single OLD row — it's the latest for q, must be KEPT despite age.
    store.insert_blackboard_op(bb("q", "q-v0", ts(0))).await.unwrap();
    // path "r": old superseded + old latest → only the superseded one deletable.
    store.insert_blackboard_op(bb("r", "r-v0", ts(0))).await.unwrap();
    store.insert_blackboard_op(bb("r", "r-v1", ts(50))).await.unwrap();

    // ── messages ────────────────────────────────────────────────────────
    let id = |m: MessageRecord| m.id;
    // m1: old normal note, delivered+read → deletable.
    let m1 = id(store.insert_message(NewMessage {
        from_agent: "x".into(), to_agent: "a".into(), kind: "note".into(),
        body: "old read".into(), sent_at: ts(0), in_reply_to: None,
    }).await.unwrap());
    store.mark_delivered(vec![m1], ts(1)).await.unwrap();
    store.mark_read(vec![m1], "a".into(), ts(2)).await.unwrap();
    // m2: old normal note, NOT delivered/read → KEPT (conservative).
    store.insert_message(NewMessage {
        from_agent: "x".into(), to_agent: "a".into(), kind: "note".into(),
        body: "old unread".into(), sent_at: ts(0), in_reply_to: None,
    }).await.unwrap();
    // m3: old wake, UN-consumed (different agent so consume below misses it) → KEPT.
    store.insert_message(NewMessage {
        from_agent: "x".into(), to_agent: "keep".into(), kind: "wake".into(),
        body: "pending wake".into(), sent_at: ts(0), in_reply_to: None,
    }).await.unwrap();
    // m4: old wake, consumed → deletable.
    store.insert_message(NewMessage {
        from_agent: "x".into(), to_agent: "cons".into(), kind: "wake".into(),
        body: "spent wake".into(), sent_at: ts(0), in_reply_to: None,
    }).await.unwrap();
    let consumed = store.consume_wakes("cons".into(), ts(3)).await.unwrap();
    assert_eq!(consumed.len(), 1, "only the 'cons' wake is consumed");
    // m5: recent normal note, delivered+read → KEPT (newer than cutoff).
    let m5 = id(store.insert_message(NewMessage {
        from_agent: "x".into(), to_agent: "a".into(), kind: "note".into(),
        body: "recent".into(), sent_at: ts(2000), in_reply_to: None,
    }).await.unwrap());
    store.mark_delivered(vec![m5], ts(2001)).await.unwrap();
    store.mark_read(vec![m5], "a".into(), ts(2002)).await.unwrap();
    // m6 parent + m7 child: both old, delivered+read. m6 is referenced by m7
    // → m6 KEPT this pass (FK-safe), m7 deletable.
    let m6 = id(store.insert_message(NewMessage {
        from_agent: "x".into(), to_agent: "a".into(), kind: "note".into(),
        body: "parent".into(), sent_at: ts(0), in_reply_to: None,
    }).await.unwrap());
    store.mark_delivered(vec![m6], ts(1)).await.unwrap();
    store.mark_read(vec![m6], "a".into(), ts(2)).await.unwrap();
    let m7 = id(store.insert_message(NewMessage {
        from_agent: "x".into(), to_agent: "a".into(), kind: "note".into(),
        body: "child".into(), sent_at: ts(0), in_reply_to: Some(m6),
    }).await.unwrap());
    store.mark_delivered(vec![m7], ts(1)).await.unwrap();
    store.mark_read(vec![m7], "a".into(), ts(2)).await.unwrap();

    // ── recordings ──────────────────────────────────────────────────────
    let cast = dir.path().join("rec1.cast");
    std::fs::write(&cast, b"cast bytes").unwrap();
    store.record_recording_start(NewRecording {
        id: "rec1".into(), agent_id: "a".into(), path: cast.to_string_lossy().into(),
        started_at: ts(0), cols: 80, rows: 24,
    }).await.unwrap();
    store.record_recording_finalize("rec1".into(), ts(10), 10, 1).await.unwrap();
    // rec2: old but LIVE (not finalized) → KEPT.
    store.record_recording_start(NewRecording {
        id: "rec2".into(), agent_id: "a".into(), path: "/tmp/rec2.cast".into(),
        started_at: ts(0), cols: 80, rows: 24,
    }).await.unwrap();
    // rec3: recent + finalized → KEPT.
    store.record_recording_start(NewRecording {
        id: "rec3".into(), agent_id: "a".into(), path: "/tmp/rec3.cast".into(),
        started_at: ts(2000), cols: 80, rows: 24,
    }).await.unwrap();
    store.record_recording_finalize("rec3".into(), ts(2010), 10, 1).await.unwrap();

    // ── prune ───────────────────────────────────────────────────────────
    let stats = store.prune_expired(cutoff).await.unwrap();
    assert_eq!(stats.blackboard_ops, 3, "p:2 + r:1 superseded-old rows");
    assert_eq!(stats.messages, 3, "m1 (read) + m4 (consumed wake) + m7 (child)");
    assert_eq!(stats.recordings, 1, "rec1 old+finalized");
    assert_eq!(stats.recording_files_removed, 1, "rec1.cast unlinked");

    // blackboard: all three paths still discoverable, latest content intact.
    let latest = store.list_blackboard_ops(None).await.unwrap();
    let mut paths: Vec<&str> = latest.iter().map(|r| r.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["p", "q", "r"], "no path lost discovery");
    let p_rows = store.list_blackboard_ops(Some("p".into())).await.unwrap();
    assert_eq!(p_rows.len(), 1, "p keeps only its latest row");
    assert_eq!(p_rows[0].content, "p-v2");

    // recordings: rec1 row + file gone; rec2 (live) + rec3 (recent) kept.
    assert!(!cast.exists(), "pruned .cast file removed from disk");
    assert!(store.get_recording("rec1".into()).await.unwrap().is_none());
    assert!(store.get_recording("rec2".into()).await.unwrap().is_some());
    assert!(store.get_recording("rec3".into()).await.unwrap().is_some());

    // messages: kept = m2 (unread), m3 (pending wake), m5 (recent), m6 (parent).
    let kept_a = store.list_messages(ListMessagesOpts {
        to_agent: Some("a".into()), limit: 100, ..Default::default()
    }).await.unwrap();
    let bodies: Vec<&str> = kept_a.iter().map(|m| m.body.as_str()).collect();
    assert!(bodies.contains(&"old unread"));
    assert!(bodies.contains(&"recent"));
    assert!(bodies.contains(&"parent"));
    assert!(!bodies.contains(&"old read"), "delivered+read old note pruned");
    assert!(!bodies.contains(&"child"), "child pruned");
    let pending = store.list_messages(ListMessagesOpts {
        to_agent: Some("keep".into()), limit: 100, ..Default::default()
    }).await.unwrap();
    assert_eq!(pending.len(), 1, "un-consumed wake must survive");

    // Second pass: m6 is now unreferenced (m7 gone) → ages out. Demonstrates
    // the FK-safe staged deletion documented on prune_expired.
    let stats2 = store.prune_expired(cutoff).await.unwrap();
    assert_eq!(stats2.messages, 1, "orphaned parent ages out next pass");
    assert_eq!(stats2.blackboard_ops, 0);
    assert_eq!(stats2.recordings, 0);
}
