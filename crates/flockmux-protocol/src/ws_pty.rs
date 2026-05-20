//! `/ws/pty/:agent_id` frame schema.
//!
//! Binary frames carry PTY bytes prefixed by a 4-byte big-endian seq counter:
//!
//!     [seq(u32 BE)][raw PTY bytes ...]
//!
//! Text frames carry JSON control messages — resize / ack / resume / signal /
//! kill / detach. Two separate frame types because:
//!  * PTY output is byte-oriented (UTF-8 boundaries can land mid-read).
//!  * Control messages are structured; embedding them in the byte stream
//!    (like hermes' `\x1b[RESIZE]` regex) breaks under high-volume traffic.

use serde::{Deserialize, Serialize};

/// Client → server control messages (text frame).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientControl {
    /// Browser viewport changed; forward to PTY via TIOCSWINSZ.
    Resize { cols: u16, rows: u16 },
    /// Confirms client has received bytes up to and including `seq`.
    /// Server uses this to advance its ring buffer's confirmed cursor.
    Ack { seq: u32 },
    /// Reconnecting client lost the trailing bytes since `last_seq`;
    /// server should replay `last_seq+1..=current` from the ring buffer.
    Resume { last_seq: u32 },
    /// Forward a signal to the child process.
    Signal { sig: Signal },
    /// SIGHUP→SIGTERM→SIGKILL teardown.
    Kill,
    /// Close the WebSocket but keep the PTY alive in the background.
    Detach,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Signal {
    Sigint,
    Sigterm,
    Sighup,
}

/// Server → client control messages (text frame). PTY bytes never come here —
/// they come on binary frames with seq prefix.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerControl {
    /// First frame after accept. `seq_start` is the seq of the FIRST byte
    /// the client will receive after Hello. For a fresh attach this equals
    /// the producer's next_seq (live-tail; no replay). For a successful
    /// resume after `?last_seq=X`, this equals `X + 1`. If the server
    /// could not honour the resume (gap — buffer evicted the requested
    /// seq), `seq_start` will be greater than `X + 1` and the client
    /// should treat the missing range as unrecoverable scrollback.
    ///
    /// `shim_ready` / `shim_exit` carry the agent's lifecycle state at the
    /// time of attach so that a reconnecting client doesn't have to wait
    /// for the next ShimReady/ShimExit event to know the CLI's status.
    Hello {
        seq_start: u32,
        agent_id: String,
        #[serde(default)]
        shim_ready: bool,
        #[serde(default)]
        shim_exit: Option<i32>,
    },
    /// Shim emitted the OSC ready sequence; CLI is up.
    ShimReady,
    /// Shim emitted the OSC exit sequence; CLI exited.
    ShimExit { code: i32 },
    /// PTY child died unexpectedly (no exit code from shim).
    Eof,
    /// Generic error to surface in UI.
    Error { message: String },
}

/// Pack a (seq, bytes) tuple into a binary frame.
pub fn pack_binary(seq: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Unpack a client→server binary frame (rare — we only expect bytes from
/// the *server* side, but client may echo keystrokes as binary too).
pub fn unpack_binary(buf: &[u8]) -> Option<(u32, &[u8])> {
    if buf.len() < 4 {
        return None;
    }
    let seq = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    Some((seq, &buf[4..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_then_unpack_round_trips() {
        let payload = b"hello";
        let frame = pack_binary(42, payload);
        let (seq, body) = unpack_binary(&frame).unwrap();
        assert_eq!(seq, 42);
        assert_eq!(body, payload);
    }

    #[test]
    fn control_json_round_trips() {
        let msg = ClientControl::Resize { cols: 120, rows: 40 };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"resize","cols":120,"rows":40}"#);
        let back: ClientControl = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ClientControl::Resize { cols: 120, rows: 40 }));
    }
}
