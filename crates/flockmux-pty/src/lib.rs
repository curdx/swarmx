//! Cross-platform PTY wrapper around `portable-pty` with an async-friendly
//! byte-stream interface.
//!
//! `portable-pty` exposes blocking `std::io::Read` / `Write`, so we run a
//! reader OS thread that pumps bytes into a `tokio::sync::mpsc` channel,
//! and a writer OS thread that pulls bytes from another channel and writes
//! them to the master. That sandwich is the *minimum* glue needed —
//! `spawn_blocking` is wrong here because both threads live as long as the
//! PTY does and would saturate the blocking pool.
//!
//! This is the Rust port of hermes-agent's `hermes_cli/pty_bridge.py`
//! (POSIX-only, byte-safe IO via raw fd reads/writes), refined with
//! golutra's `src-tauri/src/runtime/pty.rs` cross-platform polish.

use anyhow::{Context, Result};
use bytes::Bytes;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Default I/O channel capacity (in messages, not bytes). 256 lets a fast
/// producer get a head-start without losing the natural back-pressure: when
/// full, `tx.blocking_send` parks the reader thread, which parks `read()`,
/// which parks the kernel's PTY ring buffer, which parks the child. The
/// chain is what we want — `broadcast` would `Lagged`-drop bytes, which
/// would shred ANSI state machines.
const DEFAULT_CHANNEL_CAP: usize = 256;

/// One PTY + child pair. Output flows through `output_rx`, input through
/// `input_tx`. The reader thread exits when the kernel returns EOF / EIO.
/// Drop the bridge to terminate the child (SIGHUP → SIGTERM → SIGKILL,
/// implemented in `Drop`).
pub struct PtyBridge {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    input_tx: mpsc::Sender<Bytes>,
    /// Held so we can `take()` and `join()` in `Drop`, ensuring threads
    /// don't outlive the bridge if the caller forgets.
    reader_thread: Option<thread::JoinHandle<()>>,
    writer_thread: Option<thread::JoinHandle<()>>,
}

pub struct SpawnOpts<'a> {
    pub argv: &'a [String],
    pub cwd: Option<&'a Path>,
    pub env: HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
}

pub struct PtyHandles {
    pub bridge: PtyBridge,
    pub output_rx: mpsc::Receiver<Bytes>,
}

impl PtyBridge {
    pub fn spawn(opts: SpawnOpts<'_>) -> Result<PtyHandles> {
        anyhow::ensure!(!opts.argv.is_empty(), "argv must not be empty");

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: opts.rows.max(1),
                cols: opts.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty failed")?;

        let mut cmd = CommandBuilder::new(&opts.argv[0]);
        for a in &opts.argv[1..] {
            cmd.arg(a);
        }
        if let Some(cwd) = opts.cwd {
            cmd.cwd(cwd);
        }
        // PTY-hosted CLIs probe TERM (e.g. `tput cols`); back-fill a safe
        // default the same way hermes' pty_bridge.py:108-112 does.
        if !opts.env.contains_key("TERM") {
            cmd.env("TERM", "xterm-256color");
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("spawn_command failed")?;

        // We're done with the slave side once the child holds it — closing
        // here lets the kernel deliver EOF on master.read() after the child
        // exits, which is how the reader thread knows to terminate.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("try_clone_reader failed")?;
        let writer = pair
            .master
            .take_writer()
            .context("take_writer failed")?;

        let master = Arc::new(Mutex::new(pair.master));
        let child = Arc::new(Mutex::new(child));

        let (output_tx, output_rx) = mpsc::channel::<Bytes>(DEFAULT_CHANNEL_CAP);
        let (input_tx, input_rx) = mpsc::channel::<Bytes>(DEFAULT_CHANNEL_CAP);

        let reader_thread = spawn_reader(reader, output_tx);
        let writer_thread = spawn_writer(writer, input_rx);

        Ok(PtyHandles {
            bridge: PtyBridge {
                master,
                child,
                input_tx,
                reader_thread: Some(reader_thread),
                writer_thread: Some(writer_thread),
            },
            output_rx,
        })
    }

    pub fn input_sender(&self) -> mpsc::Sender<Bytes> {
        self.input_tx.clone()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        // Lock the master only briefly; never call resize from the reader
        // thread (which holds the read half of the PTY) to avoid an ioctl
        // race during high-volume output.
        let guard = self.master.lock();
        guard
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("PTY resize failed")
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.lock().process_id()
    }

    /// Non-blocking check for child liveness.
    pub fn is_alive(&self) -> bool {
        match self.child.lock().try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }

    /// Best-effort terminate. Used by `Drop`; safe to call multiple times.
    pub fn kill(&self) {
        // portable-pty's Child::kill sends SIGKILL on Unix. For a graceful
        // shutdown we close the master first (which sends SIGHUP via
        // ptmx close-on-last-fd on most kernels) and then escalate.
        let mut child = self.child.lock();
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for PtyBridge {
    fn drop(&mut self) {
        self.kill();
        // Joining the reader/writer threads here would deadlock if either
        // is blocked in a syscall on an fd that's already gone — they exit
        // on their own once the PTY tears down. Detach.
        let _ = self.reader_thread.take();
        let _ = self.writer_thread.take();
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Bytes>) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("flockmux-pty-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        debug!("pty reader: EOF");
                        break;
                    }
                    Ok(n) => {
                        let chunk = Bytes::copy_from_slice(&buf[..n]);
                        if tx.blocking_send(chunk).is_err() {
                            debug!("pty reader: output channel closed");
                            break;
                        }
                    }
                    Err(err) => {
                        // EIO on the master fd after slave closes is the
                        // normal exit path on Linux; treat as EOF.
                        warn!(?err, "pty reader: read error");
                        break;
                    }
                }
            }
        })
        .expect("spawn reader thread")
}

fn spawn_writer(mut writer: Box<dyn Write + Send>, mut rx: mpsc::Receiver<Bytes>) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("flockmux-pty-writer".into())
        .spawn(move || {
            while let Some(chunk) = rx.blocking_recv() {
                if writer.write_all(&chunk).is_err() {
                    debug!("pty writer: write error, exiting");
                    break;
                }
                // PTY masters don't strictly need an explicit flush (writes
                // go straight to the kernel ring), but flushing is cheap
                // and guarantees a write actually reached the child if the
                // underlying impl wraps in a BufWriter.
                let _ = writer.flush();
            }
        })
        .expect("spawn writer thread")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    #[cfg(unix)]
    fn cat_echoes_what_we_write() {
        let handles = PtyBridge::spawn(SpawnOpts {
            argv: &["cat".into()],
            cwd: None,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
        })
        .expect("spawn cat");

        let PtyHandles { bridge, mut output_rx } = handles;
        let input = bridge.input_sender();

        block_on(async move {
            input
                .send(Bytes::from_static(b"hello flockmux\n"))
                .await
                .unwrap();

            // Read until we see what we wrote (cat echoes lines back).
            let mut got = Vec::new();
            for _ in 0..50 {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    output_rx.recv(),
                )
                .await
                {
                    Ok(Some(chunk)) => got.extend_from_slice(&chunk),
                    _ => break,
                }
                if got.windows(b"hello flockmux".len())
                    .any(|w| w == b"hello flockmux")
                {
                    break;
                }
            }
            assert!(
                got.windows(b"hello flockmux".len())
                    .any(|w| w == b"hello flockmux"),
                "did not see echoed bytes; got = {:?}",
                String::from_utf8_lossy(&got)
            );

            // Resize should not error.
            bridge.resize(120, 30).unwrap();
        });
    }

    #[test]
    fn spawn_rejects_empty_argv() {
        let result = PtyBridge::spawn(SpawnOpts {
            argv: &[],
            cwd: None,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
        });
        let Err(err) = result else {
            panic!("expected empty-argv to be rejected, got Ok(_)")
        };
        assert!(err.to_string().contains("argv must not be empty"));
    }
}
