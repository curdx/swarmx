//! JSON-RPC 2.0 over stdio — the single shared transport-plumbing component
//! for structured CLI protocols (ACP / Codex `app-server`).
//!
//! L4 foundation. The audit (and hermes's cautionary tale of reimplementing
//! this **three times**) called for factoring the JSON-RPC-over-stdio plumbing
//! into ONE place. This module is that place: a pure, IO-free codec that every
//! structured-transport CLI builds on. Reading the child's stdout and writing
//! its stdin is the caller's job; this module only turns bytes ⇆ messages and
//! hands out request ids.
//!
//! **Framing**: newline-delimited JSON — one complete JSON value per line, the
//! framing ACP and the Codex app-server speak over stdio. (Content-Length
//! framing, used by LSP, is intentionally NOT implemented yet; add a second
//! `Framing` variant when a CLI needs it rather than guessing now.)
//!
//! **Status**: codec + id allocation are complete and tested. Driving an
//! actual ACP *session* (initialize handshake, permission/tool-call event
//! mapping, streaming) is the next L4 increment and will be built on top of
//! this — see `spawn.rs` for the transport-selection seam. Until then PTY
//! remains the only wired transport (declaring `transport = "acp"` logs a
//! warning and falls back to PTY).

// The codec is a finished, tested foundation that nothing drives YET (the ACP
// session loop is the next increment). Allow dead_code so the unwired-but-ready
// component doesn't warn; remove this once spawn.rs builds a session on it.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};

pub const JSONRPC_VERSION: &str = "2.0";

/// A JSON-RPC request (has both `method` and `id` — expects a response).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC notification (has `method`, NO `id` — fire-and-forget).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC response (has `id` + exactly one of `result` / `error`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// One decoded JSON-RPC frame, classified by which fields are present.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

impl Request {
    pub fn new(id: impl Into<Value>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method: method.into(),
            params,
        }
    }
}

impl Message {
    /// Classify a parsed JSON value into a JSON-RPC message. Per the 2.0 spec:
    /// `method` present ⇒ request (has `id`) or notification (no `id`);
    /// otherwise `id` + `result`/`error` ⇒ response.
    pub fn from_value(v: Value) -> Result<Message, CodecError> {
        let obj = v.as_object().ok_or(CodecError::NotAnObject)?;
        let has_method = obj.contains_key("method");
        let has_id = obj.contains_key("id");
        if has_method {
            if has_id {
                Ok(Message::Request(
                    serde_json::from_value(v).map_err(CodecError::Json)?,
                ))
            } else {
                Ok(Message::Notification(
                    serde_json::from_value(v).map_err(CodecError::Json)?,
                ))
            }
        } else if has_id && (obj.contains_key("result") || obj.contains_key("error")) {
            Ok(Message::Response(
                serde_json::from_value(v).map_err(CodecError::Json)?,
            ))
        } else {
            Err(CodecError::Unclassifiable)
        }
    }

    /// Render this message to a single `\n`-terminated UTF-8 line, ready to
    /// write to the child's stdin.
    pub fn encode_line(&self) -> Vec<u8> {
        let mut bytes = match self {
            Message::Request(r) => serde_json::to_vec(r),
            Message::Notification(n) => serde_json::to_vec(n),
            Message::Response(r) => serde_json::to_vec(r),
        }
        .expect("JSON-RPC message serializes");
        bytes.push(b'\n');
        bytes
    }
}

#[derive(Debug)]
pub enum CodecError {
    /// The line parsed as JSON but wasn't an object.
    NotAnObject,
    /// A JSON object that is neither request, notification, nor response.
    Unclassifiable,
    /// The line wasn't valid JSON, or didn't fit the typed shape.
    Json(serde_json::Error),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::NotAnObject => write!(f, "JSON-RPC frame was not an object"),
            CodecError::Unclassifiable => {
                write!(f, "JSON object is neither request/notification/response")
            }
            CodecError::Json(e) => write!(f, "JSON-RPC parse error: {e}"),
        }
    }
}
impl std::error::Error for CodecError {}

/// Buffers raw stdout bytes from the child and yields complete JSON-RPC frames
/// as whole `\n`-delimited lines arrive. Partial trailing data is retained
/// across `push` calls; blank lines are skipped. Each yielded item is the
/// parse result for one line, so a single malformed frame doesn't poison the
/// stream — the caller logs it and keeps going.
#[derive(Debug, Default)]
pub struct LineDecoder {
    buf: Vec<u8>,
}

impl LineDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a chunk of stdout and return every complete frame it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Result<Message, CodecError>> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        // Drain complete lines, leaving any trailing partial in `self.buf`.
        loop {
            let Some(nl) = self.buf.iter().position(|&b| b == b'\n') else {
                break;
            };
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let trimmed = &line[..line.len().saturating_sub(1)]; // drop the '\n'
            let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed); // tolerate CRLF
            if trimmed.iter().all(|b| b.is_ascii_whitespace()) {
                continue; // skip blank/keepalive lines
            }
            out.push(
                serde_json::from_slice::<Value>(trimmed)
                    .map_err(CodecError::Json)
                    .and_then(Message::from_value),
            );
        }
        out
    }
}

/// Monotonic request-id allocator. JSON-RPC ids must be unique per pending
/// call on a connection; a process-lifetime atomic is the simplest correct
/// source. Ids are numbers (the form ACP / app-server use).
#[derive(Debug, Default)]
pub struct IdGen {
    next: AtomicI64,
}

impl IdGen {
    pub fn new() -> Self {
        Self {
            next: AtomicI64::new(1),
        }
    }

    /// Next unique id as a JSON value, ready to drop into [`Request::new`].
    pub fn next(&self) -> Value {
        Value::from(self.next.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_request_as_one_terminated_line() {
        let req = Request::new(1, "initialize", Some(json!({"v": 1})));
        let line = Message::Request(req).encode_line();
        assert_eq!(*line.last().unwrap(), b'\n');
        // round-trips back to the same request
        let back = Message::from_value(
            serde_json::from_slice(&line[..line.len() - 1]).unwrap(),
        )
        .unwrap();
        match back {
            Message::Request(r) => {
                assert_eq!(r.method, "initialize");
                assert_eq!(r.id, json!(1));
                assert_eq!(r.params, Some(json!({"v": 1})));
                assert_eq!(r.jsonrpc, "2.0");
            }
            other => panic!("expected request, got {other:?}"),
        }
    }

    #[test]
    fn classifies_all_three_kinds() {
        let req = Message::from_value(json!({"jsonrpc":"2.0","id":7,"method":"m"})).unwrap();
        assert!(matches!(req, Message::Request(_)));
        let note = Message::from_value(json!({"jsonrpc":"2.0","method":"update"})).unwrap();
        assert!(matches!(note, Message::Notification(_)));
        let ok = Message::from_value(json!({"jsonrpc":"2.0","id":7,"result":{"ok":true}})).unwrap();
        assert!(matches!(ok, Message::Response(_)));
        let err = Message::from_value(
            json!({"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"no method"}}),
        )
        .unwrap();
        match err {
            Message::Response(r) => {
                assert_eq!(r.error.unwrap().code, -32601);
                assert!(r.result.is_none());
            }
            other => panic!("expected error response, got {other:?}"),
        }
    }

    #[test]
    fn unclassifiable_and_non_object_error() {
        assert!(matches!(
            Message::from_value(json!({"jsonrpc": "2.0"})),
            Err(CodecError::Unclassifiable)
        ));
        assert!(matches!(
            Message::from_value(json!([1, 2, 3])),
            Err(CodecError::NotAnObject)
        ));
    }

    #[test]
    fn decoder_splits_lines_and_keeps_partial() {
        let mut dec = LineDecoder::new();
        // Two whole frames + the start of a third in one chunk.
        let got = dec.push(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"a\"}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1}\n{\"jsonrpc\":\"2.0\",",
        );
        assert_eq!(got.len(), 2, "two complete frames, partial buffered");
        assert!(matches!(got[0].as_ref().unwrap(), Message::Notification(_)));
        assert!(matches!(got[1].as_ref().unwrap(), Message::Response(_)));
        // Completing the partial yields the third.
        let more = dec.push(b"\"id\":2,\"method\":\"b\"}\n");
        assert_eq!(more.len(), 1);
        assert!(matches!(more[0].as_ref().unwrap(), Message::Request(_)));
    }

    #[test]
    fn decoder_tolerates_crlf_and_blank_lines() {
        let mut dec = LineDecoder::new();
        let got = dec.push(b"\r\n  \n{\"jsonrpc\":\"2.0\",\"method\":\"x\"}\r\n");
        assert_eq!(got.len(), 1, "blank + whitespace lines skipped, CRLF trimmed");
        assert!(matches!(got[0].as_ref().unwrap(), Message::Notification(_)));
    }

    #[test]
    fn decoder_surfaces_malformed_without_poisoning() {
        let mut dec = LineDecoder::new();
        let got = dec.push(b"not json\n{\"jsonrpc\":\"2.0\",\"method\":\"ok\"}\n");
        assert_eq!(got.len(), 2);
        assert!(got[0].is_err(), "bad line surfaces as Err");
        assert!(got[1].is_ok(), "good line after a bad one still parses");
    }

    #[test]
    fn id_gen_is_monotonic_and_unique() {
        let gen = IdGen::new();
        assert_eq!(gen.next(), json!(1));
        assert_eq!(gen.next(), json!(2));
        assert_eq!(gen.next(), json!(3));
    }
}
