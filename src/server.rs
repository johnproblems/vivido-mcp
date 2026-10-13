//! Tool dispatch: one long-lived Vivido connection plus composites.
//!
//! [`Server`] holds the main IPC connection (opened lazily, re-resolved on
//! demand), the tool catalog (generated from the live `hello`, with a compiled
//! fallback before the first connect), the [`DrainState`] subscription, and the
//! [`Redactor`]. Every tool result is redacted before it reaches the client.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::endpoint;
use crate::events::{self, DrainFilter, DrainState};
use crate::ipc::{Connection, Failure, HelloInfo};
use crate::sanitize::Redactor;
use crate::tiers::{Allowance, refusal};
use crate::tools::{self, ToolDef, ToolKind};

/// Server configuration from CLI flags.
pub struct ServerConfig {
    /// Tiers the operator enabled.
    pub allowance: Allowance,
    /// Explicit `--socket` path, if any.
    pub socket: Option<PathBuf>,
    /// Explicit `--target` name, if any.
    pub target: Option<String>,
    /// Vivido binary for headless lifecycle.
    pub vivido_bin: String,
    /// Default longest-edge cap for screenshots.
    pub default_max_dim: u32,
}

/// One tool result.
pub struct ToolOutput {
    /// MCP `content` blocks.
    pub content: Vec<Value>,
    /// MCP `structuredContent`.
    pub structured: Value,
    /// MCP `isError`.
    pub is_error: bool,
}

impl ToolOutput {
    fn ok_text(structured: Value) -> Self {
        let text = structured.to_string();
        Self { content: vec![json!({ "type": "text", "text": text })], structured, is_error: false }
    }

    fn ok_image(base64_image: String, mime: &str, metadata: Value) -> Self {
        let text = metadata.to_string();
        Self {
            content: vec![
                json!({ "type": "image", "data": base64_image, "mimeType": mime }),
                json!({ "type": "text", "text": text }),
            ],
            structured: metadata,
            is_error: false,
        }
    }

    fn err(code: &str, message: String) -> Self {
        let structured = json!({ "error": { "code": code, "message": message } });
        let text = structured.to_string();
        Self { content: vec![json!({ "type": "text", "text": text })], structured, is_error: true }
    }
}

/// The MCP tool server.
pub struct Server {
    config: ServerConfig,
    conn: Option<Connection>,
    endpoint_path: Option<PathBuf>,
    endpoint_label: Option<String>,
    hello: Option<HelloInfo>,
    catalog: Vec<ToolDef>,
    drain: DrainState,
    redactor: Redactor,
}

impl Server {
    /// Build the server, attempting an initial connection (best effort: when
    /// Vivido is not reachable yet, tools list from the fallback catalog and
    /// the first tool call reports how to reach an endpoint).
    pub fn new(config: ServerConfig) -> Self {
        let mut server = Self {
            config,
            conn: None,
            endpoint_path: None,
            endpoint_label: None,
            hello: None,
            catalog: tools::fallback_catalog(),
            drain: DrainState::default(),
            redactor: Redactor::new(),
        };
        if let Ok(socket) = std::env::var(endpoint::VIVIDO_SOCKET_ENV) {
            server.redactor.add(&socket);
        }
        match server.connect() {
            Ok(()) => {
                let label = server.endpoint_label.clone().unwrap_or_default();
                eprintln!("vivido-mcp: connected ({label}); {} tools", server.catalog.len());
            }
            Err(message) => {
                eprintln!(
                    "vivido-mcp: starting disconnected: {message}; {} fallback tools",
                    server.catalog.len()
                );
            }
        }
        server
    }

    /// Current tool catalog.
    pub fn catalog(&self) -> &[ToolDef] {
        &self.catalog
    }

    /// Whether the main connection is currently up.
    pub fn connected(&self) -> bool {
        self.conn.is_some()
    }

    /// One-line session context for prompts: enabled tiers plus what is attached.
    pub fn context(&self) -> String {
        let tiers = if self.config.allowance.process {
            "observe,input,process"
        } else if self.config.allowance.input {
            "observe,input"
        } else {
            "observe"
        };
        let attached = self
            .hello
            .as_ref()
            .map(|hello| {
                let session = hello
                    .session
                    .clone()
                    .or_else(|| hello.automation_name.clone())
                    .unwrap_or_else(|| "unnamed".to_owned());
                format!(
                    "{} {} ({})",
                    hello.server_version,
                    if hello.headless { "headless" } else { "headed" },
                    session
                )
            })
            .unwrap_or_else(|| "not connected".to_owned());
        format!("enabled tiers: {tiers}; attached: {attached}")
    }

    /// Call one tool by name.
    pub fn call(&mut self, name: &str, args: Value) -> ToolOutput {
        let args = if args.is_null() { json!({}) } else { args };
        let Some(tool) = tools::find(&self.catalog, name).cloned() else {
            let candidates: Vec<&str> =
                self.catalog.iter().map(|tool| tool.name.as_str()).collect();
            let mut message = format!("unknown tool {name:?}");
            if let Some(guess) = tools::suggest(name, &candidates) {
                message.push_str(&format!(", did you mean `{guess}`?"));
            }
            return ToolOutput::err("tool_not_found", message);
        };
        if !self.config.allowance.allows(tool.tier) {
            return ToolOutput::err(
                "tier_refused",
                refusal(&tool.name, tool.tier, self.config.allowance),
            );
        }
        let timed = std::env::var("VIVIDO_MCP_LOG_TIMINGS").as_deref() == Ok("1");
        let started = std::time::Instant::now();
        let output = match &tool.kind {
            ToolKind::HeadlessStart => self.call_headless_start(&args),
            ToolKind::HeadlessAttach => self.call_headless_attach(&args),
            ToolKind::HeadlessStop => self.call_headless_stop(&args),
            ToolKind::HeadlessList => self.call_headless_list(&args),
            ToolKind::Unsubscribe => Ok(ToolOutput::ok_text(self.drain.reset())),
            _ => match self.ensure_connected() {
                Ok(()) => self.call_connected(&tool, &args),
                Err(message) => Err((
                    format!(
                        "{message}; start Vivido, or call headless_start (needs `--allow process`) \
                         to create an ephemeral session"
                    ),
                    "transport_error".to_owned(),
                )),
            },
        };
        if timed {
            eprintln!("vivido-mcp: timing: `{}` took {:?}", name, started.elapsed());
        }
        match output {
            Ok(output) => self.redact_output(output),
            Err((message, code)) => {
                let mut output = ToolOutput::err(&code, message);
                output.structured = self.redactor.redact(output.structured);
                let text = output.structured.to_string();
                output.content = vec![json!({ "type": "text", "text": text })];
                output
            }
        }
    }

    fn redact_output(&self, mut output: ToolOutput) -> ToolOutput {
        output.structured = self.redactor.redact(output.structured.clone());
        output.content = output
            .content
            .into_iter()
            .map(|block| {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    let text = output.structured.to_string();
                    json!({ "type": "text", "text": text })
                } else {
                    block
                }
            })
            .collect();
        output
    }

    fn call_connected(&mut self, tool: &ToolDef, args: &Value) -> Result<ToolOutput, ToolError> {
        match &tool.kind {
            ToolKind::Wire { method } => {
                let params = tools::translate(method, args)
                    .map_err(|message| (message, "invalid_args".to_owned()))?;
                let result = self.request(method, params)?;
                let result = post_process(method, result, args)?;
                Ok(ToolOutput::ok_text(result))
            }
            ToolKind::Passthrough { method } => {
                let result = self.request(method, args.clone())?;
                Ok(ToolOutput::ok_text(result))
            }
            ToolKind::Screenshot => self.call_screenshot(args),
            ToolKind::Capture => self.call_capture(args),
            ToolKind::DrainEvents => self.call_drain(args),
            ToolKind::SubscribeCollect => self.call_subscribe(args),
            ToolKind::Unsubscribe
            | ToolKind::HeadlessStart
            | ToolKind::HeadlessAttach
            | ToolKind::HeadlessStop
            | ToolKind::HeadlessList => unreachable!("handled before connect"),
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, ToolError> {
        let conn = self.conn.as_mut().expect("connected");
        let result = conn.request(method, params);
        match result {
            Ok(value) => Ok(value),
            Err(failure) => {
                let labeled = labeled_failure(self.endpoint_label.as_deref(), &failure);
                if matches!(failure, Failure::Transport(_) | Failure::Protocol(_)) {
                    self.drop_connection();
                }
                Err((labeled, failure_code(&failure)))
            }
        }
    }

    fn call_screenshot(&mut self, args: &Value) -> Result<ToolOutput, ToolError> {
        check_args("screenshot", args)?;
        let window_id = args.get("window_id").cloned().unwrap_or(Value::Null);
        let result = self.request("screenshot", json!({ "window_id": window_id }))?;
        let image =
            crate::image_tools::process_screenshot(&result, args, self.config.default_max_dim)
                .map_err(|message| (message, "image_error".to_owned()))?;
        Ok(ToolOutput::ok_image(image.base64_image, image.mime, image.metadata))
    }

    fn call_capture(&mut self, args: &Value) -> Result<ToolOutput, ToolError> {
        check_args("capture", args)?;
        let window_id = args.get("window_id").cloned().unwrap_or(Value::Null);
        let timeout = tools::parse_timeout(args, "timeout", 30_000)
            .map_err(|message| (message, "invalid_args".to_owned()))?;
        let mut activated = false;
        if args.get("activate").and_then(Value::as_bool) == Some(true) {
            let advertised = self.hello.as_ref().is_some_and(|hello| {
                hello.methods.iter().any(|method| method == "vivida_activate_pane")
            });
            if !advertised {
                return Err((
                    "this endpoint does not advertise `vivida_activate_pane`; `activate` needs a Vivida host"
                        .to_owned(),
                    "invalid_args".to_owned(),
                ));
            }
            self.request("vivida_activate_pane", json!({ "window_id": window_id }))?;
            activated = true;
        }
        let mut waited_frame = false;
        if let Some(after_frame) = args.get("after_frame").filter(|value| !value.is_null()) {
            let sequence = after_frame.as_u64().ok_or_else(|| {
                ("`after_frame` must be an integer".to_owned(), "invalid_args".to_owned())
            })?;
            self.request(
                "wait_frame",
                json!({
                    "after_frame": sequence,
                    "common": { "timeout": timeout, "target": { "window_id": window_id } },
                }),
            )?;
            waited_frame = true;
        }
        let mut waited_stable = false;
        if args.get("stable").and_then(Value::as_bool) == Some(true) {
            let quiet = tools::parse_timeout(args, "quiet", 250)
                .map_err(|message| (message, "invalid_args".to_owned()))?;
            self.request(
                "wait_screen_stable",
                json!({
                    "quiet": quiet,
                    "after_screen": Value::Null,
                    "common": { "timeout": timeout, "target": { "window_id": window_id } },
                }),
            )?;
            waited_stable = true;
        }
        let result = self.request("screenshot", json!({ "window_id": window_id }))?;
        let image =
            crate::image_tools::process_screenshot(&result, args, self.config.default_max_dim)
                .map_err(|message| (message, "image_error".to_owned()))?;
        let mut metadata = image.metadata;
        metadata["activated"] = json!(activated);
        metadata["waited_frame"] = json!(waited_frame);
        metadata["waited_stable"] = json!(waited_stable);
        Ok(ToolOutput::ok_image(image.base64_image, image.mime, metadata))
    }

    fn call_drain(&mut self, args: &Value) -> Result<ToolOutput, ToolError> {
        check_args("drain_events", args)?;
        let filter = drain_filter(args)?;
        let cursor = args
            .get("cursor")
            .filter(|value| !value.is_null())
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    ("`cursor` must be an integer".to_owned(), "invalid_args".to_owned())
                })
            })
            .transpose()?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value.clamp(1, 1000) as usize)
            .unwrap_or(100);
        let timeout_ms = tools::parse_timeout(args, "timeout", 2_000)
            .map_err(|message| (message, "invalid_args".to_owned()))?;
        if timeout_ms > 60_000 {
            return Err(("`timeout` must be at most 60s".to_owned(), "invalid_args".to_owned()));
        }
        let endpoint = self.endpoint_path.clone().expect("connected");
        let allowed = self.hello.as_ref().map(|hello| hello.event_kinds.as_slice());
        self.drain
            .drain(&endpoint, filter, cursor, limit, Duration::from_millis(timeout_ms), allowed)
            .map(ToolOutput::ok_text)
            .map_err(|failure| (failure.message(), failure_code(&failure)))
    }

    fn call_subscribe(&mut self, args: &Value) -> Result<ToolOutput, ToolError> {
        check_args("subscribe", args)?;
        let filter = drain_filter(args)?;
        let since_event = args
            .get("since_event")
            .filter(|value| !value.is_null())
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    ("`since_event` must be an integer".to_owned(), "invalid_args".to_owned())
                })
            })
            .transpose()?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value.clamp(1, 1000) as usize)
            .unwrap_or(100);
        let timeout_ms = tools::parse_timeout(args, "timeout", 5_000)
            .map_err(|message| (message, "invalid_args".to_owned()))?;
        if timeout_ms > 60_000 {
            return Err(("`timeout` must be at most 60s".to_owned(), "invalid_args".to_owned()));
        }
        if let Some(hello) = &self.hello {
            for kind in &filter.events {
                if !hello.event_kinds.contains(kind) {
                    return Err((
                        format!(
                            "unknown event kind {kind:?}; this endpoint advertises: {}",
                            hello.event_kinds.join(", ")
                        ),
                        "invalid_args".to_owned(),
                    ));
                }
            }
        }
        let endpoint = self.endpoint_path.clone().expect("connected");
        events::collect_once(
            &endpoint,
            filter,
            since_event,
            limit,
            Duration::from_millis(timeout_ms),
        )
        .map(ToolOutput::ok_text)
        .map_err(|failure| (failure.message(), failure_code(&failure)))
    }

    fn call_headless_start(&mut self, args: &Value) -> Result<ToolOutput, ToolError> {
        check_args("headless_start", args)?;
        let options = crate::headless::StartOptions {
            session: args.get("session").and_then(Value::as_str).map(str::to_owned),
            size: args
                .get("size")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| Some("1280x720px".to_owned())),
            program: args.get("program").and_then(Value::as_str).map(str::to_owned),
            args: args
                .get("args")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
            ephemeral: args.get("ephemeral").and_then(Value::as_bool).unwrap_or(true),
        };
        let size = options.size.clone();
        let ephemeral = options.ephemeral;
        let started = crate::headless::start(&self.config.vivido_bin, options)
            .map_err(|message| (message, "headless_error".to_owned()))?;
        self.redactor.add(&started.socket.to_string_lossy());
        // Attach to the session we just started.
        if let Err(message) = self.connect_to(started.socket.clone(), started.session.clone()) {
            return Err((
                format!(
                    "session {:?} started but attaching failed: {message}; retry `headless_attach`",
                    started.session
                ),
                "headless_error".to_owned(),
            ));
        }
        let windows = self
            .request("list_windows", json!({}))
            .ok()
            .and_then(|value| value.get("windows").cloned())
            .unwrap_or(Value::Null);
        Ok(ToolOutput::ok_text(json!({
            "session": started.session,
            "attached": true,
            "size": size,
            "ephemeral": ephemeral,
            "windows": windows,
        })))
    }

    fn call_headless_attach(&mut self, args: &Value) -> Result<ToolOutput, ToolError> {
        check_args("headless_attach", args)?;
        let target = args
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| ("missing required `target`".to_owned(), "invalid_args".to_owned()))?;
        let endpoint = endpoint::resolve(None, Some(target))
            .map_err(|message| (message, "transport_error".to_owned()))?;
        self.redactor.add(&endpoint.path.to_string_lossy());
        let already = already_attached(self.hello.as_ref(), target);
        self.connect_to(endpoint.path, endpoint.label)
            .map_err(|message| (message, "transport_error".to_owned()))?;
        let hello = self.hello.clone().expect("connected");
        Ok(ToolOutput::ok_text(json!({
            "attached": true,
            "already_attached": already,
            "target": target,
            "server_version": hello.server_version,
            "headless": hello.headless,
            "session": hello.session,
            "automation_name": hello.automation_name,
            "methods": hello.methods.len(),
            "limits": hello.limits,
        })))
    }

    fn call_headless_list(&self, args: &Value) -> Result<ToolOutput, ToolError> {
        check_args("headless_list", args)?;
        let sessions: Vec<Value> = endpoint::list_sessions()
            .iter()
            .map(|session| {
                json!({
                    "name": session.name,
                    "headless": session.headless,
                    "pid": session.pid,
                    "live": session.live,
                })
            })
            .collect();
        Ok(ToolOutput::ok_text(json!({ "sessions": sessions })))
    }

    fn call_headless_stop(&mut self, args: &Value) -> Result<ToolOutput, ToolError> {
        check_args("headless_stop", args)?;
        let target = args
            .get("target")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                self.hello.as_ref().and_then(|hello| {
                    hello.session.clone().or_else(|| hello.automation_name.clone())
                })
            })
            .ok_or_else(|| {
                ("no attached session; pass `target`".to_owned(), "invalid_args".to_owned())
            })?;
        let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
        let stopped_as = crate::headless::stop(&self.config.vivido_bin, &target, force, || {
            let endpoint =
                endpoint::resolve(None, Some(&target)).map_err(|message| message.to_string())?;
            let mut conn =
                Connection::connect(&endpoint.path).map_err(|failure| failure.message())?;
            conn.request("quit", json!({})).map(|_| ()).map_err(|failure| failure.message())
        })
        .map_err(|message| (message, "headless_error".to_owned()))?;
        // If we stopped the attached session, drop the dead connection; the
        // next call re-resolves from the launch configuration.
        let attached = self
            .hello
            .as_ref()
            .and_then(|hello| hello.session.clone().or_else(|| hello.automation_name.clone()));
        if attached.as_deref() == Some(target.as_str()) {
            self.drop_connection();
        }
        Ok(ToolOutput::ok_text(json!({
            "session": target,
            "stopped": true,
            "method": stopped_as,
        })))
    }

    fn ensure_connected(&mut self) -> Result<(), String> {
        if self.conn.is_some() {
            return Ok(());
        }
        self.connect()
    }

    /// Resolve from the launch configuration and connect.
    fn connect(&mut self) -> Result<(), String> {
        // Register the configured path before resolving so even a failed
        // resolution redacts in place instead of hiding the whole error.
        if let Some(socket) = &self.config.socket {
            self.redactor.add(&socket.to_string_lossy());
        }
        let endpoint =
            endpoint::resolve(self.config.socket.clone(), self.config.target.as_deref())?;
        self.redactor.add(&endpoint.path.to_string_lossy());
        self.connect_to(endpoint.path, endpoint.label)
    }

    /// Connect to one endpoint path, handshake, and rebuild the catalog.
    fn connect_to(&mut self, path: PathBuf, label: String) -> Result<(), String> {
        let mut conn = Connection::connect(&path).map_err(|failure| failure.message())?;
        let hello = conn.hello().map_err(|failure| failure.message())?;
        eprintln!(
            "vivido-mcp: endpoint is Vivido {} ({}, {})",
            hello.server_version,
            if hello.headless { "headless" } else { "headed" },
            hello
                .session
                .clone()
                .or_else(|| hello.automation_name.clone())
                .unwrap_or_else(|| "unnamed".to_owned()),
        );
        self.catalog = tools::build_catalog(&hello);
        self.conn = Some(conn);
        self.endpoint_path = Some(path);
        self.endpoint_label = Some(label);
        self.hello = Some(hello);
        Ok(())
    }

    fn drop_connection(&mut self) {
        self.conn = None;
        self.endpoint_path = None;
        self.endpoint_label = None;
        self.hello = None;
        self.drain.reset();
    }
}

/// A tool failure: human message plus machine code.
type ToolError = (String, String);

/// Prefix connection-level failures with the endpoint label so parallel
/// sessions stay distinguishable. Labels never contain paths.
fn labeled_failure(label: Option<&str>, failure: &Failure) -> String {
    match failure {
        Failure::Transport(_) | Failure::Protocol(_) => match label {
            Some(label) => format!("({label}) {}", failure.message()),
            None => failure.message(),
        },
        Failure::Server(_) => failure.message(),
    }
}

/// Whether `target` names the session this server is already attached to.
fn already_attached(hello: Option<&HelloInfo>, target: &str) -> bool {
    hello
        .and_then(|hello| hello.session.clone().or_else(|| hello.automation_name.clone()))
        .as_deref()
        == Some(target)
}

fn failure_code(failure: &Failure) -> String {
    match failure {
        Failure::Transport(_) => "transport_error".to_owned(),
        Failure::Protocol(_) => "protocol_error".to_owned(),
        Failure::Server(error) => error.code.clone(),
    }
}

/// Reject unknown arguments for composite tools (wire tools check in `translate`).
fn check_args(tool: &str, args: &Value) -> Result<(), ToolError> {
    let expected: &[&str] = match tool {
        "screenshot" => &["window_id", "crop", "max_dim", "format", "quality"],
        "capture" => &[
            "window_id",
            "activate",
            "after_frame",
            "stable",
            "quiet",
            "timeout",
            "crop",
            "max_dim",
        ],
        "drain_events" => &["cursor", "window_id", "all", "events", "limit", "timeout"],
        "subscribe" => &["window_id", "all", "events", "since_event", "limit", "timeout"],
        "headless_start" => &["session", "size", "program", "args", "ephemeral"],
        "headless_attach" => &["target"],
        "headless_stop" => &["target", "force"],
        "headless_list" => &[],
        _ => &[],
    };
    tools::check_known_args(tool, args, expected)
        .map_err(|message| (message, "invalid_args".to_owned()))
}

fn drain_filter(args: &Value) -> Result<DrainFilter, ToolError> {
    let window_id = args
        .get("window_id")
        .filter(|value| !value.is_null())
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                ("`window_id` must be an integer".to_owned(), "invalid_args".to_owned())
            })
        })
        .transpose()?;
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    if all && window_id.is_some() {
        return Err((
            "`window_id` and `all` are mutually exclusive".to_owned(),
            "invalid_args".to_owned(),
        ));
    }
    let events = match args.get("events") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(kind)) => vec![kind.clone()],
        Some(Value::Array(items)) => {
            items.iter().filter_map(Value::as_str).map(str::to_owned).collect()
        }
        Some(_) => {
            return Err((
                "`events` must be a kind or an array of kinds".to_owned(),
                "invalid_args".to_owned(),
            ));
        }
    };
    Ok(DrainFilter { window_id, all, events })
}

/// Client-side result shaping: `transcript` with `raw` decodes to text.
fn post_process(method: &str, result: Value, args: &Value) -> Result<Value, ToolError> {
    if method == "transcript" && args.get("raw").and_then(Value::as_bool) == Some(true) {
        let encoded = result.get("data").and_then(Value::as_str).ok_or_else(|| {
            ("transcript reply is missing data".to_owned(), "protocol_error".to_owned())
        })?;
        let bytes = base64_decode(encoded).map_err(|_| {
            ("transcript data is not valid base64".to_owned(), "protocol_error".to_owned())
        })?;
        let (text, lossy) = match String::from_utf8(bytes.clone()) {
            Ok(text) => (text, false),
            Err(_) => (String::from_utf8_lossy(&bytes).into_owned(), true),
        };
        return Ok(json!({
            "text": text,
            "bytes": bytes.len(),
            "lossy": lossy,
            "truncated": result.get("truncated").cloned().unwrap_or(Value::Null),
            "start_offset": result.get("start_offset").cloned().unwrap_or(Value::Null),
            "end_offset": result.get("end_offset").cloned().unwrap_or(Value::Null),
            "current_end_offset": result.get("current_end_offset").cloned().unwrap_or(Value::Null),
        }));
    }
    Ok(result)
}

fn base64_decode(text: &str) -> Result<Vec<u8>, ()> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(text).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_failures_carry_the_endpoint_label() {
        let failure = Failure::Transport("boom".to_owned());
        assert_eq!(labeled_failure(Some("target \"build\""), &failure), "(target \"build\") boom");
        assert_eq!(labeled_failure(None, &failure), "boom");
        let server_failure = Failure::Server(crate::ipc::IpcError {
            code: "timeout".to_owned(),
            message: "slow".to_owned(),
            data: None,
        });
        // Server errors already name their own cause; no label prefix.
        assert!(!labeled_failure(Some("target \"build\""), &server_failure).contains("build"));
    }

    #[test]
    fn attach_detects_the_current_session() {
        assert!(!already_attached(None, "build"));
        let hello = HelloInfo {
            server_version: "x".to_owned(),
            headless: true,
            session: Some("build".to_owned()),
            automation_name: None,
            methods: Vec::new(),
            capabilities: Vec::new(),
            event_kinds: Vec::new(),
            limits: Value::Null,
        };
        assert!(already_attached(Some(&hello), "build"));
        assert!(!already_attached(Some(&hello), "other"));
    }
}
