//! Minimal Chrome DevTools Protocol client.
//!
//! Two transports only, both hand-rolled on purpose (no external CDP crates):
//! - DevTools HTTP discovery (`/json/version`, `/json`) over a raw `TcpStream`
//!   with a manually written HTTP/1.1 request;
//! - the command channel over a tokio-tungstenite websocket.
//!
//! ## Sessions, events and the reader pump
//!
//! The client talks to *either* a page-level socket (`attach`) or the
//! browser-level socket (`attach` on the `/json/version` url). On the browser
//! socket every command that targets a tab carries the flat-mode `sessionId`
//! field ([`CdpClient::send_with_session`]).
//!
//! A `send`-and-wait client cannot also consume events: on one websocket a
//! single reader must own the read half, or two callers would race for frames.
//! So the socket is split and a background *pump* task owns both halves; it
//! routes each inbound frame:
//! - a frame with `id` resolves the matching in-flight command (which `send`
//!   awaits, exactly as before);
//! - a frame without `id` is an event and is pushed into the event channel
//!   handed out by [`CdpClient::events`].
//!
//! `send` therefore keeps its old contract ("wait for the response to *this*
//! id") while events, instead of being silently skipped, go to a subscriber.

use anyhow::{anyhow, Context, Result};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// How long a single CDP command may wait for its matching response.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a single DevTools HTTP request may take.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Outbound websocket message queue depth is unbounded: `send` must never
/// block on a slow reader of the event channel.
type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// A live CDP session over one DevTools websocket endpoint.
///
/// Construct with [`CdpClient::attach`]. Commands go through [`CdpClient::send`]
/// (session-less) or [`CdpClient::send_with_session`] (flat-mode `sessionId`);
/// events come out of [`CdpClient::events`].
pub struct CdpClient {
    /// Outbound frames for the pump task (the pump owns the websocket sink).
    outbound: mpsc::UnboundedSender<Message>,
    /// In-flight commands keyed by wire `id`, resolved by the pump.
    pending: Pending,
    next_id: AtomicU64,
    /// Default `sessionId` used by [`CdpClient::send`]. `None` for a
    /// page-level socket (commands go to the socket's own target) or for
    /// browser-level commands.
    session_id: Option<String>,
    /// Event subscriber end, handed out once by [`CdpClient::events`].
    event_rx: Option<mpsc::UnboundedReceiver<Value>>,
    /// Pump task handle; aborted on drop.
    pump: tokio::task::JoinHandle<()>,
}

/// A `type == "page"` entry of `GET /json`.
#[derive(Debug, Clone)]
pub struct PageTarget {
    /// DevTools target id (the `id` field of the `/json` entry).
    pub target_id: String,
    /// Page-level websocket debugger URL.
    pub ws_url: String,
    /// URL currently loaded in the target.
    pub url: String,
    /// Target title.
    pub title: String,
}

impl CdpClient {
    /// Open a CDP session over an existing DevTools websocket URL.
    ///
    /// The URL may be page-level
    /// (`ws://127.0.0.1:<port>/devtools/page/<targetId>`) or browser-level
    /// (`ws://127.0.0.1:<port>/devtools/browser/<uuid>`, from
    /// [`get_ws_url`]). Commands sent with [`CdpClient::send`] carry no
    /// `sessionId`; browser-level sessions drive tabs through
    /// [`CdpClient::send_with_session`].
    pub async fn attach(ws_url: &str) -> Result<Self> {
        let (ws, _response) = tokio_tungstenite::connect_async(ws_url)
            .await
            .with_context(|| format!("failed to connect to CDP endpoint {ws_url}"))?;
        Ok(Self::from_stream(ws, None))
    }

    /// Build a client around an already-connected websocket, spawning the
    /// reader pump task.
    fn from_stream(
        ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
        session_id: Option<String>,
    ) -> Self {
        let (sink, read) = ws.split();
        let (outbound, outbound_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let pending: Pending = Arc::new(StdMutex::new(HashMap::new()));
        let pump = tokio::spawn(run_pump(sink, read, outbound_rx, pending.clone(), event_tx));
        Self {
            outbound,
            pending,
            next_id: AtomicU64::new(1),
            session_id,
            event_rx: Some(event_rx),
            pump,
        }
    }

    /// Subscribe to CDP events. Call once per client (the receiver is a single
    /// stream); a second call returns a fresh channel that will stay empty.
    ///
    /// The channel is unbounded so a slow subscriber can never stall the pump
    /// (and, with it, command responses).
    pub fn events(&mut self) -> mpsc::UnboundedReceiver<Value> {
        match self.event_rx.take() {
            Some(rx) => rx,
            None => {
                let (_tx, rx) = mpsc::unbounded_channel();
                rx
            }
        }
    }

    /// Send `method` with `params` on the client's default session and wait for
    /// the matching response.
    ///
    /// Takes `&self`: the id counter is atomic and the pending map is behind its
    /// own lock, so commands may be issued concurrently from several tasks on a
    /// shared client. Serializing every command behind one outer mutex let a
    /// long page `Runtime.evaluate` (e.g. one that awaits a worker) starve the
    /// auto-attach task that must unpause that very worker.
    pub async fn send(&self, method: &str, params: Value) -> Result<Value> {
        let session = self.session_id.clone();
        self.send_with_session(session.as_deref(), method, params)
            .await
    }

    /// Send `method` with `params` under an explicit flat-mode `sessionId`
    /// (or `None` for a browser-level / page-level command) and wait for the
    /// matching response.
    pub async fn send_with_session(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        self.send_with_session_timeout(session_id, method, params, COMMAND_TIMEOUT)
            .await
    }

    /// [`CdpClient::send_with_session`] with an explicit per-command timeout.
    ///
    /// Used for worker-scope targets, where a single paused instance can stop
    /// answering Runtime commands: a bounded wait keeps one stuck worker from
    /// stalling the serialized auto-attach loop (and every target behind it).
    /// A timed-out command drops its pending entry so the late response, if it
    /// ever arrives, is simply ignored.
    pub async fn send_with_session_timeout(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let wire = wire_message_with_session(id, session_id, method, params)?;
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        self.outbound
            .send(Message::Text(wire))
            .map_err(|_| anyhow!("CDP connection is closed; cannot send {method} (id {id})"))?;

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_dropped)) => Err(anyhow!(
                "CDP connection closed before response to {method} (id {id})"
            )),
            Err(_elapsed) => {
                lock(&self.pending).remove(&id);
                Err(anyhow!(
                    "CDP timeout: no response to {method} (id {id}) within {timeout:?}"
                ))
            }
        }
    }
}

impl Drop for CdpClient {
    /// Stop the pump task. Dropping the outbound sender would end it too, but
    /// aborting is immediate and keeps a half-dead socket from lingering.
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// Lock a `std::sync::Mutex`, recovering from poisoning (a panicked command
/// task must not wedge every later command).
fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The websocket owner: writes queued frames, reads inbound frames and routes
/// them to the matching command or to the event channel.
///
/// One task owns both halves, so there is never a read race between commands
/// and event consumption. `select!` is not `biased`: writes and reads are both
/// serviced as they become ready.
async fn run_pump(
    mut sink: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    mut read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    mut outbound: mpsc::UnboundedReceiver<Message>,
    pending: Pending,
    event_tx: mpsc::UnboundedSender<Value>,
) {
    loop {
        tokio::select! {
            outbound_msg = outbound.recv() => match outbound_msg {
                Some(message) => {
                    if sink.send(message).await.is_err() {
                        break;
                    }
                }
                // All senders dropped: the client is gone.
                None => break,
            },
            frame = read.next() => match frame {
                Some(Ok(Message::Text(text))) => route_text(&text, &pending, &event_tx),
                // Reply immediately: the pump owns the sink, and tungstenite's
                // auto-queued pong may not be flushed otherwise.
                Some(Ok(Message::Ping(payload))) => {
                    if sink.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Pong(_) | Message::Binary(_) | Message::Frame(_))) => {}
                Some(Err(_)) | None => break,
            },
        }
    }
    // Resolve every in-flight command so its waiter fails fast instead of
    // waiting out COMMAND_TIMEOUT.
    for (_, tx) in lock(&pending).drain() {
        let _ = tx.send(Err(anyhow!("CDP connection closed")));
    }
}

/// Route one inbound text frame: a command response by its `id`, otherwise an
/// event pushed to the subscriber.
fn route_text(text: &str, pending: &Pending, event_tx: &mpsc::UnboundedSender<Value>) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        // Malformed frame: nothing to route; the command (if any) will time out.
        return;
    };
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        if let Some(tx) = lock(pending).remove(&id) {
            let _ = tx.send(response_result(&value, id));
        }
        return;
    }
    // No `id` => a protocol event. Ignore a closed subscriber.
    let _ = event_tx.send(value);
}

/// Turn a raw response frame into the command's `Ok(result)` or `Err(error)`.
fn response_result(value: &Value, id: u64) -> Result<Value> {
    if let Some(error) = value.get("error") {
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("CDP command");
        return Err(anyhow!("CDP error on {method} (id {id}): {error}"));
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

/// Build the exact JSON wire text for one CDP command without a session.
///
/// Object keys come out in serde_json's canonical (alphabetical) order —
/// `{"id":..,"method":..,"params":{..}}` — which coincides with the contract's
/// field order. CDP itself is key-order-insensitive.
pub fn wire_message(id: u64, method: &str, params: Value) -> Result<String> {
    wire_message_with_session(id, None, method, params)
}

/// Build the exact wire text for one CDP command, adding the flat-mode
/// `sessionId` field only when a session is given.
///
/// Alphabetical order places `sessionId` last:
/// `{"id":..,"method":..,"params":{..},"sessionId":".."}`. With
/// `session_id == None` the output is byte-identical to [`wire_message`].
pub fn wire_message_with_session(
    id: u64,
    session_id: Option<&str>,
    method: &str,
    params: Value,
) -> Result<String> {
    let mut object = Map::new();
    object.insert(String::from("id"), json!(id));
    object.insert(String::from("method"), json!(method));
    object.insert(String::from("params"), params);
    if let Some(session_id) = session_id {
        object.insert(String::from("sessionId"), json!(session_id));
    }
    serde_json::to_string(&Value::Object(object)).context("failed to serialize CDP wire message")
}

/// `GET /json/version` → `webSocketDebuggerUrl` (browser-level endpoint).
///
/// Used as a readiness probe after spawn; the returned URL points at the
/// browser target, not at a page.
pub async fn get_ws_url(debug_port: u16) -> Result<String> {
    let version = http_get_json(debug_port, "/json/version").await?;
    version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            anyhow!("webSocketDebuggerUrl missing from /json/version on port {debug_port}")
        })
}

/// `GET /json` → the first target with `type == "page"`.
///
/// Page targets without a usable `webSocketDebuggerUrl` (not attachable yet)
/// are skipped rather than rejected: right after startup the list can churn.
pub async fn get_page_target(debug_port: u16) -> Result<PageTarget> {
    let targets = http_get_json(debug_port, "/json").await?;
    let targets = targets
        .as_array()
        .ok_or_else(|| anyhow!("/json on port {debug_port} returned a non-array payload"))?;
    for target in targets {
        if target.get("type").and_then(Value::as_str) != Some("page") {
            continue;
        }
        let Some(ws_url) = target
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
        else {
            continue;
        };
        return Ok(PageTarget {
            target_id: target
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            ws_url: ws_url.to_owned(),
            url: target
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            title: target
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    Err(anyhow!("no attachable page target on port {debug_port}"))
}

/// Minimal manual HTTP/1.1 GET of a JSON document from the DevTools endpoint.
///
/// Implemented over a raw `TcpStream` on purpose — the DevTools server is
/// always plain localhost HTTP, so reqwest/hyper would be dead weight.
async fn http_get_json(debug_port: u16, path: &str) -> Result<Value> {
    let request = async {
        let addr = SocketAddr::from(([127, 0, 0, 1], debug_port));
        let mut stream = TcpStream::connect(addr).await.with_context(|| {
            format!("failed to connect to DevTools endpoint 127.0.0.1:{debug_port}")
        })?;

        // The Host header must be localhost or an IP literal: Chrome 111+
        // rejects anything else as a DNS-rebinding defense.
        let request = format!(
            "GET {path} HTTP/1.1\r\n\
             Host: 127.0.0.1:{debug_port}\r\n\
             Accept: application/json\r\n\
             Connection: close\r\n\
             \r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .context("failed to write DevTools HTTP request")?;

        let mut raw = Vec::with_capacity(16 * 1024);
        let mut chunk = [0u8; 8192];
        loop {
            let read = stream
                .read(&mut chunk)
                .await
                .context("failed to read DevTools HTTP response")?;
            if read == 0 {
                break; // server honored Connection: close
            }
            raw.extend_from_slice(&chunk[..read]);
            if response_complete(&raw) {
                break;
            }
        }
        parse_http_json(&raw, debug_port, path)
    };
    tokio::time::timeout(HTTP_TIMEOUT, request)
        .await
        .map_err(|_| anyhow!("timeout fetching http://127.0.0.1:{debug_port}{path}"))?
}

/// Decide whether `raw` already holds a complete HTTP response.
fn response_complete(raw: &[u8]) -> bool {
    let Some(headers_end) = find_headers_end(raw) else {
        return false;
    };
    let Ok(headers) = std::str::from_utf8(&raw[..headers_end]) else {
        return false;
    };
    if is_chunked(headers) {
        // Walk the actual chunk framing: chunk data that coincidentally ends
        // in `0\r\n\r\n` must not count as termination.
        return chunked_complete(&raw[headers_end + 4..]);
    }
    match content_length(headers) {
        Some(len) => raw.len() >= headers_end + 4 + len,
        // No framing information available: wait for EOF (Connection: close).
        None => false,
    }
}

/// Split a raw HTTP response into status, headers and JSON body.
fn parse_http_json(raw: &[u8], debug_port: u16, path: &str) -> Result<Value> {
    let headers_end = find_headers_end(raw).ok_or_else(|| {
        anyhow!("malformed HTTP response from 127.0.0.1:{debug_port}{path}: header block not terminated")
    })?;
    let headers = std::str::from_utf8(&raw[..headers_end])
        .map_err(|_| anyhow!("non-UTF8 HTTP headers from 127.0.0.1:{debug_port}{path}"))?;
    let status = status_code(headers)
        .ok_or_else(|| anyhow!("malformed HTTP status line from 127.0.0.1:{debug_port}{path}"))?;
    let body = &raw[headers_end + 4..];
    let body = if is_chunked(headers) {
        decode_chunked(body)?
    } else {
        body.to_vec()
    };
    if status != 200 {
        return Err(anyhow!(
            "HTTP {status} from 127.0.0.1:{debug_port}{path}: {}",
            String::from_utf8_lossy(&body)
        ));
    }
    serde_json::from_slice(&body)
        .with_context(|| format!("invalid JSON from http://127.0.0.1:{debug_port}{path}"))
}

/// Byte offset of the `\r\n\r\n` header terminator, if present yet.
fn find_headers_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Case-insensitive header lookup.
fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    })
}

fn content_length(headers: &str) -> Option<usize> {
    header_value(headers, "content-length").and_then(|value| value.parse().ok())
}

fn is_chunked(headers: &str) -> bool {
    header_value(headers, "transfer-encoding")
        .map(|value| value.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
}

fn status_code(headers: &str) -> Option<u16> {
    headers
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Decode a chunked transfer body (insurance: the DevTools server normally
/// sends Content-Length, but the parser must not choke on chunked framing).
fn decode_chunked(mut body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| anyhow!("malformed chunked body: missing chunk header"))?;
        let size_text = std::str::from_utf8(&body[..line_end])
            .map_err(|_| anyhow!("malformed chunked body: non-UTF8 chunk size"))?;
        // Strip optional chunk extensions ("1a;name=value").
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| anyhow!("malformed chunked body: invalid chunk size {size_text:?}"))?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        // Subtraction form: `size + 2` could overflow on an adversarial
        // huge size value (e.g. "ffffffffffffffff") and panic in debug.
        if body.len() < size || body.len() - size < 2 {
            return Err(anyhow!("malformed chunked body: truncated chunk"));
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..]; // skip chunk data + trailing CRLF
    }
}

/// Walk the chunked framing of `body`: `true` only once the terminal size-0
/// chunk has fully arrived (headers are assumed already stripped).
fn chunked_complete(body: &[u8]) -> bool {
    let mut rest = body;
    loop {
        let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return false; // chunk header still incomplete
        };
        let Ok(size_text) = std::str::from_utf8(&rest[..line_end]) else {
            return false;
        };
        let Ok(size) = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
        else {
            return false; // not a chunk size (yet) — keep reading
        };
        rest = &rest[line_end + 2..];
        if size == 0 {
            return true; // terminal chunk consumed
        }
        if rest.len() < size {
            return false; // chunk data still incomplete
        }
        rest = &rest[size..];
        if rest.len() < 2 {
            return false; // chunk trailing CRLF still incomplete
        }
        rest = &rest[2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_message_is_exact_json_without_params() {
        let wire = wire_message(1, "Page.enable", json!({})).expect("serialize");
        assert_eq!(wire, r#"{"id":1,"method":"Page.enable","params":{}}"#);
    }

    #[test]
    fn wire_message_is_exact_json_with_params() {
        let wire = wire_message(
            42,
            "Emulation.setTimezoneOverride",
            json!({ "timezoneId": "Europe/London" }),
        )
        .expect("serialize");
        assert_eq!(
            wire,
            r#"{"id":42,"method":"Emulation.setTimezoneOverride","params":{"timezoneId":"Europe/London"}}"#
        );
    }

    #[test]
    fn wire_message_without_session_matches_legacy_wire() {
        // A None session must not add a `sessionId` key: the bytes must equal
        // the session-less builder exactly.
        let wire = wire_message_with_session(7, None, "Page.enable", json!({})).expect("serialize");
        assert_eq!(
            wire,
            wire_message(7, "Page.enable", json!({})).expect("serialize")
        );
    }

    #[test]
    fn wire_message_with_session_appends_session_id_last() {
        let wire = wire_message_with_session(
            9,
            Some("ABCDEF1234567890"),
            "Emulation.setLocaleOverride",
            json!({ "locale": "en-US" }),
        )
        .expect("serialize");
        assert_eq!(
            wire,
            r#"{"id":9,"method":"Emulation.setLocaleOverride","params":{"locale":"en-US"},"sessionId":"ABCDEF1234567890"}"#
        );
    }

    #[test]
    fn response_result_maps_result_and_error() {
        let ok: Value = serde_json::from_str(r#"{"id":1,"result":{"value":2}}"#).expect("json");
        assert_eq!(response_result(&ok, 1).expect("ok"), json!({ "value": 2 }));

        let no_result: Value = serde_json::from_str(r#"{"id":1}"#).expect("json");
        assert_eq!(response_result(&no_result, 1).expect("ok"), Value::Null);

        let err: Value =
            serde_json::from_str(r#"{"id":1,"error":{"code":-32601,"message":"nope"}}"#)
                .expect("json");
        let error = response_result(&err, 1).expect_err("must be an error");
        assert!(error.to_string().contains("nope"), "unexpected: {error}");
    }

    #[test]
    fn finds_header_terminator() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(find_headers_end(raw), Some(raw.len() - 4));
    }

    #[test]
    fn no_header_terminator_in_partial_data() {
        assert_eq!(find_headers_end(b"HTTP/1.1 200 O"), None);
        assert_eq!(find_headers_end(b""), None);
    }

    #[test]
    fn parses_status_and_headers() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 42\r\n";
        assert_eq!(status_code(headers), Some(200));
        assert_eq!(content_length(headers), Some(42));
        assert!(!is_chunked(headers));

        let chunked = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n";
        assert!(is_chunked(chunked));
        assert_eq!(content_length(chunked), None);
    }

    #[test]
    fn decodes_chunked_bodies() {
        let decoded = decode_chunked(b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n").expect("decode");
        assert_eq!(decoded, b"Wikipedia");
        assert!(decode_chunked(b"zz\r\n").is_err());
        assert!(decode_chunked(b"5\r\nabc").is_err());
    }

    #[test]
    fn decode_chunked_rejects_huge_sizes_without_overflow() {
        // u64::MAX as a chunk size must error, not overflow in `size + 2`.
        assert!(decode_chunked(b"ffffffffffffffff\r\nxx").is_err());
        // Adversarial-but-valid framing: 8-byte chunk whose data itself ends
        // in `0\r\n\r\n`, followed by the real terminal chunk.
        let decoded = decode_chunked(b"8\r\nabc0\r\n\r\n\r\n0\r\n\r\n").expect("decode");
        assert_eq!(decoded, b"abc0\r\n\r\n");
    }

    #[test]
    fn chunked_completeness_is_not_fooled_by_chunk_data() {
        // Chunk data that coincidentally ends in `0\r\n\r\n` must not count
        // as the terminal chunk (a plain suffix check would finish early).
        assert!(!chunked_complete(b"8\r\nabc0\r\n\r\n"));
        assert!(chunked_complete(b"8\r\nabc0\r\n\r\n\r\n0\r\n\r\n"));
        assert!(!chunked_complete(b"4\r\nWiki"));
        assert!(chunked_complete(b"4\r\nWiki\r\n0\r\n\r\n"));
        assert!(!chunked_complete(b""));
    }

    #[test]
    fn response_complete_handles_chunked_framing() {
        let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut raw = head.to_vec();
        assert!(!response_complete(&raw));
        // Chunk data ending in `0\r\n\r\n` must not complete the response.
        raw.extend_from_slice(b"8\r\nabc0\r\n\r\n");
        assert!(!response_complete(&raw));
        // The data CRLF plus the real terminal zero-size chunk does.
        raw.extend_from_slice(b"\r\n0\r\n\r\n");
        assert!(response_complete(&raw));
    }

    #[test]
    fn response_complete_respects_content_length() {
        let mut raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n".to_vec();
        assert!(!response_complete(&raw));
        raw.extend_from_slice(b"{\"a\"");
        assert!(!response_complete(&raw));
        raw.extend_from_slice(b":1}");
        assert!(response_complete(&raw));
    }
}
