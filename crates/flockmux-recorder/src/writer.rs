//! asciicast v2 writer: header line + per-chunk `[delta, "o", data]` events.
//!
//! All disk I/O lives on a dedicated tokio task so the PTY pump never
//! blocks. The channel is unbounded — local-disk writes are fast enough
//! that bounding it would just hide a bug if it ever fell behind, and the
//! pump is the slow consumer of the PTY anyway.

use anyhow::{Context, Result};
use bytes::Bytes;
use serde::Serialize;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{mpsc, oneshot};

/// Config for opening a new recording.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub agent_id: String,
    pub cols: u16,
    pub rows: u16,
    /// Unix-ms timestamp the recording is logically anchored to. The cast
    /// header `timestamp` field is derived from this (asciicast v2 stores
    /// seconds-since-epoch). All per-event deltas are measured from
    /// [`Recorder::start`] returning, not from this value.
    pub started_at_ms: i64,
    pub file_path: PathBuf,
}

/// Result emitted by the writer task when the recording finalizes.
#[derive(Debug, Clone)]
pub struct FinalizeResult {
    pub finalized_at_ms: i64,
    pub duration_ms: i64,
    /// Total bytes recorded across every chunk (== sum of chunk lengths).
    pub last_seq: i64,
}

/// Cheap-to-clone push-side handle. The PTY pump holds at least one of
/// these; cloning it adds another producer. When every clone is dropped
/// the writer task observes EOF on the channel and finalizes.
#[derive(Clone)]
pub struct RecorderHandle {
    tx: mpsc::UnboundedSender<Bytes>,
}

impl RecorderHandle {
    /// Non-blocking enqueue. Silently no-ops if the writer task has
    /// already exited (e.g. earlier disk error).
    pub fn write_chunk(&self, chunk: Bytes) {
        // `send` on UnboundedSender only errors if the receiver is gone.
        let _ = self.tx.send(chunk);
    }
}

/// The recorder. The constructor opens the file + writes the header
/// synchronously (before returning) so spawn-time errors surface
/// immediately. The background writer task starts before the constructor
/// returns and runs until every [`RecorderHandle`] is dropped.
pub struct Recorder {
    /// The "owner" handle — the constructor returns one *implicit* handle
    /// inside the struct so callers must explicitly call [`Self::handle`]
    /// to get one for the pump. Dropping `Recorder` (or calling
    /// [`Self::wait_finalize`]) closes this owner copy.
    owner_handle: RecorderHandle,
    finalize_rx: oneshot::Receiver<FinalizeResult>,
}

impl Recorder {
    /// Open the .cast file, write the asciicast v2 header, and spawn the
    /// writer task. Errors before the writer task starts: file open / header
    /// write / parent directory missing.
    pub async fn start(cfg: RecorderConfig) -> Result<Self> {
        if let Some(parent) = cfg.file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create recording dir {}", parent.display()))?;
        }
        let f = File::create(&cfg.file_path)
            .await
            .with_context(|| format!("create cast file {}", cfg.file_path.display()))?;
        let mut writer = BufWriter::new(f);

        // asciicast v2 header. `env` is optional but most players display
        // SHELL/TERM in their info pane; we synthesize sane defaults.
        let header = CastHeader {
            version: 2,
            width: cfg.cols,
            height: cfg.rows,
            timestamp: (cfg.started_at_ms / 1000).max(0) as u64,
            env: HeaderEnv {
                shell: "/bin/sh",
                term: "xterm-256color",
            },
        };
        let header_json = serde_json::to_string(&header).context("serialize cast header")?;
        writer
            .write_all(header_json.as_bytes())
            .await
            .context("write cast header")?;
        writer.write_all(b"\n").await.context("write header newline")?;
        writer.flush().await.context("flush cast header")?;

        let (tx, rx) = mpsc::unbounded_channel::<Bytes>();
        let (fin_tx, fin_rx) = oneshot::channel::<FinalizeResult>();
        let path = cfg.file_path.clone();
        let started_at_ms = cfg.started_at_ms;

        tokio::spawn(writer_loop(writer, rx, fin_tx, path, started_at_ms));

        Ok(Self {
            owner_handle: RecorderHandle { tx },
            finalize_rx: fin_rx,
        })
    }

    /// Clone of the producer handle. Hand one to the PTY pump.
    pub fn handle(&self) -> RecorderHandle {
        self.owner_handle.clone()
    }

    /// Drop the owner-side handle and await finalization. The writer task
    /// only completes after every other [`RecorderHandle`] clone (typically
    /// held by the PTY pump) has also been dropped.
    pub async fn wait_finalize(self) -> Result<FinalizeResult> {
        drop(self.owner_handle);
        self.finalize_rx
            .await
            .context("recorder finalize oneshot dropped before completion")
    }
}

async fn writer_loop(
    mut writer: BufWriter<File>,
    mut rx: mpsc::UnboundedReceiver<Bytes>,
    fin_tx: oneshot::Sender<FinalizeResult>,
    path: PathBuf,
    started_at_ms: i64,
) {
    // Monotonic clock for per-event deltas — robust to wall-clock jumps.
    let start_instant = Instant::now();
    let mut last_seq: i64 = 0;
    let mut io_failed = false;

    while let Some(chunk) = rx.recv().await {
        if io_failed {
            // Keep draining so the channel can close cleanly, but skip the
            // disk writes — we've already lost write integrity.
            last_seq += chunk.len() as i64;
            continue;
        }
        last_seq += chunk.len() as i64;
        let delta = start_instant.elapsed().as_secs_f64();
        // asciicast v2 requires `data` to be a valid UTF-8 string. PTY
        // output is *usually* UTF-8 already; lossy conversion replaces
        // any stray non-UTF-8 bytes with U+FFFD so the file stays parseable.
        let data = String::from_utf8_lossy(&chunk);
        let event: (f64, &str, &str) = (delta, "o", &data);
        let line = match serde_json::to_string(&event) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(?e, path = %path.display(), "serialize cast event");
                continue;
            }
        };
        if let Err(e) = writer.write_all(line.as_bytes()).await {
            tracing::warn!(?e, path = %path.display(), "write cast line");
            io_failed = true;
            continue;
        }
        if let Err(e) = writer.write_all(b"\n").await {
            tracing::warn!(?e, path = %path.display(), "write cast newline");
            io_failed = true;
            continue;
        }
        // asciicast files are commonly tailed live (asciinema-player can
        // stream a growing file). Without per-event flush the BufWriter
        // holds everything until shutdown — so a live recording looks
        // empty on disk until the agent exits.
        if let Err(e) = writer.flush().await {
            tracing::warn!(?e, path = %path.display(), "flush cast event");
            io_failed = true;
            continue;
        }
    }
    if let Err(e) = writer.flush().await {
        tracing::warn!(?e, path = %path.display(), "flush cast on close");
    }
    if let Err(e) = writer.shutdown().await {
        tracing::debug!(?e, path = %path.display(), "shutdown cast on close");
    }
    let duration_ms = start_instant.elapsed().as_millis() as i64;
    let finalized_at_ms = now_ms().max(started_at_ms);
    let _ = fin_tx.send(FinalizeResult {
        finalized_at_ms,
        duration_ms,
        last_seq,
    });
}

#[derive(Debug, Serialize)]
struct CastHeader<'a> {
    version: u32,
    width: u16,
    height: u16,
    timestamp: u64,
    env: HeaderEnv<'a>,
}

#[derive(Debug, Serialize)]
struct HeaderEnv<'a> {
    #[serde(rename = "SHELL")]
    shell: &'a str,
    #[serde(rename = "TERM")]
    term: &'a str,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
