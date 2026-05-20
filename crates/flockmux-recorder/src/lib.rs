//! asciicast v2 recorder.
//!
//! One [`Recorder`] per PTY. The recorder owns the .cast file on disk and a
//! background writer task. The PTY pump task obtains a cheap-to-clone
//! [`RecorderHandle`] and calls [`RecorderHandle::write_chunk`] for every
//! byte slice it reads from the PTY (including OSC lifecycle markers — the
//! replay must preserve them).
//!
//! Finalization is automatic: when every clone of the handle has been
//! dropped, the writer task flushes, closes the file, and resolves the
//! finalize oneshot. The caller awaits [`Recorder::wait_finalize`] in a
//! background task and persists the finalize op to SQLite.
//!
//! Spec reference: <https://docs.asciinema.org/manual/asciicast/v2/>

mod writer;

pub use writer::{FinalizeResult, Recorder, RecorderConfig, RecorderHandle};
