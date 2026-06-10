//! Swarm integration tests: send/receive via mpsc, blackboard write+read+DB,
//! path traversal rejection, and watcher external-edit detection.

use flockmux_protocol::{rest::AgentActivityRecord, ws_swarm::SwarmEvent};
use flockmux_storage::{ListMessagesOpts, NewAgent, NewThread, NewWorker, NewWorkspace, Store};
use flockmux_swarm::{NewMessage, Swarm, WatcherHandle};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

async fn fresh() -> (TempDir, Arc<Swarm>) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("flockmux.db");
    let bb_root: PathBuf = dir.path().join("blackboard");
    std::fs::create_dir_all(&bb_root).unwrap();
    let store = Arc::new(Store::open(&db_path).await.unwrap());
    let swarm = Swarm::new(store, bb_root);
    (dir, swarm)
}

async fn fresh_threaded() -> (TempDir, Arc<Swarm>) {
    let (dir, swarm) = fresh().await;
    let ws = swarm
        .store()
        .create_workspace(
            NewWorkspace {
                name: "ws".into(),
                cwd: "/tmp/ws".into(),
                accent: None,
            },
            1,
        )
        .await
        .unwrap();
    let thread = swarm
        .store()
        .create_thread(
            NewThread {
                workspace_id: ws.id.clone(),
                slug: "main".into(),
                name: Some("main".into()),
                isolation: "shared".into(),
                branch: None,
                cwd: ws.cwd.clone(),
                state: "ready".into(),
            },
            1,
        )
        .await
        .unwrap();
    swarm
        .store()
        .record_agent_spawn(NewAgent {
            id: "orch".into(),
            cli: "codex".into(),
            role: "orchestrator".into(),
            workspace: ws.cwd,
            spawned_at: 1,
            workspace_id: Some(ws.id),
            spell_run_id: None,
            thread_id: Some(thread.id),
        })
        .await
        .unwrap();
    (dir, swarm)
}

#[tokio::test]
async fn register_send_receive() {
    let (_dir, swarm) = fresh().await;
    let mut rx_b = swarm.register_agent("b".into());
    let mut sub = swarm.subscribe();

    let rec = swarm
        .send_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "hi b".into(),
            sent_at: 1,
            in_reply_to: None,
            meta: None,
        })
        .await
        .unwrap();
    assert_eq!(rec.body, "hi b");

    let env = timeout(Duration::from_millis(500), rx_b.recv())
        .await
        .expect("inbox recv timed out")
        .expect("inbox closed");
    assert_eq!(env.id, rec.id);
    assert_eq!(env.body, "hi b");

    let ev = timeout(Duration::from_millis(500), sub.recv())
        .await
        .unwrap()
        .unwrap();
    match ev {
        SwarmEvent::Message {
            id,
            from_agent,
            to_agent,
            body,
            ..
        } => {
            assert_eq!(id, rec.id);
            assert_eq!(from_agent, "a");
            assert_eq!(to_agent, "b");
            assert_eq!(body, "hi b");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn message_persists_even_without_inbox() {
    // No `register_agent("b")` — the mpsc delivery should silently fail
    // but the SQLite row + ws/swarm broadcast must still happen.
    let (_dir, swarm) = fresh().await;
    let mut sub = swarm.subscribe();
    let rec = swarm
        .send_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "queued for later".into(),
            sent_at: 1,
            in_reply_to: None,
            meta: None,
        })
        .await
        .unwrap();

    let _ = timeout(Duration::from_millis(500), sub.recv())
        .await
        .expect("broadcast missed")
        .unwrap();

    let messages = swarm
        .store()
        .list_messages(ListMessagesOpts {
            to_agent: Some("b".into()),
            limit: 50,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, rec.id);
    assert!(
        messages[0].delivered_at.is_none(),
        "no inbox ⇒ delivered_at stays NULL"
    );
}

#[tokio::test]
async fn blackboard_write_read_roundtrip_and_db_record() {
    let (_dir, swarm) = fresh().await;
    let mut sub = swarm.subscribe();

    let rec = swarm
        .write_blackboard(Some("a".into()), "tasks.md", "- [ ] first\n")
        .await
        .unwrap();
    assert_eq!(rec.op, "write");
    assert_eq!(rec.path, "tasks.md");

    let got = swarm.read_blackboard("tasks.md").await.unwrap();
    assert_eq!(got.as_deref(), Some("- [ ] first\n"));

    let history = swarm
        .store()
        .list_blackboard_ops(Some("tasks.md".into()))
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].agent_id.as_deref(), Some("a"));

    let ev = timeout(Duration::from_millis(500), sub.recv())
        .await
        .unwrap()
        .unwrap();
    match ev {
        SwarmEvent::BlackboardChanged {
            op, path, agent_id, ..
        } => {
            assert_eq!(op, "write");
            assert_eq!(path, "tasks.md");
            assert_eq!(agent_id.as_deref(), Some("a"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn path_traversal_rejected() {
    let (_dir, swarm) = fresh().await;
    let err = swarm
        .write_blackboard(None, "../escape.md", "boom")
        .await
        .err()
        .expect("traversal should be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("..")
            || msg.contains("escape")
            || msg.contains("traversal")
            || msg.contains("rel path"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn watcher_records_external_edit() {
    let (dir, swarm) = fresh().await;
    let bb_root = dir.path().join("blackboard");
    let _watcher = WatcherHandle::spawn(bb_root.clone(), swarm.clone()).unwrap();
    let mut sub = swarm.subscribe();

    // Give the OS time to arm the watcher before writing.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // External write — *not* through Swarm::write_blackboard.
    std::fs::write(bb_root.join("external.md"), b"hand-edited\n").unwrap();

    // Wait up to 5s for a BlackboardChanged{op:"external"} event.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while std::time::Instant::now() < deadline {
        let res = timeout(Duration::from_millis(500), sub.recv()).await;
        match res {
            Ok(Ok(SwarmEvent::BlackboardChanged { op, path, .. }))
                if op == "external" && path == "external.md" =>
            {
                found = true;
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    assert!(
        found,
        "watcher should have emitted external BlackboardChanged"
    );
}

#[tokio::test]
async fn watcher_skips_self_write() {
    let (dir, swarm) = fresh().await;
    let bb_root = dir.path().join("blackboard");
    let _watcher = WatcherHandle::spawn(bb_root.clone(), swarm.clone()).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Our own write — should record one "write" op only, not an extra
    // "external" from the watcher.
    swarm
        .write_blackboard(Some("a".into()), "self.md", "from us\n")
        .await
        .unwrap();

    // Wait long enough for the debouncer's 150ms window + slack.
    tokio::time::sleep(Duration::from_millis(800)).await;

    let history = swarm
        .store()
        .list_blackboard_ops(Some("self.md".into()))
        .await
        .unwrap();
    let ops: Vec<&str> = history.iter().map(|r| r.op.as_str()).collect();
    assert_eq!(ops, vec!["write"], "watcher must not echo self-writes");
}

#[tokio::test]
async fn mark_read_broadcasts_event() {
    let (_dir, swarm) = fresh().await;
    let mut sub = swarm.subscribe();
    let m = swarm
        .send_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "ping".into(),
            sent_at: 1,
            in_reply_to: None,
            meta: None,
        })
        .await
        .unwrap();
    // Drain the Message broadcast first.
    let _ = timeout(Duration::from_millis(500), sub.recv()).await;

    let marked = swarm.mark_read("b".into(), vec![m.id]).await.unwrap();
    assert_eq!(marked, vec![m.id]);

    let ev = timeout(Duration::from_millis(500), sub.recv())
        .await
        .expect("event timed out")
        .unwrap();
    match ev {
        SwarmEvent::MessageRead { ids, to_agent, .. } => {
            assert_eq!(ids, vec![m.id]);
            assert_eq!(to_agent, "b");
        }
        other => panic!("unexpected: {other:?}"),
    }

    // Second call is idempotent — empty marked list, no broadcast.
    let again = swarm.mark_read("b".into(), vec![m.id]).await.unwrap();
    assert!(again.is_empty());
    let no_event = timeout(Duration::from_millis(150), sub.recv()).await;
    assert!(no_event.is_err(), "no broadcast when nothing changed");
}

#[tokio::test]
async fn send_message_with_in_reply_to_threads() {
    let (_dir, swarm) = fresh().await;
    let mut sub = swarm.subscribe();
    let parent = swarm
        .send_message(NewMessage {
            from_agent: "a".into(),
            to_agent: "b".into(),
            kind: "note".into(),
            body: "first".into(),
            sent_at: 1,
            in_reply_to: None,
            meta: None,
        })
        .await
        .unwrap();
    // Drain parent's broadcast.
    let _ = timeout(Duration::from_millis(500), sub.recv()).await;

    let reply = swarm
        .send_message(NewMessage {
            from_agent: "b".into(),
            to_agent: "a".into(),
            kind: "note".into(),
            body: "pong".into(),
            sent_at: 2,
            in_reply_to: Some(parent.id),
            meta: None,
        })
        .await
        .unwrap();
    assert_eq!(reply.in_reply_to, Some(parent.id));

    let ev = timeout(Duration::from_millis(500), sub.recv())
        .await
        .unwrap()
        .unwrap();
    match ev {
        SwarmEvent::Message { in_reply_to, .. } => {
            assert_eq!(in_reply_to, Some(parent.id))
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn user_and_agent_messages_auto_attach_thought_trace() {
    let (_dir, swarm) = fresh_threaded().await;
    let mut sub = swarm.subscribe();

    let user = swarm
        .send_message(NewMessage {
            from_agent: "user".into(),
            to_agent: "orch".into(),
            kind: "task".into(),
            body: "请处理".into(),
            sent_at: 10,
            in_reply_to: None,
            meta: None,
        })
        .await
        .unwrap();
    assert!(
        user.thought_trace.is_some(),
        "user trigger gets active trace"
    );
    let _ = timeout(Duration::from_millis(500), sub.recv())
        .await
        .unwrap()
        .unwrap();

    let reply = swarm
        .send_message(NewMessage {
            from_agent: "orch".into(),
            to_agent: "user".into(),
            kind: "reply".into(),
            body: "已处理".into(),
            sent_at: 25,
            in_reply_to: Some(user.id),
            meta: None,
        })
        .await
        .unwrap();
    let trace = reply.thought_trace.expect("reply gets completed trace");
    assert_eq!(trace.status, "done");
    assert_eq!(trace.response_message_id, Some(reply.id));

    let ev = timeout(Duration::from_millis(500), sub.recv())
        .await
        .unwrap()
        .unwrap();
    match ev {
        SwarmEvent::Message { thought_trace, .. } => {
            let trace = thought_trace.expect("ws event carries trace");
            assert_eq!(trace.status, "done");
        }
        other => panic!("unexpected: {other:?}"),
    }

    let rows = swarm
        .store()
        .list_messages(ListMessagesOpts {
            to_agent: Some("user".into()),
            from_agent: None,
            only_undelivered: false,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(rows[0].id, reply.id);
    assert!(rows[0].thought_trace.is_some());
}

#[tokio::test]
async fn worker_activity_appends_to_parent_thought_trace() {
    let (_dir, swarm) = fresh_threaded().await;
    let agents = swarm.store().list_agents().await.unwrap();
    let orch = agents.iter().find(|a| a.id == "orch").unwrap();

    swarm
        .store()
        .record_agent_spawn(NewAgent {
            id: "worker-a".into(),
            cli: "codex".into(),
            role: "implementer".into(),
            workspace: orch.workspace.clone(),
            spawned_at: 2,
            workspace_id: orch.workspace_id.clone(),
            spell_run_id: None,
            thread_id: orch.thread_id.clone(),
        })
        .await
        .unwrap();
    swarm
        .store()
        .record_worker(NewWorker {
            agent_id: "worker-a".into(),
            parent_agent_id: "orch".into(),
            role_label: "implementer".into(),
            system_prompt: "do work".into(),
            handoff_signal: "ws/main/implementer.done".into(),
            depends_on_json: "[]".into(),
            spawned_at: 2,
            role_slug: "implementer".into(),
            produces_json: "[\"done\"]".into(),
            consumes_json: "[]".into(),
        })
        .await
        .unwrap();

    let user = swarm
        .send_message(NewMessage {
            from_agent: "user".into(),
            to_agent: "orch".into(),
            kind: "task".into(),
            body: "请实现".into(),
            sent_at: 10,
            in_reply_to: None,
            meta: None,
        })
        .await
        .unwrap();
    swarm.record_activity(
        "worker-a",
        AgentActivityRecord {
            agent_id: "worker-a".into(),
            kind: "tool".into(),
            label: "Edit web/src/App.tsx".into(),
            phase: "ok".into(),
            seq: 1,
            duration_ms: Some(123),
            at: 20,
        },
    );
    tokio::time::sleep(Duration::from_millis(100)).await;

    let reply = swarm
        .send_message(NewMessage {
            from_agent: "orch".into(),
            to_agent: "user".into(),
            kind: "reply".into(),
            body: "已完成".into(),
            sent_at: 30,
            in_reply_to: Some(user.id),
            meta: None,
        })
        .await
        .unwrap();
    let trace = reply.thought_trace.expect("parent trace completed");
    assert!(
        trace
            .summary_json
            .contains("完成工具: Edit web/src/App.tsx"),
        "worker tool activity should be preserved in parent trace summary: {}",
        trace.summary_json
    );
}

// F6 completion: a file left on disk by a crash mid-write (content present but
// op-log row missing) must be backfilled at boot so it's discoverable again.
#[tokio::test]
async fn reconcile_backfills_only_orphaned_files() {
    let (dir, swarm) = fresh().await;

    // A normal write — gets an op-log row through the happy path.
    swarm
        .write_blackboard(Some("a".into()), "normal.md", "hi")
        .await
        .unwrap();

    // An orphaned file: written straight to disk, NO op-log row. This is what
    // write_blackboard leaves behind when the insert fails after the fs::write.
    let bb = dir.path().join("blackboard");
    std::fs::write(bb.join("orphan.md"), b"orphan-content").unwrap();

    // Reconcile backfills ONLY the orphan — normal.md already has a row.
    let n = swarm.reconcile_oplog_from_disk().await.unwrap();
    assert_eq!(n, 1, "only the orphaned file should be backfilled");

    // Idempotent: a second pass finds nothing new.
    assert_eq!(
        swarm.reconcile_oplog_from_disk().await.unwrap(),
        0,
        "second reconcile is a no-op"
    );

    // The backfilled key is readable (content was never lost — only the row).
    assert_eq!(
        swarm.read_blackboard("orphan.md").await.unwrap().as_deref(),
        Some("orphan-content")
    );

    // And it is now present in the op-log discovery surface.
    let paths: Vec<String> = swarm
        .store()
        .list_blackboard_ops(None)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert!(paths.contains(&"orphan.md".to_string()));
    assert!(paths.contains(&"normal.md".to_string()));
}
