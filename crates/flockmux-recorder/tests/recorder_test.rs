//! Integration tests for the asciicast v2 recorder.

use bytes::Bytes;
use flockmux_recorder::{Recorder, RecorderConfig};
use serde_json::Value;
use tempfile::TempDir;

async fn read_cast(path: &std::path::Path) -> Vec<String> {
    let text = tokio::fs::read_to_string(path).await.unwrap();
    text.lines().map(|l| l.to_string()).collect()
}

#[tokio::test]
async fn header_then_events_then_finalize() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.cast");
    let started_at_ms = 1_700_000_000_000_i64;
    let rec = Recorder::start(RecorderConfig {
        agent_id: "a-1".into(),
        cols: 120,
        rows: 32,
        started_at_ms,
        file_path: path.clone(),
    })
    .await
    .expect("start recorder");

    let h = rec.handle();
    h.write_chunk(Bytes::from_static(b"hello"));
    h.write_chunk(Bytes::from_static(b" world\n"));
    // Drop the pump's handle so the writer task sees EOF.
    drop(h);

    let fin = rec.wait_finalize().await.expect("finalize");
    assert_eq!(fin.last_seq, 12, "5 + 7 = 12 bytes recorded");
    assert!(fin.duration_ms >= 0);

    let lines = read_cast(&path).await;
    assert!(lines.len() >= 3, "header + 2 events, got {}", lines.len());

    // Header is JSON object with version=2, width/height/timestamp.
    let header: Value = serde_json::from_str(&lines[0]).expect("header is JSON");
    assert_eq!(header["version"], 2);
    assert_eq!(header["width"], 120);
    assert_eq!(header["height"], 32);
    assert_eq!(header["timestamp"], started_at_ms / 1000);

    // Events are JSON arrays of [number, "o", string].
    let ev1: Value = serde_json::from_str(&lines[1]).expect("event1 is JSON");
    let arr = ev1.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert!(arr[0].is_number(), "delta is a number");
    assert_eq!(arr[1], "o");
    assert_eq!(arr[2], "hello");

    let ev2: Value = serde_json::from_str(&lines[2]).expect("event2 is JSON");
    let arr = ev2.as_array().unwrap();
    assert_eq!(arr[2], " world\n");
}

#[tokio::test]
async fn empty_recording_yields_only_header() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.cast");
    let rec = Recorder::start(RecorderConfig {
        agent_id: "a-empty".into(),
        cols: 80,
        rows: 24,
        started_at_ms: 1_700_000_000_000,
        file_path: path.clone(),
    })
    .await
    .unwrap();
    // Don't take a handle for the pump — just finalize immediately.
    let fin = rec.wait_finalize().await.unwrap();
    assert_eq!(fin.last_seq, 0);

    let lines = read_cast(&path).await;
    assert_eq!(lines.len(), 1, "only the header line");
    let h: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(h["version"], 2);
}

#[tokio::test]
async fn invalid_utf8_replaced_lossy() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("binary.cast");
    let rec = Recorder::start(RecorderConfig {
        agent_id: "a-bin".into(),
        cols: 80,
        rows: 24,
        started_at_ms: 1_700_000_000_000,
        file_path: path.clone(),
    })
    .await
    .unwrap();
    let h = rec.handle();
    // 0xFF is invalid UTF-8 on its own. String::from_utf8_lossy replaces
    // it with U+FFFD (REPLACEMENT CHARACTER, 3 bytes UTF-8 = ef bf bd).
    h.write_chunk(Bytes::from_static(b"ok\xffend"));
    drop(h);
    let fin = rec.wait_finalize().await.unwrap();
    assert_eq!(fin.last_seq, 6);

    let lines = read_cast(&path).await;
    let ev: Value = serde_json::from_str(&lines[1]).unwrap();
    let s = ev[2].as_str().unwrap();
    assert!(s.starts_with("ok"));
    assert!(s.ends_with("end"));
    assert!(s.contains('\u{FFFD}'), "replacement char inserted, got {s:?}");
}

#[tokio::test]
async fn deltas_are_monotonically_nondecreasing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("delta.cast");
    let rec = Recorder::start(RecorderConfig {
        agent_id: "a-delta".into(),
        cols: 80,
        rows: 24,
        started_at_ms: 1_700_000_000_000,
        file_path: path.clone(),
    })
    .await
    .unwrap();
    let h = rec.handle();
    h.write_chunk(Bytes::from_static(b"first"));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    h.write_chunk(Bytes::from_static(b"second"));
    drop(h);
    rec.wait_finalize().await.unwrap();

    let lines = read_cast(&path).await;
    let d1: f64 = serde_json::from_str::<Value>(&lines[1]).unwrap()[0]
        .as_f64()
        .unwrap();
    let d2: f64 = serde_json::from_str::<Value>(&lines[2]).unwrap()[0]
        .as_f64()
        .unwrap();
    assert!(d2 >= d1, "{} >= {}", d2, d1);
    assert!(d2 - d1 >= 0.015, "~20ms gap shows up, got {}", d2 - d1);
}
