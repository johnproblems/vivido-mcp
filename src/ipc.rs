//! Minimal Vivido automation IPC client.
//!
//! A small client of our own, deliberately not reusing the `vivido` crate's `host`
//! module: that opens a new connection per call and pulls in the whole renderer.
//! This client holds one long-lived connection per [`Connection`], speaks the
//! version-2 newline-delimited JSON protocol (`hello` first), and makes strictly
//! sequential requests — the MCP loop issues one tool call at a time, so the
//! server's 64 in-flight slots are never needed.
//!
//! Request frames are capped at 1 MiB and reply/event frames at 16 MiB, matching
//! the server. An unknown `protocol_version` in the `hello` reply is refused with
//! a clear message rather than limped along with.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Automation protocol version this client speaks.
pub const PROTOCOL_VERSION: u16 = 2;
/// Maximum request frame the server accepts.
pub const MAX_REQUEST_FRAME_BYTES: usize = 1024 * 1024;
/// Maximum reply or event frame the server sends.
pub const MAX_REPLY_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound for one frame read, so a hostile peer cannot grow memory unboundedly.
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// One stable Vivido IPC error.
#[derive(Clone, Debug)]
pub struct IpcError {
    /// Stable machine-readable code (`window_not_found`, `timeout`, ...).
    pub code: String,
    /// Human-readable diagnostic.
    pub message: String,
    /// Optional structured recovery detail.
    pub data: Option<Value>,
}

/// Failure of an IPC operation.
#[derive(Clone, Debug)]
pub enum Failure {
    /// The transport broke (connect, read, write, timeout).
    Transport(String),
    /// The peer violated the protocol (bad framing, wrong version, desync).
    Protocol(String),
    /// The server answered with a structured error.
    Server(IpcError),
}

impl Failure {
    /// Human-readable message for MCP text content. Server recovery detail
    /// (`data`) rides along so mismatches stay diagnosable.
    pub fn message(&self) -> String {
        match self {
            Failure::Transport(message) | Failure::Protocol(message) => message.clone(),
            Failure::Server(error) => {
                let mut message = match &error.data {
                    Some(data) => format!("{}: {} [{}]", error.code, error.message, data),
                    None => format!("{}: {}", error.code, error.message),
                };
                if let Some(hint) = hint_for(&error.code) {
                    message.push_str(&format!(" ({hint})"));
                }
                message
            }
        }
    }
}

/// Recovery hint for a common server error code.
pub fn hint_for(code: &str) -> Option<&'static str> {
    match code {
        "window_not_found" => Some("call list_windows to rediscover"),
        "no_focused_window" => Some("pass window_id explicitly"),
        "timeout" => Some("inspect before retrying"),
        "sequence_gap" => Some("history was evicted; omit the offset to read newest"),
        "focus_denied" => Some("the window system refused activation"),
        "resize_mismatch" => Some("the requested size was not reached"),
        "subscription_overflow" => Some("poll drain_events more often or narrow events"),
        _ => None,
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// One advertised method capability from the `hello` handshake.
#[derive(Clone, Debug)]
pub struct MethodCapability {
    /// Method name as `hello.methods` spells it.
    pub name: String,
    /// Effect class: `observe`, `input`, `window`, `config`, `process`,
    /// `lifecycle`, or `extension`.
    pub class: String,
    /// Whether the method changes host state.
    pub mutating: bool,
    /// Whether an embedding host (Vivida) owns dispatch for this method.
    pub host_claimed: bool,
}

/// Parsed `hello` handshake: the single authority for tool generation.
#[derive(Clone, Debug)]
pub struct HelloInfo {
    /// Vivido/Vivida server version.
    pub server_version: String,
    /// Whether the instance is headless.
    pub headless: bool,
    /// Headless session name, if any.
    pub session: Option<String>,
    /// Name `--target` addresses this instance by, if any.
    pub automation_name: Option<String>,
    /// Advertised method names, Vivido's own plus host-claimed.
    pub methods: Vec<String>,
    /// Per-method class/mutating/host flags.
    pub capabilities: Vec<MethodCapability>,
    /// Subscribable event kinds; also the `subscribe --events` allowlist.
    pub event_kinds: Vec<String>,
    /// Server limits (`request_frame_bytes`, `subscriptions`, ...).
    pub limits: Value,
}

impl HelloInfo {
    /// Capability for `method`, if the handshake classified it.
    pub fn capability(&self, method: &str) -> Option<&MethodCapability> {
        self.capabilities.iter().find(|capability| capability.name == method)
    }
}

/// One frame read from the connection.
#[derive(Clone, Debug)]
pub enum Frame {
    /// Correlated request reply.
    Response {
        /// Request id this answers.
        id: u64,
        /// Whether the request succeeded.
        ok: bool,
        /// Result payload on success.
        result: Option<Value>,
        /// Structured error on failure.
        error: Option<IpcError>,
    },
    /// Subscription event, distinguished by `subscription_id`.
    Event {
        /// Subscription this event belongs to.
        subscription_id: u64,
        /// Global monotonic event sequence.
        event_sequence: u64,
        /// Window the event concerns, if any.
        window_id: Option<u64>,
        /// `{"type": ..., "data": ...}` payload.
        event: Value,
    },
}

/// Platform transport: a Unix socket on Unix, a named pipe on Windows.
#[cfg(unix)]
mod transport {
    use std::io::{self, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    pub struct Transport {
        stream: UnixStream,
    }

    impl Transport {
        pub fn connect(path: &std::path::Path) -> io::Result<Self> {
            UnixStream::connect(path).map(|stream| Self { stream })
        }

        pub fn send_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.stream.write_all(bytes)
        }

        pub fn recv_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            use std::io::Read;
            self.stream.read(buf)
        }

        pub fn set_recv_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.stream.set_read_timeout(timeout)
        }
    }
}

/// Platform transport: a Unix socket on Unix, a named pipe on Windows.
#[cfg(windows)]
mod transport {
    use std::io::{self, Read, Write};
    use std::path::Path;
    use std::time::{Duration, Instant};

    use interprocess::local_socket::traits::Stream as _;
    use interprocess::local_socket::{
        GenericNamespaced, RecvHalf, SendHalf, Stream, ToNsName as _,
    };

    pub struct Transport {
        recv: RecvHalf,
        send: SendHalf,
        deadline: Option<Instant>,
    }

    impl Transport {
        pub fn connect(path: &Path) -> io::Result<Self> {
            // Registry and discovery paths spell Windows endpoints as
            // `\\.\pipe\<name>`; the pipe name is the trailing segment.
            let text = path.to_string_lossy();
            let name = text.rsplit('\\').next().unwrap_or(text.as_ref());
            if name.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid pipe path {}", path.display()),
                ));
            }
            let name = name.to_ns_name::<GenericNamespaced>()?;
            let stream = Stream::connect(name)?;
            // Named pipes have no recv timeouts, so reads poll in nonblocking
            // mode against a deadline (see `recv_chunk`).
            stream.set_nonblocking(true)?;
            let (recv, send) = stream.split();
            Ok(Self { recv, send, deadline: None })
        }

        pub fn send_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.send.write_all(bytes)
        }

        pub fn recv_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            loop {
                match self.recv.read(buf) {
                    Ok(read) => return Ok(read),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if self.deadline.is_some_and(|end| Instant::now() >= end) {
                            return Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "timed out waiting for Vivido",
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        pub fn set_recv_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.deadline = timeout.map(|span| Instant::now() + span);
            Ok(())
        }
    }
}

use transport::Transport;

/// Whether an endpoint accepts a connection right now. Used for discovery-time
/// liveness checks; real work opens a [`Connection`].
pub fn probe(path: &Path) -> bool {
    Transport::connect(path).is_ok()
}

/// One long-lived version-2 connection. Requests are strictly sequential.
pub struct Connection {
    transport: Transport,
    next_id: u64,
    buffer: Vec<u8>,
    poisoned: Option<String>,
    /// Stray event frames seen on this connection (it holds no subscription).
    stray_events: u64,
}

impl Connection {
    /// Connect to an already-resolved endpoint path.
    pub fn connect(path: &Path) -> Result<Self, Failure> {
        Transport::connect(path)
            .map_err(|error| {
                Failure::Transport(format!("cannot connect to Vivido endpoint: {error}"))
            })
            .map(|transport| Self {
                transport,
                next_id: 1,
                buffer: Vec::new(),
                poisoned: None,
                stray_events: 0,
            })
    }

    /// Perform the mandatory `hello` handshake and validate the protocol version.
    pub fn hello(&mut self) -> Result<HelloInfo, Failure> {
        let result = self.request("hello", Value::Object(Default::default()))?;
        parse_hello(&result)
    }

    /// Send one request and wait for its correlated response. Event frames are
    /// ignored: this connection holds no subscription, so any would be stray.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        let id = self.send_request(method, params)?;
        loop {
            match self.recv_frame(None)? {
                Frame::Response { id: reply, ok: true, result, .. } if reply == id => {
                    return Ok(result.unwrap_or(Value::Null));
                }
                Frame::Response { id: reply, ok: false, error, .. } if reply == id => {
                    let error = error.unwrap_or(IpcError {
                        code: "invalid_response".to_owned(),
                        message: "server sent a failure without an error body".to_owned(),
                        data: None,
                    });
                    return Err(Failure::Server(error));
                }
                Frame::Response { id: reply, .. } => {
                    return Err(self.poison(format!(
                        "server replied to unknown request id {reply} (expected {id})"
                    )));
                }
                Frame::Event { .. } => {
                    self.stray_events += 1;
                }
            }
        }
    }

    /// Send a request without waiting. The caller owns correlation via [`Self::recv_frame`].
    pub fn send_request(&mut self, method: &str, params: Value) -> Result<u64, Failure> {
        self.check_poisoned()?;
        validate_method_name(method)?;
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Failure::Protocol("IPC request id space exhausted".to_owned()))?;
        let mut frame = serde_json::to_vec(&serde_json::json!({
            "version": PROTOCOL_VERSION,
            "id": id,
            "method": method,
            "params": params,
        }))
        .map_err(|error| Failure::Protocol(format!("cannot encode request: {error}")))?;
        if frame.len() + 1 > MAX_REQUEST_FRAME_BYTES {
            return Err(Failure::Protocol(format!(
                "request frame ({} bytes) exceeds the 1 MiB server limit",
                frame.len() + 1
            )));
        }
        frame.push(b'\n');
        self.transport.send_bytes(&frame).map_err(|error| {
            Failure::Transport(format!("cannot send `{method}` request: {error}"))
        })?;
        Ok(id)
    }

    /// Read one frame, waiting up to `timeout`. `None` blocks indefinitely.
    pub fn recv_frame(&mut self, timeout: Option<Duration>) -> Result<Frame, Failure> {
        self.check_poisoned()?;
        let frame = self.read_line(timeout)?;
        parse_frame(&frame).map_err(|message| self.poison(message))
    }

    /// Number of stray event frames ignored on this connection.
    pub fn stray_events(&self) -> u64 {
        self.stray_events
    }

    fn check_poisoned(&self) -> Result<(), Failure> {
        if let Some(reason) = &self.poisoned {
            return Err(Failure::Transport(format!("connection is unusable: {reason}")));
        }
        Ok(())
    }

    fn poison(&mut self, reason: String) -> Failure {
        self.poisoned = Some(reason.clone());
        Failure::Protocol(reason)
    }

    fn read_line(&mut self, timeout: Option<Duration>) -> Result<Vec<u8>, Failure> {
        if let Some(offset) = self.buffer.iter().position(|byte| *byte == b'\n') {
            return Ok(self.buffer.drain(..=offset).collect());
        }
        self.transport
            .set_recv_timeout(timeout)
            .map_err(|error| Failure::Transport(format!("cannot set read timeout: {error}")))?;
        let deadline = timeout.map(|span| Instant::now() + span);
        let mut chunk = vec![0u8; READ_CHUNK_BYTES];
        loop {
            if let Some(left) = deadline.map(|end| end.saturating_duration_since(Instant::now())) {
                if left.is_zero() {
                    return Err(Failure::Transport("timed out waiting for Vivido".to_owned()));
                }
                self.transport.set_recv_timeout(Some(left)).map_err(|error| {
                    Failure::Transport(format!("cannot set read timeout: {error}"))
                })?;
            }
            match self.transport.recv_chunk(&mut chunk) {
                Ok(0) => {
                    return Err(Failure::Transport("Vivido closed the connection".to_owned()));
                }
                Ok(read) => {
                    if self.buffer.len() + read > MAX_REPLY_FRAME_BYTES {
                        self.poisoned = Some("reply exceeded 16 MiB".to_owned());
                        return Err(Failure::Protocol(
                            "reply frame exceeds the 16 MiB limit; connection desynchronized"
                                .to_owned(),
                        ));
                    }
                    self.buffer.extend_from_slice(&chunk[..read]);
                    if let Some(offset) = self.buffer.iter().position(|byte| *byte == b'\n') {
                        let _ = self.transport.set_recv_timeout(None);
                        return Ok(self.buffer.drain(..=offset).collect());
                    }
                }
                Err(error)
                    if error.kind() == io::ErrorKind::TimedOut
                        || error.kind() == io::ErrorKind::WouldBlock =>
                {
                    return Err(Failure::Transport("timed out waiting for Vivido".to_owned()));
                }
                Err(error) => {
                    return Err(Failure::Transport(format!("cannot read from Vivido: {error}")));
                }
            }
        }
    }
}

fn validate_method_name(method: &str) -> Result<(), Failure> {
    if method.is_empty()
        || method.len() > 128
        || !method.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(Failure::Protocol(
            "IPC method must contain 1-128 ASCII letters, digits, or underscores".to_owned(),
        ));
    }
    Ok(())
}

fn parse_frame(frame: &[u8]) -> Result<Frame, String> {
    let value: Value =
        serde_json::from_slice(frame).map_err(|error| format!("invalid IPC frame: {error}"))?;
    if let Some(subscription) = value.get("subscription_id") {
        return Ok(Frame::Event {
            subscription_id: subscription
                .as_u64()
                .ok_or_else(|| "event frame has a non-numeric subscription_id".to_owned())?,
            event_sequence: value
                .get("event_sequence")
                .and_then(Value::as_u64)
                .ok_or_else(|| "event frame is missing event_sequence".to_owned())?,
            window_id: value.get("window_id").and_then(Value::as_u64),
            event: value.get("event").cloned().unwrap_or(Value::Null),
        });
    }
    let id = value
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "response frame is missing id".to_owned())?;
    let ok = value
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("response frame {id} is missing ok"))?;
    if ok {
        Ok(Frame::Response { id, ok: true, result: value.get("result").cloned(), error: None })
    } else {
        let body = value.get("error").cloned().unwrap_or(Value::Null);
        Ok(Frame::Response {
            id,
            ok: false,
            result: None,
            error: Some(IpcError {
                code: body
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("invalid_response")
                    .to_owned(),
                message: body
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("server sent a failure without a message")
                    .to_owned(),
                data: body.get("data").cloned(),
            }),
        })
    }
}

fn parse_hello(result: &Value) -> Result<HelloInfo, Failure> {
    let protocol_version = result
        .get("protocol_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| Failure::Protocol("hello reply is missing protocol_version".to_owned()))?;
    if protocol_version != u64::from(PROTOCOL_VERSION) {
        return Err(Failure::Protocol(format!(
            "unsupported Vivido automation protocol version {protocol_version} \
             (this vivido-mcp speaks version {PROTOCOL_VERSION}); upgrade vivido-mcp"
        )));
    }
    let methods = result
        .get("methods")
        .and_then(Value::as_array)
        .map(|methods| methods.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let capabilities = result
        .get("method_capabilities")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|entry| {
                    Some(MethodCapability {
                        name: entry.get("name")?.as_str()?.to_owned(),
                        class: entry.get("class")?.as_str()?.to_owned(),
                        mutating: entry.get("mutating")?.as_bool()?,
                        host_claimed: entry.get("host_claimed")?.as_bool().unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let strings = |key: &str| {
        result
            .get(key)
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    };
    Ok(HelloInfo {
        server_version: result
            .get("server_version")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        headless: result.get("headless").and_then(Value::as_bool).unwrap_or(false),
        session: result.get("session").and_then(Value::as_str).map(str::to_owned),
        automation_name: result.get("automation_name").and_then(Value::as_str).map(str::to_owned),
        methods,
        capabilities,
        event_kinds: strings("event_kinds"),
        limits: result.get("limits").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn refuses_unknown_protocol_version() {
        let error = parse_hello(&json!({"protocol_version": 99})).unwrap_err();
        assert!(error.message().contains("99"), "{}", error.message());
        assert!(error.message().contains("2"), "{}", error.message());
    }

    #[test]
    fn parses_hello_capabilities() {
        let hello = parse_hello(&json!({
            "server_version": "0.5.4",
            "protocol_version": 2,
            "headless": true,
            "session": "build",
            "automation_name": "build",
            "methods": ["hello", "key"],
            "method_capabilities": [
                {"name": "hello", "class": "observe", "mutating": false, "host_claimed": false},
                {"name": "key", "class": "input", "mutating": true, "host_claimed": false},
            ],
            "event_kinds": ["screen_changed"],
            "error_codes": ["timeout"],
            "limits": {"connections": 32},
        }))
        .unwrap();
        assert_eq!(hello.server_version, "0.5.4");
        assert!(hello.headless);
        assert_eq!(hello.capability("key").unwrap().class, "input");
    }

    #[test]
    fn distinguishes_events_by_subscription_id() {
        let event = parse_frame(
            br#"{"version":2,"subscription_id":7,"event_sequence":123,"event":{"type":"bell"}}"#,
        )
        .unwrap();
        assert!(matches!(event, Frame::Event { subscription_id: 7, .. }));
        let response = parse_frame(br#"{"version":2,"id":3,"ok":true,"result":{}}"#).unwrap();
        assert!(matches!(response, Frame::Response { id: 3, ok: true, .. }));
    }

    #[test]
    fn server_errors_carry_hints() {
        let failure = Failure::Server(IpcError {
            code: "window_not_found".to_owned(),
            message: "gone".to_owned(),
            data: None,
        });
        assert!(failure.message().contains("list_windows"), "{}", failure.message());
        assert!(hint_for("bogus").is_none());
    }

    #[test]
    fn rejects_bad_method_names() {
        assert!(validate_method_name("key").is_ok());
        assert!(validate_method_name("").is_err());
        assert!(validate_method_name("key; rm").is_err());
        assert!(validate_method_name(&"k".repeat(129)).is_err());
    }
}
