//! Swarm integration tests: send/receive via mpsc, blackboard write+read+DB,
//! path traversal rejection, and watcher external-edit detection.

use flockmux_protocol::ws_swarm::SwarmEvent;
use flockmux_storage::{ListMessagesOpts, Store};
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
        SwarmEvent::Message { id, from_agent, to_agent, body, .. } => {
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
        SwarmEvent::BlackboardChanged { op, path, agent_id, .. } => {
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
        msg.contains("..") || msg.contains("escape") || msg.contains("traversal") || msg.contains("rel path"),
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
    assert!(found, "watcher should have emitted external BlackboardChanged");
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
