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
//! mapping, streaming) is the next L4 increment.
//!
//! **Status**: the codec AND the async [`Connection`] layer (JSON-RPC
//! request/response correlation over a child's stdio + notification / peer-
//! request channels) are complete and tested (with a simulated peer). What
//! remains is the ACP-SPECIFIC session on top — the `initialize` handshake and
//! mapping ACP notifications onto flockmux `SwarmEvent`s — plus a piped-stdio
//! spawn path in `spawn.rs` (today the transport seam still falls back to PTY).
//! Those need a live ACP CLI to pin the wire schema; see the `spawn.rs` seam.

// Built + tested foundation that nothing drives YET (the ACP session layer is
// the next increment, gated on a live CLI). Allow dead_code so the
// unwired-but-ready component doesn't warn; remove once spawn.rs drives it.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

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
    ///
    /// Codex app-server uses JSON-RPC-shaped messages over JSONL but omits the
    /// `"jsonrpc":"2.0"` field on the wire. Normalize that field before typed
    /// deserialization so the same codec can drive both strict ACP peers and
    /// Codex app-server without duplicating transport code.
    pub fn from_value(v: Value) -> Result<Message, CodecError> {
        let mut v = v;
        let obj = v.as_object_mut().ok_or(CodecError::NotAnObject)?;
        obj.entry("jsonrpc")
            .or_insert_with(|| Value::String(JSONRPC_VERSION.into()));
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

/// Why a `request()` didn't get a result.
#[derive(Debug)]
pub enum ConnError {
    /// The peer answered with a JSON-RPC error object.
    Rpc(RpcError),
    /// The connection's reader task ended (EOF / IO error) before a response
    /// arrived, or a write failed — the peer is gone.
    Closed,
}

impl std::fmt::Display for ConnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnError::Rpc(e) => write!(f, "JSON-RPC error {}: {}", e.code, e.message),
            ConnError::Closed => write!(f, "JSON-RPC connection closed"),
        }
    }
}
impl std::error::Error for ConnError {}

/// What [`Connection::spawn`] hands back: the connection handle plus the two
/// inbound streams a client cares about — peer **notifications** (e.g. ACP
/// `session/update` streaming) and peer-initiated **requests** (e.g. an ACP
/// server asking the client for a permission decision, which the client must
/// answer with a [`Response`] — wiring that answer back is the session layer's
/// job, built on top of this).
pub struct ConnectionHandles {
    pub conn: Connection,
    pub notifications: mpsc::UnboundedReceiver<Notification>,
    pub incoming_requests: mpsc::UnboundedReceiver<Request>,
}

/// An async JSON-RPC 2.0 connection over a child's stdio (or any
/// AsyncRead/AsyncWrite pair). Built on the codec + [`LineDecoder`] — the
/// SINGLE place flockmux speaks JSON-RPC-over-stdio, so a future ACP /
/// app-server transport doesn't reimplement framing + id-correlation (hermes's
/// cautionary 3×). Protocol-agnostic: correlates responses to outbound
/// requests by id, surfaces inbound notifications + requests on channels, and
/// serializes concurrent writes behind an async mutex.
///
/// NOT here yet (the ACP-specific session layer — the final L4 step, which
/// needs a live ACP CLI to pin the wire schema): the `initialize` handshake +
/// capability params, and mapping ACP notifications (permission / tool-call /
/// streaming) onto flockmux `SwarmEvent`s + the `spawn.rs` transport branch.
/// Those build ON this Connection.
pub struct Connection {
    writer: Arc<AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Response>>>>,
    next_id: AtomicI64,
    /// Reader task handle — aborted when the Connection is dropped.
    reader: JoinHandle<()>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl Connection {
    /// Start a connection: spawn a reader task that decodes frames off `reader`
    /// and routes them (responses → the matching `request()` future,
    /// notifications + peer requests → the returned channels). Writes go out
    /// through `writer`.
    pub fn spawn<R, W>(reader: R, writer: W) -> ConnectionHandles
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (notif_tx, notif_rx) = mpsc::unbounded_channel();
        let (req_tx, req_rx) = mpsc::unbounded_channel();

        let pending_r = pending.clone();
        let reader_task = tokio::spawn(async move {
            let mut reader = reader;
            let mut dec = LineDecoder::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = match reader.read(&mut chunk).await {
                    Ok(0) | Err(_) => break, // EOF or IO error → peer gone
                    Ok(n) => n,
                };
                for msg in dec.push(&chunk[..n]) {
                    match msg {
                        Ok(Message::Response(r)) => {
                            if let Some(id) = r.id.as_i64() {
                                if let Some(tx) = pending_r.lock().unwrap().remove(&id) {
                                    let _ = tx.send(r);
                                }
                            }
                        }
                        Ok(Message::Notification(n)) => {
                            let _ = notif_tx.send(n);
                        }
                        Ok(Message::Request(req)) => {
                            let _ = req_tx.send(req);
                        }
                        Err(e) => tracing::warn!(?e, "acp: dropping malformed JSON-RPC frame"),
                    }
                }
            }
            // Peer gone: drop every pending sender so awaiting `request()`s get
            // `ConnError::Closed` instead of hanging forever.
            pending_r.lock().unwrap().clear();
        });

        let conn = Connection {
            writer: Arc::new(AsyncMutex::new(Box::new(writer))),
            pending,
            next_id: AtomicI64::new(1),
            reader: reader_task,
        };
        ConnectionHandles {
            conn,
            notifications: notif_rx,
            incoming_requests: req_rx,
        }
    }

    /// Send a request and await its correlated response. Resolves to the
    /// `result` value, or `ConnError::Rpc` if the peer returned an error, or
    /// `ConnError::Closed` if the peer went away first.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, ConnError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self
            .write_msg(&Message::Request(Request::new(id, method, params)))
            .await
        {
            self.pending.lock().unwrap().remove(&id); // un-register; no response coming
            return Err(e);
        }
        match rx.await {
            Ok(resp) => match resp.error {
                Some(err) => Err(ConnError::Rpc(err)),
                None => Ok(resp.result.unwrap_or(Value::Null)),
            },
            Err(_) => Err(ConnError::Closed), // reader dropped our pending sender
        }
    }

    /// Fire a notification (no response expected).
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), ConnError> {
        self.write_msg(&Message::Notification(Notification::new(method, params)))
            .await
    }

    /// Answer a peer-initiated request (e.g. an ACP permission prompt) by
    /// writing a [`Response`] carrying the original request's id.
    pub async fn respond(&self, resp: Response) -> Result<(), ConnError> {
        self.write_msg(&Message::Response(resp)).await
    }

    async fn write_msg(&self, m: &Message) -> Result<(), ConnError> {
        let line = m.encode_line();
        let mut w = self.writer.lock().await;
        w.write_all(&line).await.map_err(|_| ConnError::Closed)?;
        w.flush().await.map_err(|_| ConnError::Closed)?;
        Ok(())
    }
}

/// Minimal typed session driver for Codex `app-server`. This is the first
/// structured-transport session layer above [`Connection`]: it owns the
/// required initialize/initialized handshake and the core thread/turn calls.
/// Spawn integration stays opt-in/future; this module gives that branch a
/// tested, small interface instead of open-coded JSON-RPC calls.
pub struct CodexAppServerSession {
    conn: Connection,
    notifications: mpsc::UnboundedReceiver<Notification>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexAppEvent {
    TurnStarted {
        turn_id: Option<String>,
    },
    AgentMessageDelta {
        text: String,
    },
    ItemStarted {
        item_id: Option<String>,
        item_type: Option<String>,
    },
    ItemCompleted {
        item_id: Option<String>,
        item_type: Option<String>,
    },
    TurnCompleted {
        status: Option<String>,
    },
    Other {
        method: String,
        params: Option<Value>,
    },
}

impl CodexAppServerSession {
    pub fn from_handles(handles: ConnectionHandles) -> Self {
        Self {
            conn: handles.conn,
            notifications: handles.notifications,
        }
    }

    pub async fn initialize(
        &self,
        name: &str,
        title: &str,
        version: &str,
    ) -> Result<Value, ConnError> {
        let result = self
            .conn
            .request(
                "initialize",
                Some(json!({
                    "clientInfo": {
                        "name": name,
                        "title": title,
                        "version": version,
                    }
                })),
            )
            .await?;
        self.conn.notify("initialized", Some(json!({}))).await?;
        Ok(result)
    }

    pub async fn start_thread(&self, model: Option<&str>) -> Result<String, ConnError> {
        let mut params = serde_json::Map::new();
        if let Some(model) = model.filter(|s| !s.trim().is_empty()) {
            params.insert("model".into(), Value::String(model.to_string()));
        }
        let result = self
            .conn
            .request("thread/start", Some(Value::Object(params)))
            .await?;
        Ok(result
            .get("thread")
            .and_then(|t| t.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    pub async fn start_turn(&self, thread_id: &str, text: &str) -> Result<Value, ConnError> {
        self.conn
            .request(
                "turn/start",
                Some(json!({
                    "threadId": thread_id,
                    "input": [{ "type": "text", "text": text }],
                })),
            )
            .await
    }

    pub async fn next_event(&mut self) -> Option<CodexAppEvent> {
        self.notifications.recv().await.map(map_codex_notification)
    }
}

fn map_codex_notification(n: Notification) -> CodexAppEvent {
    let params = n.params.clone();
    match n.method.as_str() {
        "turn/started" => CodexAppEvent::TurnStarted {
            turn_id: params
                .as_ref()
                .and_then(|p| p.get("turn"))
                .and_then(|t| t.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        "item/agentMessage/delta" => CodexAppEvent::AgentMessageDelta {
            text: params
                .as_ref()
                .and_then(|p| p.get("delta").or_else(|| p.get("text")))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "item/started" => CodexAppEvent::ItemStarted {
            item_id: item_field(params.as_ref(), "id"),
            item_type: item_field(params.as_ref(), "type"),
        },
        "item/completed" => CodexAppEvent::ItemCompleted {
            item_id: item_field(params.as_ref(), "id"),
            item_type: item_field(params.as_ref(), "type"),
        },
        "turn/completed" => CodexAppEvent::TurnCompleted {
            status: params
                .as_ref()
                .and_then(|p| p.get("status"))
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        _ => CodexAppEvent::Other {
            method: n.method,
            params,
        },
    }
}

fn item_field(params: Option<&Value>, field: &str) -> Option<String> {
    params
        .and_then(|p| p.get("item"))
        .and_then(|i| i.get(field))
        .or_else(|| params.and_then(|p| p.get(field)))
        .and_then(Value::as_str)
        .map(str::to_string)
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
        let back =
            Message::from_value(serde_json::from_slice(&line[..line.len() - 1]).unwrap()).unwrap();
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
    fn accepts_codex_app_server_jsonrpc_omitted_frames() {
        let req = Message::from_value(json!({
            "id": 10,
            "method": "thread/start",
            "params": { "model": "gpt-5.4" }
        }))
        .unwrap();
        match req {
            Message::Request(r) => {
                assert_eq!(r.jsonrpc, JSONRPC_VERSION);
                assert_eq!(r.method, "thread/start");
            }
            other => panic!("expected request, got {other:?}"),
        }

        let note = Message::from_value(json!({
            "method": "turn/started",
            "params": { "turn": { "id": "turn_1" } }
        }))
        .unwrap();
        assert!(matches!(note, Message::Notification(_)));

        let ok = Message::from_value(json!({
            "id": 10,
            "result": { "thread": { "id": "thr_1" } }
        }))
        .unwrap();
        assert!(matches!(ok, Message::Response(_)));
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
        assert_eq!(
            got.len(),
            1,
            "blank + whitespace lines skipped, CRLF trimmed"
        );
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

#[cfg(test)]
mod conn_tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{split, AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Wire a Connection to an in-memory peer (tokio duplex). Returns the conn
    /// handles + the peer's (read, write) halves so a test can play the server.
    fn pair() -> (
        ConnectionHandles,
        BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
    ) {
        let (client, server) = tokio::io::duplex(4096);
        let (cr, cw) = split(client);
        let (sr, sw) = split(server);
        (Connection::spawn(cr, cw), BufReader::new(sr), sw)
    }

    #[tokio::test]
    async fn request_correlates_with_response() {
        let (h, mut peer_r, mut peer_w) = pair();
        // Peer: read one request line, echo back a Response with the same id.
        let server = tokio::spawn(async move {
            let mut line = String::new();
            peer_r.read_line(&mut line).await.unwrap();
            let req: Request = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(req.method, "ping");
            let resp = Response {
                jsonrpc: JSONRPC_VERSION.into(),
                id: req.id.clone(),
                result: Some(json!({"pong": true})),
                error: None,
            };
            peer_w
                .write_all(&Message::Response(resp).encode_line())
                .await
                .unwrap();
        });
        let got = h.conn.request("ping", Some(json!({"n": 1}))).await.unwrap();
        assert_eq!(got, json!({"pong": true}));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_surfaces_rpc_error() {
        let (h, mut peer_r, mut peer_w) = pair();
        let server = tokio::spawn(async move {
            let mut line = String::new();
            peer_r.read_line(&mut line).await.unwrap();
            let req: Request = serde_json::from_str(line.trim()).unwrap();
            let resp = Response {
                jsonrpc: JSONRPC_VERSION.into(),
                id: req.id,
                result: None,
                error: Some(RpcError {
                    code: -32601,
                    message: "no method".into(),
                    data: None,
                }),
            };
            peer_w
                .write_all(&Message::Response(resp).encode_line())
                .await
                .unwrap();
        });
        let err = h.conn.request("nope", None).await.unwrap_err();
        assert!(matches!(err, ConnError::Rpc(e) if e.code == -32601));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn delivers_notifications_and_peer_requests() {
        let (mut h, _peer_r, mut peer_w) = pair();
        // Peer pushes a notification then a server→client request.
        peer_w
            .write_all(
                &Message::Notification(Notification::new("update", Some(json!({"x": 1}))))
                    .encode_line(),
            )
            .await
            .unwrap();
        peer_w
            .write_all(&Message::Request(Request::new(7, "permission/request", None)).encode_line())
            .await
            .unwrap();
        let n = h
            .notifications
            .recv()
            .await
            .expect("notification delivered");
        assert_eq!(n.method, "update");
        let req = h
            .incoming_requests
            .recv()
            .await
            .expect("peer request delivered");
        assert_eq!(req.method, "permission/request");
        assert_eq!(req.id, json!(7));
    }

    #[tokio::test]
    async fn request_errors_when_peer_closes() {
        let (h, peer_r, peer_w) = pair();
        // Drop the peer immediately → reader hits EOF → pending fails Closed.
        drop(peer_r);
        drop(peer_w);
        let err = h.conn.request("ping", None).await.unwrap_err();
        assert!(matches!(err, ConnError::Closed));
    }

    #[tokio::test]
    async fn codex_session_initializes_starts_thread_turn_and_maps_events() {
        let (h, mut peer_r, mut peer_w) = pair();
        let server = tokio::spawn(async move {
            let mut line = String::new();
            peer_r.read_line(&mut line).await.unwrap();
            let init: Request = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(init.method, "initialize");
            peer_w
                .write_all(
                    &Message::Response(Response {
                        jsonrpc: JSONRPC_VERSION.into(),
                        id: init.id,
                        result: Some(json!({"platformFamily": "mac"})),
                        error: None,
                    })
                    .encode_line(),
                )
                .await
                .unwrap();

            line.clear();
            peer_r.read_line(&mut line).await.unwrap();
            let initialized: Notification = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(initialized.method, "initialized");

            line.clear();
            peer_r.read_line(&mut line).await.unwrap();
            let thread_start: Request = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(thread_start.method, "thread/start");
            assert_eq!(
                thread_start.params.as_ref().unwrap().get("model").unwrap(),
                "gpt-5.4"
            );
            peer_w
                .write_all(
                    &Message::Response(Response {
                        jsonrpc: JSONRPC_VERSION.into(),
                        id: thread_start.id,
                        result: Some(json!({"thread": {"id": "thr_1"}})),
                        error: None,
                    })
                    .encode_line(),
                )
                .await
                .unwrap();

            line.clear();
            peer_r.read_line(&mut line).await.unwrap();
            let turn_start: Request = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(turn_start.method, "turn/start");
            assert_eq!(
                turn_start.params.as_ref().unwrap().get("threadId").unwrap(),
                "thr_1"
            );
            peer_w
                .write_all(
                    &Message::Notification(Notification::new(
                        "item/agentMessage/delta",
                        Some(json!({"delta": "hello"})),
                    ))
                    .encode_line(),
                )
                .await
                .unwrap();
            peer_w
                .write_all(
                    &Message::Response(Response {
                        jsonrpc: JSONRPC_VERSION.into(),
                        id: turn_start.id,
                        result: Some(json!({"turn": {"id": "turn_1"}})),
                        error: None,
                    })
                    .encode_line(),
                )
                .await
                .unwrap();
        });

        let mut session = CodexAppServerSession::from_handles(h);
        let init = session
            .initialize("flockmux", "flockmux", "0.1.0")
            .await
            .unwrap();
        assert_eq!(init.get("platformFamily").unwrap(), "mac");
        let thread_id = session.start_thread(Some("gpt-5.4")).await.unwrap();
        assert_eq!(thread_id, "thr_1");
        let turn = session.start_turn(&thread_id, "say hello").await.unwrap();
        assert_eq!(turn.get("turn").unwrap().get("id").unwrap(), "turn_1");
        let ev = session.next_event().await.unwrap();
        assert_eq!(
            ev,
            CodexAppEvent::AgentMessageDelta {
                text: "hello".into()
            }
        );
        server.await.unwrap();
    }
}
