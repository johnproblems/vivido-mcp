#![allow(dead_code)]

// Shared mock Vivido endpoint for integration tests.
// Each test binary compiles this module separately and uses a different
// subset, so unused helpers are expected rather than suspicious.
//

// Speaks just enough of the version-2 protocol to exercise the client:
// hello handshake, correlated request/reply, subscribe with since_event
// replay plus live delivery, and screenshot PNGs with configurable metrics.
// The method table below is an INDEPENDENT copy of the upstream contract —
// sharing the client's own table would make generation tests circular.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

/// (method, class, mutating, host_claimed): upstream contract plus adversarial extras.
pub const MOCK_METHODS: &[(&str, &str, bool, bool)] = &[
    ("hello", "observe", false, false),
    ("ping", "observe", false, false),
    ("reset_terminal", "lifecycle", true, false),
    ("restart_terminal", "lifecycle", true, false),
    ("unsubscribe", "observe", false, false),
    ("create_window", "window", true, false),
    ("close_window", "lifecycle", true, false),
    ("config", "config", true, false),
    ("get_config", "observe", false, false),
    ("typing", "input", true, false),
    ("get_text", "observe", false, false),
    ("find_text", "observe", false, false),
    ("screenshot", "observe", false, false),
    ("key", "input", true, false),
    ("paste", "input", true, false),
    ("drop_file", "input", true, false),
    ("mouse", "input", true, false),
    ("resize", "window", true, false),
    ("set_geometry", "window", true, false),
    ("set_geometry_batch", "window", true, false),
    ("set_visible", "window", true, false),
    ("set_level", "window", true, false),
    ("focus", "process", true, false),
    ("signal", "process", true, false),
    ("exec", "process", true, false),
    ("list_windows", "observe", false, false),
    ("inspect", "observe", false, false),
    ("diagnose", "observe", false, false),
    ("vivid_sessions", "observe", false, false),
    ("vivid_surfaces", "observe", false, false),
    ("vivid_surface_status", "observe", false, false),
    ("vivid_tracks", "observe", false, false),
    ("vivid_track_status", "observe", false, false),
    ("vivid_scene_status", "observe", false, false),
    ("vivid_trace", "observe", false, false),
    ("get_grid", "observe", false, false),
    ("wait_text", "observe", false, false),
    ("wait_output", "observe", false, false),
    ("wait_screen_change", "observe", false, false),
    ("wait_screen_stable", "observe", false, false),
    ("wait_frame", "observe", false, false),
    ("wait_exit", "observe", false, false),
    ("wait_prompt", "observe", false, false),
    ("wait_command_finish", "observe", false, false),
    ("quit", "lifecycle", true, false),
    ("wait_vivid_track", "observe", false, false),
    ("transcript", "observe", false, false),
    ("subscribe", "observe", false, false),
    // Host-claimed Vivida-style methods.
    ("vivida_layout", "observe", false, true),
    ("vivida_activate_pane", "window", true, true),
    // A future method the client has no translator for.
    ("future_method", "extension", true, false),
    // Adversarial: claims `observe` but is mutating. Must never land in the
    // observe tier.
    ("sneaky_observe", "observe", true, false),
];

pub const MOCK_EVENT_KINDS: &[&str] = &[
    "screen_changed",
    "output",
    "frame_presented",
    "title_changed",
    "directory_changed",
    "progress_changed",
    "focus_changed",
    "resized",
    "moved",
    "bell",
    "child_exit",
    "window_created",
    "window_closed",
    "client_fault",
    "client_recovered",
    "overflow",
];

/// Handler result: Ok(result) or Err((code, message)).
pub type Handler = Arc<dyn Fn(&Value) -> Result<Value, (String, String)> + Send + Sync>;

struct Shared {
    recorded: Mutex<Vec<(String, Value)>>,
    handlers: Mutex<HashMap<String, Handler>>,
    events: Mutex<Vec<Value>>,
    protocol_version: Mutex<u64>,
    hello_override: Mutex<Option<Value>>,
    strict_hello_first: bool,
    temp: TempDir,
    screenshot_seq: Mutex<u64>,
}

impl Shared {
    fn hello_result(&self) -> Value {
        if let Some(custom) = self.hello_override.lock().unwrap().clone() {
            return custom;
        }
        let version = *self.protocol_version.lock().unwrap();
        json!({
            "server_version": "9.9.9-mock",
            "protocol_version": version,
            "headless": true,
            "session": "mock",
            "automation_name": "mock",
            "methods": MOCK_METHODS.iter().map(|entry| entry.0).collect::<Vec<_>>(),
            "method_capabilities": MOCK_METHODS.iter().map(|entry| {
                json!({
                    "name": entry.0,
                    "class": entry.1,
                    "mutating": entry.2,
                    "host_claimed": entry.3,
                })
            }).collect::<Vec<_>>(),
            "event_kinds": MOCK_EVENT_KINDS,
            "error_codes": ["window_not_found", "timeout", "sequence_gap"],
            "limits": {
                "request_frame_bytes": 1048576,
                "reply_event_frame_bytes": 16777216,
                "connections": 32,
                "in_flight_requests_per_connection": 64,
                "subscriptions": 32,
            },
        })
    }
}

/// A running mock endpoint. The socket lives in a tempdir removed on drop.
pub struct MockServer {
    shared: Arc<Shared>,
    /// Endpoint path to hand to the client.
    pub socket: PathBuf,
}

impl MockServer {
    /// Start serving with default handlers.
    pub fn start() -> Self {
        Self::start_with(false)
    }

    /// Start serving, optionally enforcing hello-first like the real server.
    pub fn start_with(strict_hello_first: bool) -> Self {
        let temp = TempDir::new().expect("tempdir");
        let socket = temp.path().join("v.sock");
        let listener = UnixListener::bind(&socket).expect("bind mock socket");
        let shared = Arc::new(Shared {
            recorded: Mutex::new(Vec::new()),
            handlers: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
            protocol_version: Mutex::new(2),
            hello_override: Mutex::new(None),
            strict_hello_first,
            temp,
            screenshot_seq: Mutex::new(0),
        });
        let serving = Arc::clone(&shared);
        thread::spawn(move || {
            for stream in listener.incoming().filter_map(Result::ok) {
                let shared = Arc::clone(&serving);
                thread::spawn(move || serve_connection(stream, shared));
            }
        });
        Self { shared, socket }
    }

    /// Register a handler for `method` (overrides the default).
    pub fn on(&self, method: &str, handler: Handler) {
        self.shared.handlers.lock().unwrap().insert(method.to_owned(), handler);
    }

    /// Preload one replayable event.
    pub fn push_event(&self, sequence: u64, window_id: Option<u64>, kind: &str, data: Value) {
        self.shared.events.lock().unwrap().push(json!({
            "sequence": sequence,
            "window_id": window_id,
            "type": kind,
            "data": data,
        }));
    }

    /// Every (method, params) received so far, across connections.
    pub fn recorded(&self) -> Vec<(String, Value)> {
        self.shared.recorded.lock().unwrap().clone()
    }

    /// Override the protocol version in `hello`.
    pub fn set_protocol_version(&self, version: u64) {
        *self.shared.protocol_version.lock().unwrap() = version;
    }

    /// Write a test PNG and return its path. Quadrants are red/green/blue/white
    /// so crops can assert exact pixels.
    pub fn write_png(&self, width: u32, height: u32) -> PathBuf {
        let mut seq = self.shared.screenshot_seq.lock().unwrap();
        *seq += 1;
        let path = self.shared.temp.path().join(format!("shot-{seq}.png"));
        let image = quadrant_image(width, height);
        image.save(&path).expect("save test PNG");
        path
    }

    /// Default screenshot result for `path` with the given metrics.
    pub fn screenshot_result(
        &self,
        path: &PathBuf,
        window_id: u64,
        cell: (u32, u32),
        padding: (u32, u32),
    ) -> Value {
        let (width, height) = image::image_dimensions(path).expect("read test PNG");
        json!({
            "window_id": window_id,
            "frame_sequence": 42,
            "width": width,
            "height": height,
            "scale_factor": 1.0,
            "cell": {"width": cell.0, "height": cell.1},
            "padding": {"x": padding.0, "y": padding.1},
            "path": path.to_string_lossy(),
        })
    }
}

fn serve_connection(stream: UnixStream, shared: Arc<Shared>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone mock stream"));
    let mut writer = stream;
    let mut line = String::new();
    let mut first = true;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(_) => return,
        }
        let frame: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => {
                let _ = write_line(
                    &mut writer,
                    &json!({
                        "version": 2, "id": 0, "ok": false,
                        "error": {"code": "invalid_request", "message": "bad frame"},
                    }),
                );
                return;
            }
        };
        let id = frame.get("id").and_then(Value::as_u64).unwrap_or(0);
        let method = frame.get("method").and_then(Value::as_str).unwrap_or("").to_owned();
        let params = frame.get("params").cloned().unwrap_or(Value::Null);
        shared.recorded.lock().unwrap().push((method.clone(), params.clone()));
        if shared.strict_hello_first && first && method != "hello" {
            let _ = write_line(
                &mut writer,
                &json!({
                    "version": 2, "id": id, "ok": false,
                    "error": {"code": "invalid_request", "message": "first request must be hello"},
                }),
            );
            return;
        }
        first = false;
        match method.as_str() {
            "hello" => {
                let reply =
                    json!({"version": 2, "id": id, "ok": true, "result": shared.hello_result()});
                if write_line(&mut writer, &reply).is_err() {
                    return;
                }
            }
            "subscribe" => {
                serve_subscribe(&mut reader, &mut writer, id, &params, &shared);
                return;
            }
            _ => {
                let reply =
                    if let Some(handler) = shared.handlers.lock().unwrap().get(&method).cloned() {
                        match handler(&params) {
                            Ok(result) => {
                                json!({"version": 2, "id": id, "ok": true, "result": result})
                            }
                            Err((code, message)) => {
                                json!({"version": 2, "id": id, "ok": false,
                                   "error": {"code": code, "message": message}})
                            }
                        }
                    } else {
                        json!({"version": 2, "id": id, "ok": true, "result": {}})
                    };
                if write_line(&mut writer, &reply).is_err() {
                    return;
                }
            }
        }
    }
}

fn serve_subscribe(
    reader: &mut BufReader<UnixStream>,
    writer: &mut UnixStream,
    id: u64,
    params: &Value,
    shared: &Arc<Shared>,
) {
    let window_id = params.get("window_id").and_then(Value::as_u64);
    let all = params.get("all").and_then(Value::as_bool).unwrap_or(false);
    let kinds: Vec<String> = params
        .get("events")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let subscription_id = 7;
    let current = shared
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| event["sequence"].as_u64())
        .max()
        .unwrap_or(0);
    // Like the real server: replay only with since_event, else live from now.
    let mut last_sent = params.get("since_event").and_then(Value::as_u64).unwrap_or(current);
    let ack = json!({
        "version": 2, "id": id, "ok": true,
        "result": {"subscription_id": subscription_id, "event_sequence": current},
    });
    if write_line(writer, &ack).is_err() {
        return;
    }
    // Replay, then live delivery: poll the pool while watching for unsubscribe.
    writer.set_read_timeout(Some(Duration::from_millis(20))).ok();
    let mut line = String::new();
    loop {
        let due: Vec<Value> = shared
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| {
                event["sequence"].as_u64().is_some_and(|sequence| sequence > last_sent)
                    && (kinds.is_empty()
                        || event["type"]
                            .as_str()
                            .is_some_and(|kind| kinds.iter().any(|keep| keep == kind)))
                    && (all || window_id.is_none() || event["window_id"].as_u64() == window_id)
            })
            .cloned()
            .collect();
        for event in due {
            last_sent = event["sequence"].as_u64().unwrap_or(last_sent);
            let frame = json!({
                "version": 2,
                "subscription_id": subscription_id,
                "event_sequence": last_sent,
                "window_id": event["window_id"].clone(),
                "event": {"type": event["type"].clone(), "data": event["data"].clone()},
            });
            if write_line(writer, &frame).is_err() {
                return;
            }
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {
                if line.trim().is_empty() {
                    continue;
                }
                // An unsubscribe (or anything else) ends this subscription.
                if let Ok(frame) = serde_json::from_str::<Value>(line.trim()) {
                    let method =
                        frame.get("method").and_then(Value::as_str).unwrap_or("").to_owned();
                    let params = frame.get("params").cloned().unwrap_or(Value::Null);
                    let rid = frame.get("id").and_then(Value::as_u64).unwrap_or(0);
                    shared.recorded.lock().unwrap().push((method, params));
                    let _ = write_line(
                        writer,
                        &json!({"version": 2, "id": rid, "ok": true, "result": {}}),
                    );
                }
                return;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(_) => return,
        }
    }
}

fn write_line(writer: &mut UnixStream, value: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).expect("encode mock frame");
    line.push('\n');
    writer.write_all(line.as_bytes()).and_then(|()| writer.flush())
}

/// Test image with red/green/blue/white quadrants (top-left origin).
pub fn quadrant_image(width: u32, height: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(width, height, |x, y| {
        let (left, top) = (x < width / 2, y < height / 2);
        match (left, top) {
            (true, true) => image::Rgba([255, 0, 0, 255]),
            (false, true) => image::Rgba([0, 255, 0, 255]),
            (true, false) => image::Rgba([0, 0, 255, 255]),
            (false, false) => image::Rgba([255, 255, 255, 255]),
        }
    })
}

/// Build a [`Server`](vivido_mcp::server::Server) attached to `mock`.
pub fn test_server(mock: &MockServer, allow: &[&str]) -> vivido_mcp::server::Server {
    use vivido_mcp::server::ServerConfig;
    let allowance =
        vivido_mcp::tiers::parse_allow(&allow.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .expect("parse tiers");
    vivido_mcp::server::Server::new(ServerConfig {
        allowance,
        socket: Some(mock.socket.clone()),
        target: None,
        vivido_bin: "vivido".to_owned(),
        default_max_dim: 1280,
    })
}
