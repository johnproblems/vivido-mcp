//! Tool catalog: generated from the live `hello` handshake.
//!
//! Every method the endpoint advertises becomes one MCP tool with the same
//! snake_case name, so the server tracks Vivido and Vivida versions without hand
//! edits. Known methods get precise schemas plus friendly flat arguments (a
//! top-level `window_id`, `timeout` as milliseconds or `"30s"`); unknown or
//! host-claimed methods get a generic passthrough tool that forwards its
//! arguments verbatim, tiered by their advertised class and mutating flag.
//!
//! Our own composite tools sit alongside the wire methods:
//!
//! - `capture`: settle-then-screenshot returning an image (observe).
//! - `drain_events`: poll the server-side event subscription (observe).
//! - `headless_start` / `headless_attach` / `headless_stop`: session lifecycle.
//! - `type_text`: alias of the `typing` wire method (input).

use serde_json::{Value, json};

use crate::ipc::HelloInfo;
use crate::tiers::{Tier, tier_for_method};

/// How a tool executes.
#[derive(Clone, Debug)]
pub enum ToolKind {
    /// A known wire method; arguments are translated by [`translate`].
    Wire {
        /// Method name as `hello` spells it.
        method: &'static str,
    },
    /// An unknown or host-claimed method; the whole argument object is the params.
    Passthrough {
        /// Method name as `hello` spells it.
        method: String,
    },
    /// `screenshot` with image content, cropping, downscaling, and temp cleanup.
    Screenshot,
    /// Settle-then-screenshot composite returning an image.
    Capture,
    /// Poll the server-side event subscription.
    DrainEvents,
    /// One-shot `subscribe`: collect events briefly, then unsubscribe.
    SubscribeCollect,
    /// Drop the `drain_events` subscription and cursor.
    Unsubscribe,
    /// Start an ephemeral headless session and attach to it.
    HeadlessStart,
    /// Attach this server to another named session.
    HeadlessAttach,
    /// Stop a headless session gracefully, forcefully on request.
    HeadlessStop,
    /// List registered sessions with liveness.
    HeadlessList,
}

/// One MCP tool: definition plus execution kind.
#[derive(Clone, Debug)]
pub struct ToolDef {
    /// Tool name (wire methods keep their `hello` spelling).
    pub name: String,
    /// One-paragraph description for the model.
    pub description: String,
    /// JSON Schema for the tool arguments.
    pub schema: Value,
    /// Safety tier gating this tool.
    pub tier: Tier,
    /// How the tool executes.
    pub kind: ToolKind,
}

impl ToolDef {
    /// MCP `tools/list` entry, with tier-mirroring annotations.
    pub fn listing(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.schema,
            "annotations": crate::tiers::annotations(self.tier),
        })
    }
}

/// Build the catalog from a live `hello` handshake.
pub fn build_catalog(hello: &HelloInfo) -> Vec<ToolDef> {
    let mut tools = Vec::new();
    for method in &hello.methods {
        // The handshake itself is connection plumbing, not a tool.
        if method == "hello" {
            continue;
        }
        let (class, mutating, host_claimed) = hello
            .capability(method)
            .map(|capability| {
                (capability.class.as_str(), capability.mutating, capability.host_claimed)
            })
            // An unclassified method is a mutating extension until proven otherwise.
            .unwrap_or(("extension", true, false));
        let tier = tier_for_method(class, mutating);
        let kind = match method.as_str() {
            "screenshot" => ToolKind::Screenshot,
            "subscribe" => ToolKind::SubscribeCollect,
            "unsubscribe" => ToolKind::Unsubscribe,
            _ if is_known_method(method) => ToolKind::Wire { method: known_method(method) },
            _ => ToolKind::Passthrough { method: method.clone() },
        };
        let (description, schema) = definition_for(method, &kind, class, mutating, host_claimed);
        tools.push(ToolDef { name: method.clone(), description, schema, tier, kind });
    }
    // Alias for the PRD spelling: the wire method is `typing`.
    if hello.methods.iter().any(|method| method == "typing") {
        tools.push(ToolDef {
            name: "type_text".to_owned(),
            description: "Alias of `typing`: write literal UTF-8 text to the terminal PTY. \
                (Input tier: needs `--allow input`.)"
                .to_owned(),
            schema: schema_for("typing"),
            tier: Tier::Input,
            kind: ToolKind::Wire { method: "typing" },
        });
    }
    tools.push(ToolDef {
        name: "capture".to_owned(),
        description: CAPTURE_DESCRIPTION.to_owned(),
        schema: capture_schema(),
        tier: Tier::Observe,
        kind: ToolKind::Capture,
    });
    tools.push(ToolDef {
        name: "drain_events".to_owned(),
        description: DRAIN_DESCRIPTION.to_owned(),
        schema: drain_schema(),
        tier: Tier::Observe,
        kind: ToolKind::DrainEvents,
    });
    for (name, description, schema) in headless_definitions() {
        // Listing sessions only reads rendezvous files and probes liveness.
        let tier = if name == "headless_list" { Tier::Observe } else { Tier::Process };
        tools.push(ToolDef {
            name: name.to_owned(),
            description: description.to_owned(),
            schema,
            tier,
            kind: match name {
                "headless_start" => ToolKind::HeadlessStart,
                "headless_attach" => ToolKind::HeadlessAttach,
                "headless_list" => ToolKind::HeadlessList,
                _ => ToolKind::HeadlessStop,
            },
        });
    }
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    tools
}

/// Catalog used before the first successful `hello` (Vivido not yet reachable).
/// Mirrors the upstream method table; the live handshake replaces it on connect.
pub fn fallback_catalog() -> Vec<ToolDef> {
    let hello = HelloInfo {
        server_version: "unknown".to_owned(),
        headless: false,
        session: None,
        automation_name: None,
        methods: FALLBACK_METHODS.iter().map(|entry| entry.0.to_owned()).collect(),
        capabilities: FALLBACK_METHODS
            .iter()
            .map(|entry| crate::ipc::MethodCapability {
                name: entry.0.to_owned(),
                class: entry.1.to_owned(),
                mutating: entry.2,
                host_claimed: false,
            })
            .collect(),
        event_kinds: Vec::new(),
        limits: Value::Null,
    };
    build_catalog(&hello)
}

/// Upstream method table (`METHODS` + `method_class`), for disconnected boot only.
const FALLBACK_METHODS: &[(&str, &str, bool)] = &[
    ("ping", "observe", false),
    ("reset_terminal", "lifecycle", true),
    ("restart_terminal", "lifecycle", true),
    ("unsubscribe", "observe", false),
    ("create_window", "window", true),
    ("close_window", "lifecycle", true),
    ("config", "config", true),
    ("get_config", "observe", false),
    ("typing", "input", true),
    ("get_text", "observe", false),
    ("find_text", "observe", false),
    ("screenshot", "observe", false),
    ("key", "input", true),
    ("paste", "input", true),
    ("drop_file", "input", true),
    ("mouse", "input", true),
    ("resize", "window", true),
    ("set_geometry", "window", true),
    ("set_geometry_batch", "window", true),
    ("set_visible", "window", true),
    ("set_level", "window", true),
    ("focus", "process", true),
    ("signal", "process", true),
    ("exec", "process", true),
    ("list_windows", "observe", false),
    ("inspect", "observe", false),
    ("diagnose", "observe", false),
    ("vivid_sessions", "observe", false),
    ("vivid_surfaces", "observe", false),
    ("vivid_surface_status", "observe", false),
    ("vivid_tracks", "observe", false),
    ("vivid_track_status", "observe", false),
    ("vivid_scene_status", "observe", false),
    ("vivid_trace", "observe", false),
    ("get_grid", "observe", false),
    ("wait_text", "observe", false),
    ("wait_output", "observe", false),
    ("wait_screen_change", "observe", false),
    ("wait_screen_stable", "observe", false),
    ("wait_frame", "observe", false),
    ("wait_exit", "observe", false),
    ("wait_prompt", "observe", false),
    ("wait_command_finish", "observe", false),
    ("quit", "lifecycle", true),
    ("wait_vivid_track", "observe", false),
    ("transcript", "observe", false),
    ("subscribe", "observe", false),
];

/// Find a tool in the catalog by name.
pub fn find<'a>(catalog: &'a [ToolDef], name: &str) -> Option<&'a ToolDef> {
    catalog.iter().find(|tool| tool.name == name)
}

fn definition_for(
    method: &str,
    kind: &ToolKind,
    class: &str,
    mutating: bool,
    host_claimed: bool,
) -> (String, Value) {
    match kind {
        ToolKind::Wire { .. } => (description_for(method).to_owned(), schema_for(method)),
        ToolKind::Screenshot => (description_for("screenshot").to_owned(), screenshot_schema()),
        ToolKind::SubscribeCollect => (SUBSCRIBE_DESCRIPTION.to_owned(), subscribe_schema()),
        ToolKind::Unsubscribe => (
            "Drop the `drain_events` server-side subscription and reset its cursor. \
              Next `drain_events` call resubscribes from now."
                .to_owned(),
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        ),
        ToolKind::Passthrough { .. } => (
            format!(
                "{host} `{method}` method (class `{class}`, mutating: {mutating}). \
                 Arguments are passed to Vivido verbatim as the method params; see \
                 `vivido msg capabilities` for its contract.",
                host = if host_claimed { "Host-provided" } else { "Newly advertised" },
            ),
            json!({
                "type": "object",
                "description": format!(
                    "Raw params for the host-provided `{method}` method, passed verbatim."
                ),
            }),
        ),
        _ => unreachable!("composites are not built from wire methods"),
    }
}

// --- Argument helpers -------------------------------------------------------

/// Read an optional argument, distinguishing absent/null from present.
fn arg<'a>(args: &'a Value, name: &str) -> Option<&'a Value> {
    args.get(name).filter(|value| !value.is_null())
}

fn arg_u64(args: &Value, name: &str) -> Result<Option<u64>, String> {
    match arg(args, name) {
        None => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .ok_or_else(|| format!("`{name}` must be a non-negative integer"))
            .map(Some),
        Some(_) => Err(format!("`{name}` must be an integer")),
    }
}

fn required_u64(args: &Value, name: &str) -> Result<u64, String> {
    arg_u64(args, name)?.ok_or_else(|| format!("missing required `{name}`"))
}

fn arg_i64(args: &Value, name: &str) -> Result<Option<i64>, String> {
    match arg(args, name) {
        None => Ok(None),
        Some(Value::Number(number)) => {
            number.as_i64().ok_or_else(|| format!("`{name}` must be an integer")).map(Some)
        }
        Some(_) => Err(format!("`{name}` must be an integer")),
    }
}

fn arg_u16(args: &Value, name: &str, min: u16, max: u16) -> Result<Option<u16>, String> {
    match arg_u64(args, name)? {
        None => Ok(None),
        Some(number) if number >= u64::from(min) && number <= u64::from(max) => {
            Ok(Some(number as u16))
        }
        Some(_) => Err(format!("`{name}` must be {min} through {max}")),
    }
}

fn arg_u32(args: &Value, name: &str) -> Result<Option<u32>, String> {
    match arg_u64(args, name)? {
        None => Ok(None),
        Some(number) if number <= u64::from(u32::MAX) => Ok(Some(number as u32)),
        Some(_) => Err(format!("`{name}` is too large")),
    }
}

fn arg_i32(args: &Value, name: &str) -> Result<Option<i32>, String> {
    match arg_i64(args, name)? {
        None => Ok(None),
        Some(number) if number >= i64::from(i32::MIN) && number <= i64::from(i32::MAX) => {
            Ok(Some(number as i32))
        }
        Some(_) => Err(format!("`{name}` is out of range")),
    }
}

fn arg_f64(args: &Value, name: &str) -> Result<Option<f64>, String> {
    match arg(args, name) {
        None => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| format!("`{name}` must be a finite number")),
        Some(_) => Err(format!("`{name}` must be a number")),
    }
}

fn arg_bool(args: &Value, name: &str, default: bool) -> Result<bool, String> {
    match arg(args, name) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("`{name}` must be a boolean")),
    }
}

fn arg_string(args: &Value, name: &str) -> Result<Option<String>, String> {
    match arg(args, name) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("`{name}` must be a string")),
    }
}

fn required_string(args: &Value, name: &str) -> Result<String, String> {
    arg_string(args, name)?.ok_or_else(|| format!("missing required `{name}`"))
}

fn arg_string_list(args: &Value, name: &str) -> Result<Vec<String>, String> {
    match arg(args, name) {
        None => Ok(Vec::new()),
        // A single string coerces to a one-element list.
        Some(Value::String(text)) => Ok(vec![text.clone()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("`{name}` must be an array of strings"))
            })
            .collect(),
        Some(_) => Err(format!("`{name}` must be an array of strings")),
    }
}

/// Timeout in milliseconds: a JSON integer or a `"500ms"`/`"30s"`/`"5m"`/`"1h"`
/// duration string (bare numbers are milliseconds), 1 ms through 24 h.
pub fn parse_timeout(args: &Value, name: &str, default_ms: u64) -> Result<u64, String> {
    match arg(args, name) {
        None => Ok(default_ms),
        Some(Value::Number(number)) => {
            let ms = number.as_u64().ok_or_else(|| format!("`{name}` must be 1ms through 24h"))?;
            check_timeout_range(name, ms)
        }
        Some(Value::String(text)) => parse_duration(text)
            .map_err(|_| {
                format!(
                    "`{name}` must be milliseconds or a duration like \"500ms\", \"30s\", \"5m\""
                )
            })
            .and_then(|ms| check_timeout_range(name, ms)),
        Some(_) => Err(format!("`{name}` must be milliseconds or a duration string")),
    }
}

fn check_timeout_range(name: &str, ms: u64) -> Result<u64, String> {
    if (1..=86_400_000).contains(&ms) {
        Ok(ms)
    } else {
        Err(format!("`{name}` must be 1ms through 24h"))
    }
}

/// Parse a CLI-style duration (`500ms`, `30s`, `5m`, `1h`, bare milliseconds).
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let (number, multiplier) = if let Some(number) = text.strip_suffix("ms") {
        (number, 1)
    } else if let Some(number) = text.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = text.strip_suffix('m') {
        (number, 60_000)
    } else if let Some(number) = text.strip_suffix('h') {
        (number, 3_600_000)
    } else {
        (text, 1)
    };
    let number = number.parse::<u64>().map_err(|_| format!("invalid duration {text:?}"))?;
    number.checked_mul(multiplier).ok_or_else(|| format!("duration {text:?} is too large"))
}

/// `{"window_id": N|null}` for methods taking the id directly.
fn direct_target(window_id: Option<u64>) -> Value {
    json!({ "window_id": window_id })
}

/// `{"common": {"timeout": ms, "target": {"window_id"}}}` for sequence waits.
fn wait_common(args: &Value, default_ms: u64) -> Result<Value, String> {
    Ok(json!({
        "timeout": parse_timeout(args, "timeout", default_ms)?,
        "target": { "window_id": arg_u64(args, "window_id")? },
    }))
}

fn input_route(args: &Value) -> Result<&str, String> {
    match arg_string(args, "route")?.as_deref() {
        None => Ok("application"),
        Some(route) if route.eq_ignore_ascii_case("application") => Ok("application"),
        Some(route) if route.eq_ignore_ascii_case("ui") => Ok("ui"),
        Some(other) => Err(format!("`route` must be \"application\" or \"ui\", not {other:?}")),
    }
}

/// Canonicalize Ctrl/Alt/Shift/Super modifiers (case-insensitive).
fn normalize_mods(args: &Value) -> Result<Vec<String>, String> {
    arg_string_list(args, "mods")?
        .iter()
        .map(|name| {
            ["Ctrl", "Alt", "Shift", "Super"]
                .iter()
                .find(|canonical| canonical.eq_ignore_ascii_case(name))
                .map(|canonical| canonical.to_string())
                .ok_or_else(|| format!("unknown modifier {name:?}: Ctrl, Alt, Shift, Super"))
        })
        .collect()
}

// --- Wire translation ---------------------------------------------------------

/// Whether `method` has a hand-written translator and schema.
fn is_known_method(method: &str) -> bool {
    matches!(
        method,
        "ping"
            | "reset_terminal"
            | "restart_terminal"
            | "create_window"
            | "close_window"
            | "config"
            | "get_config"
            | "typing"
            | "get_text"
            | "find_text"
            | "screenshot"
            | "key"
            | "paste"
            | "drop_file"
            | "mouse"
            | "resize"
            | "set_geometry"
            | "set_geometry_batch"
            | "set_visible"
            | "set_level"
            | "focus"
            | "signal"
            | "exec"
            | "list_windows"
            | "inspect"
            | "diagnose"
            | "vivid_sessions"
            | "vivid_surfaces"
            | "vivid_surface_status"
            | "vivid_tracks"
            | "vivid_track_status"
            | "vivid_scene_status"
            | "vivid_trace"
            | "get_grid"
            | "wait_text"
            | "wait_output"
            | "wait_screen_change"
            | "wait_screen_stable"
            | "wait_frame"
            | "wait_exit"
            | "wait_prompt"
            | "wait_command_finish"
            | "quit"
            | "wait_vivid_track"
            | "transcript"
    )
}

fn known_method(method: &str) -> &'static str {
    match method {
        "ping" => "ping",
        "reset_terminal" => "reset_terminal",
        "restart_terminal" => "restart_terminal",
        "create_window" => "create_window",
        "close_window" => "close_window",
        "config" => "config",
        "get_config" => "get_config",
        "typing" => "typing",
        "get_text" => "get_text",
        "find_text" => "find_text",
        "screenshot" => "screenshot",
        "key" => "key",
        "paste" => "paste",
        "drop_file" => "drop_file",
        "mouse" => "mouse",
        "resize" => "resize",
        "set_geometry" => "set_geometry",
        "set_geometry_batch" => "set_geometry_batch",
        "set_visible" => "set_visible",
        "set_level" => "set_level",
        "focus" => "focus",
        "signal" => "signal",
        "exec" => "exec",
        "list_windows" => "list_windows",
        "inspect" => "inspect",
        "diagnose" => "diagnose",
        "vivid_sessions" => "vivid_sessions",
        "vivid_surfaces" => "vivid_surfaces",
        "vivid_surface_status" => "vivid_surface_status",
        "vivid_tracks" => "vivid_tracks",
        "vivid_track_status" => "vivid_track_status",
        "vivid_scene_status" => "vivid_scene_status",
        "vivid_trace" => "vivid_trace",
        "get_grid" => "get_grid",
        "wait_text" => "wait_text",
        "wait_output" => "wait_output",
        "wait_screen_change" => "wait_screen_change",
        "wait_screen_stable" => "wait_screen_stable",
        "wait_frame" => "wait_frame",
        "wait_exit" => "wait_exit",
        "wait_prompt" => "wait_prompt",
        "wait_command_finish" => "wait_command_finish",
        "quit" => "quit",
        "wait_vivid_track" => "wait_vivid_track",
        "transcript" => "transcript",
        _ => unreachable!("checked by is_known_method"),
    }
}

/// Suggest the closest candidate to a misspelled name (edit distance <= 3).
pub fn suggest(name: &str, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .map(|candidate| (edit_distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= 3)
        .min()
        .map(|(_, candidate)| candidate.to_string())
}

fn edit_distance(left: &str, right: &str) -> usize {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for left_index in 1..=left.len() {
        let mut previous = row[0];
        row[0] = left_index;
        for right_index in 1..=right.len() {
            let insert = row[right_index] + 1;
            let delete = row[right_index - 1] + 1;
            let replace = previous + usize::from(left[left_index - 1] != right[right_index - 1]);
            previous = row[right_index];
            row[right_index] = insert.min(delete).min(replace);
        }
    }
    row[right.len()]
}

/// Reject unknown arguments so a typo like `windowd_id` fails loudly instead
/// of silently targeting the wrong window.
pub fn check_known_args(tool: &str, args: &Value, expected: &[&str]) -> Result<(), String> {
    let Some(object) = args.as_object() else {
        return Err("tool arguments must be a JSON object".to_owned());
    };
    for key in object.keys() {
        if !expected.contains(&key.as_str()) {
            let mut message = format!("unknown argument `{key}` for `{tool}`");
            if let Some(guess) = suggest(key, expected) {
                message.push_str(&format!(", did you mean `{guess}`?"));
            }
            return Err(message);
        }
    }
    Ok(())
}

/// Expected argument names for a known wire method (client-side extras like
/// `crop` included where they apply).
fn expected_args(method: &str) -> &'static [&'static str] {
    match method {
        "ping" | "list_windows" | "quit" => &[],
        "reset_terminal" | "restart_terminal" | "focus" | "inspect" => &["window_id"],
        "get_config" => &["window_id"],
        "get_text" => &["window_id", "rows"],
        "find_text" => &["pattern", "regex", "window_id", "max_matches"],
        "screenshot" => &["window_id", "crop", "max_dim", "format", "quality"],
        "typing" => &["window_id", "text"],
        "key" => &["window_id", "key", "mods", "repeat", "route"],
        "paste" => &["window_id", "text", "route"],
        "drop_file" => &["window_id", "path", "at", "type_path", "timeout"],
        "mouse" => &[
            "window_id",
            "action",
            "button",
            "cell_column",
            "cell_row",
            "x",
            "y",
            "relative_x",
            "relative_y",
            "mods",
            "route",
            "points",
            "duration",
            "wait_frame",
            "timeout",
            "vertical",
            "horizontal",
        ],
        "resize" => &["window_id", "columns", "rows", "width", "height"],
        "set_geometry" => &["window_id", "x", "y", "width", "height"],
        "set_geometry_batch" => &["items"],
        "set_visible" => &["window_id", "visible"],
        "set_level" => &["window_id", "level"],
        "signal" => &["window_id", "signal"],
        "exec" => &["window_id", "command", "timeout"],
        "create_window" => &[
            "program",
            "args",
            "command",
            "working_directory",
            "hold",
            "title",
            "class",
            "vivid_target",
            "ipc_window_id",
            "no_activate",
            "config_options",
        ],
        "close_window" => &["window_id", "force"],
        "config" => &["window_id", "options", "reset"],
        "diagnose" => &["window_id", "trace_limit"],
        "vivid_sessions" | "vivid_surfaces" | "vivid_tracks" => &["window_id"],
        "vivid_surface_status" => &["window_id", "session_id", "context_id", "surface_id"],
        "vivid_track_status" => {
            &["window_id", "session_id", "context_id", "surface_id", "track_id"]
        }
        "vivid_scene_status" => &["window_id", "session_id"],
        "vivid_trace" => &[
            "window_id",
            "after",
            "tail",
            "before",
            "around",
            "preceding",
            "following",
            "limit",
            "timeout",
            "session_id",
            "context_id",
            "surface_id",
            "track_id",
            "category",
            "recovery_only",
        ],
        "get_grid" => &["window_id", "start_line", "row_count", "since_screen"],
        "wait_text" => &["window_id", "text", "regex", "after_screen", "line", "rect", "timeout"],
        "wait_output" => &["window_id", "pattern", "regex", "base64", "after_offset", "timeout"],
        "wait_screen_change" => &["window_id", "after_screen", "timeout"],
        "wait_screen_stable" => &["window_id", "quiet", "after_screen", "timeout"],
        "wait_frame" => &["window_id", "after_frame", "timeout"],
        "wait_exit" | "wait_prompt" | "wait_command_finish" => &["window_id", "timeout"],
        "wait_vivid_track" => &[
            "window_id",
            "session_id",
            "context_id",
            "surface_id",
            "track_id",
            "channel_generation",
            "condition",
            "value",
            "timeout",
        ],
        "transcript" => &["window_id", "after_offset", "max_bytes", "raw"],
        _ => &[],
    }
}

/// Translate friendly flat tool arguments into exact wire params, validating
/// limits client-side so mistakes fail fast with clear messages.
pub fn translate(method: &str, args: &Value) -> Result<Value, String> {
    if !args.is_object() {
        return Err("tool arguments must be a JSON object".to_owned());
    }
    check_known_args(method, args, expected_args(method))?;
    match method {
        "ping" | "list_windows" | "quit" => Ok(json!({})),
        "reset_terminal" | "restart_terminal" | "focus" | "inspect" => {
            Ok(direct_target(arg_u64(args, "window_id")?))
        }
        "get_config" => Ok(json!({ "window_id": arg_i64(args, "window_id")? })),
        "get_text" => {
            let rows = arg_u16(args, "rows", 1, 1000)?.map(u64::from);
            Ok(json!({ "rows": rows, "window_id": arg_u64(args, "window_id")? }))
        }
        "find_text" => Ok(json!({
            "pattern": required_string(args, "pattern")?,
            "regex": arg_bool(args, "regex", false)?,
            "window_id": arg_u64(args, "window_id")?,
            "max_matches": arg_u16(args, "max_matches", 1, 100)?.unwrap_or(50),
        })),
        "screenshot" => Ok(direct_target(arg_u64(args, "window_id")?)),
        "typing" => {
            let text = required_string(args, "text")?;
            if text.len() > 1024 * 1024 {
                return Err("`text` exceeds the 1 MiB input limit".to_owned());
            }
            Ok(json!({ "text": text, "window_id": arg_u64(args, "window_id")? }))
        }
        "key" => translate_key(args),
        "paste" => {
            let text = required_string(args, "text")?;
            if text.len() > 1024 * 1024 {
                return Err("`text` exceeds the 1 MiB input limit".to_owned());
            }
            Ok(json!({
                "text": text,
                "route": input_route(args)?,
                "target": { "window_id": arg_u64(args, "window_id")? },
            }))
        }
        "drop_file" => translate_drop_file(args),
        "mouse" => translate_mouse(args),
        "resize" => {
            let columns = arg_u16(args, "columns", 2, u16::MAX)?;
            let rows = arg_u16(args, "rows", 1, u16::MAX)?;
            let width = arg_u32(args, "width")?;
            let height = arg_u32(args, "height")?;
            match (columns, rows, width, height) {
                (Some(_), Some(_), None, None) | (None, None, Some(_), Some(_)) => {}
                _ => {
                    return Err("`resize` needs `columns`+`rows` or `width`+`height`, never both"
                        .to_owned());
                }
            }
            if width.is_some_and(|value| value == 0) || height.is_some_and(|value| value == 0) {
                return Err("`width` and `height` must be at least 1".to_owned());
            }
            let mut params = json!({
                "columns": columns,
                "rows": rows,
                "width": width,
                "height": height,
            });
            params["target"] = json!({ "window_id": arg_u64(args, "window_id")? });
            Ok(params)
        }
        "set_geometry" => translate_set_geometry(args),
        "set_geometry_batch" => {
            let items = args
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| "missing required `items` array".to_owned())?;
            if items.is_empty() {
                return Err("`items` must not be empty".to_owned());
            }
            let translated =
                items.iter().map(translate_set_geometry).collect::<Result<Vec<_>, _>>()?;
            Ok(json!({ "items": translated }))
        }
        "set_visible" => Ok(json!({
            "visible": arg(args, "visible").and_then(Value::as_bool).ok_or_else(|| {
                "missing required `visible` boolean".to_owned()
            })?,
            "target": { "window_id": arg_u64(args, "window_id")? },
        })),
        "set_level" => {
            let level = required_string(args, "level")?;
            let level = ["normal", "always_on_top", "always_on_bottom"]
                .iter()
                .find(|canonical| canonical.eq_ignore_ascii_case(&level))
                .ok_or_else(|| {
                    "`level` must be \"normal\", \"always_on_top\", or \"always_on_bottom\""
                        .to_owned()
                })?;
            Ok(json!({
                "level": level,
                "target": { "window_id": arg_u64(args, "window_id")? },
            }))
        }
        "signal" => {
            let signal = required_string(args, "signal")?.to_ascii_uppercase();
            if !["INT", "TERM", "HUP", "QUIT", "TSTP", "CONT", "WINCH", "KILL", "STOP"]
                .contains(&signal.as_str())
            {
                return Err(format!("unknown signal {signal:?}"));
            }
            Ok(json!({
                "signal": signal,
                "target": { "window_id": arg_u64(args, "window_id")? },
            }))
        }
        "exec" => {
            let command = required_string(args, "command")?;
            if command.trim().is_empty() {
                return Err("`command` must not be empty".to_owned());
            }
            if command.len() > 64 * 1024 {
                return Err("`command` exceeds 64 KiB".to_owned());
            }
            Ok(json!({
                "command": command,
                "window_id": arg_u64(args, "window_id")?,
                "timeout": parse_timeout(args, "timeout", 60_000)?,
            }))
        }
        "create_window" => translate_create_window(args),
        "close_window" => Ok(json!({
            "window_id": arg_u64(args, "window_id")?,
            "force": arg_bool(args, "force", false)?,
        })),
        "config" => {
            let reset = arg_bool(args, "reset", false)?;
            let options = arg_string_list(args, "options")?;
            if reset == !options.is_empty() {
                return Err("`config` needs `options` or `reset: true`, not both".to_owned());
            }
            Ok(json!({
                "options": options,
                "window_id": arg_i64(args, "window_id")?,
                "reset": reset,
            }))
        }
        "diagnose" => Ok(json!({
            "window_id": arg_u64(args, "window_id")?,
            "trace_limit": arg_u16(args, "trace_limit", 1, 512)?.unwrap_or(128),
        })),
        "vivid_sessions" | "vivid_surfaces" | "vivid_tracks" => {
            Ok(direct_target(arg_u64(args, "window_id")?))
        }
        "vivid_surface_status" => Ok(json!({
            "window_id": arg_u64(args, "window_id")?,
            "session_id": required_u64(args, "session_id")?,
            "context_id": required_u64(args, "context_id")?,
            "surface_id": required_u64(args, "surface_id")?,
        })),
        "vivid_track_status" => Ok(json!({
            "window_id": arg_u64(args, "window_id")?,
            "session_id": required_u64(args, "session_id")?,
            "context_id": required_u64(args, "context_id")?,
            "surface_id": required_u64(args, "surface_id")?,
            "track_id": required_u64(args, "track_id")?,
        })),
        "vivid_scene_status" => Ok(json!({
            "window_id": arg_u64(args, "window_id")?,
            "session_id": required_u64(args, "session_id")?,
        })),
        "vivid_trace" => translate_vivid_trace(args),
        "get_grid" => {
            let start_line = arg_i32(args, "start_line")?;
            let row_count = arg_u16(args, "row_count", 1, 1000)?;
            let since_screen = arg_u64(args, "since_screen")?;
            if start_line.is_some() != row_count.is_some() {
                return Err("`start_line` and `row_count` must appear together".to_owned());
            }
            if since_screen.is_some() && start_line.is_some() {
                return Err("`since_screen` excludes an explicit `start_line` range".to_owned());
            }
            Ok(json!({
                "start_line": start_line,
                "row_count": row_count,
                "since_screen": since_screen,
                "target": { "window_id": arg_u64(args, "window_id")? },
            }))
        }
        "wait_text" => {
            let line = arg_i32(args, "line")?;
            let rect = match arg(args, "rect") {
                None => Value::Null,
                Some(Value::Object(_)) => {
                    let rect = arg(args, "rect").unwrap();
                    for key in ["col", "row", "width", "height"] {
                        if rect.get(key).and_then(Value::as_u64).is_none() {
                            return Err(format!("`rect.{key}` must be an integer"));
                        }
                    }
                    let width = rect["width"].as_u64().unwrap();
                    let height = rect["height"].as_u64().unwrap();
                    if !(1..=1000).contains(&width) || !(1..=1000).contains(&height) {
                        return Err("`rect` width and height must be 1 through 1000".to_owned());
                    }
                    json!({
                        "col": rect["col"],
                        "row": rect["row"],
                        "width": width,
                        "height": height,
                    })
                }
                Some(_) => return Err("`rect` must be {col,row,width,height}".to_owned()),
            };
            if line.is_some() && !rect.is_null() {
                return Err("`line` and `rect` are mutually exclusive".to_owned());
            }
            Ok(json!({
                "text": required_string(args, "text")?,
                "regex": arg_bool(args, "regex", false)?,
                "after_screen": arg_u64(args, "after_screen")?,
                "line": line,
                "rect": rect,
                "common": wait_common(args, 30_000)?,
            }))
        }
        "wait_output" => {
            let regex = arg_bool(args, "regex", false)?;
            let base64 = arg_bool(args, "base64", false)?;
            if regex && base64 {
                return Err("`regex` and `base64` are mutually exclusive".to_owned());
            }
            Ok(json!({
                "pattern": required_string(args, "pattern")?,
                "regex": regex,
                "base64": base64,
                "after_offset": arg_u64(args, "after_offset")?,
                "common": wait_common(args, 30_000)?,
            }))
        }
        "wait_screen_change" => Ok(json!({
            "after_screen": arg_u64(args, "after_screen")?,
            "common": wait_common(args, 30_000)?,
        })),
        "wait_screen_stable" => Ok(json!({
            "quiet": parse_timeout(args, "quiet", 250)?,
            "after_screen": arg_u64(args, "after_screen")?,
            "common": wait_common(args, 30_000)?,
        })),
        "wait_frame" => Ok(json!({
            "after_frame": arg_u64(args, "after_frame")?,
            "common": wait_common(args, 30_000)?,
        })),
        "wait_exit" | "wait_prompt" | "wait_command_finish" => Ok(json!({
            "timeout": parse_timeout(args, "timeout", 30_000)?,
            "target": { "window_id": arg_u64(args, "window_id")? },
        })),
        "wait_vivid_track" => translate_vivid_wait(args),
        "transcript" => {
            let max_bytes = arg_u64(args, "max_bytes")?.unwrap_or(65_536);
            if !(1..=1_048_576).contains(&max_bytes) {
                return Err("`max_bytes` must be 1 through 1048576".to_owned());
            }
            Ok(json!({
                "after_offset": arg_u64(args, "after_offset")?,
                "max_bytes": max_bytes,
                "raw": arg_bool(args, "raw", false)?,
                "target": { "window_id": arg_u64(args, "window_id")? },
            }))
        }
        _ => Err(format!("no translator for method `{method}`")),
    }
}

fn translate_key(args: &Value) -> Result<Value, String> {
    let key = canonical_key_name(&required_string(args, "key")?)?;
    let mods = normalize_mods(args)?;
    Ok(json!({
        "key": key,
        "mods": mods,
        "repeat": arg_u16(args, "repeat", 1, 1000)?.unwrap_or(1),
        "route": input_route(args)?,
        "target": { "window_id": arg_u64(args, "window_id")? },
    }))
}

/// Canonicalize one Unicode scalar or named key. Single characters pass through
/// untouched (case is significant: `a` is not `A`); named keys match
/// case-insensitively and canonicalize to the upstream spelling.
fn canonical_key_name(key: &str) -> Result<String, String> {
    if key.chars().count() == 1 {
        return Ok(key.to_owned());
    }
    const NAMED: &[&str] = &[
        "Enter",
        "Escape",
        "Tab",
        "Backspace",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "Home",
        "End",
        "Insert",
        "Delete",
        "PageUp",
        "PageDown",
        "KeypadDecimal",
        "KeypadDivide",
        "KeypadMultiply",
        "KeypadSubtract",
        "KeypadAdd",
        "KeypadEnter",
        "KeypadEqual",
    ];
    if let Some(canonical) = NAMED.iter().find(|name| name.eq_ignore_ascii_case(key)) {
        return Ok(canonical.to_string());
    }
    let lower = key.to_ascii_lowercase();
    if let Some(number) = lower.strip_prefix('f')
        && let Ok(number) = number.parse::<u32>()
        && (1..=35).contains(&number)
    {
        return Ok(format!("F{number}"));
    }
    if let Some(number) = lower.strip_prefix("keypad")
        && let Ok(number) = number.parse::<u32>()
        && number <= 9
    {
        return Ok(format!("Keypad{number}"));
    }
    Err("`key` must be one character or a named key (Enter, Escape, arrows, F1-F35, ...)"
        .to_owned())
}

fn translate_drop_file(args: &Value) -> Result<Value, String> {
    let path = required_string(args, "path")?;
    let absolute = std::path::absolute(&path).map_err(|error| error.to_string())?;
    let metadata = std::fs::symlink_metadata(&absolute)
        .map_err(|_| format!("`path` does not exist: {}", absolute.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("`path` must be a regular file".to_owned());
    }
    let mut params = json!({
        "path": absolute.to_string_lossy(),
        "type_path": arg_bool(args, "type_path", false)?,
        "timeout": parse_timeout(args, "timeout", 600_000)?,
        "target": { "window_id": arg_u64(args, "window_id")? },
    });
    if let Some(at) = arg(args, "at") {
        let (column, row) = match at {
            Value::Array(pair) if pair.len() == 2 => (pair[0].as_u64(), pair[1].as_u64()),
            Value::Object(_) => {
                (at.get("column").and_then(Value::as_u64), at.get("row").and_then(Value::as_u64))
            }
            Value::String(text) => match text.split_once(',') {
                Some((column, row)) => {
                    (column.trim().parse::<u64>().ok(), row.trim().parse::<u64>().ok())
                }
                None => (None, None),
            },
            _ => (None, None),
        };
        match (column, row) {
            (Some(column), Some(row))
                if column <= u64::from(u16::MAX) && row <= u64::from(u16::MAX) =>
            {
                params["at"] = json!([column, row]);
            }
            _ => {
                return Err("`at` must be [column, row], {column, row}, or \"col,row\"".to_owned());
            }
        }
    }
    Ok(params)
}

fn translate_mouse(args: &Value) -> Result<Value, String> {
    // Accept the CLI's dashed spelling alongside snake_case.
    let action = required_string(args, "action")?.replace('-', "_");
    let mods = normalize_mods(args)?;
    let route = input_route(args)?;
    let window_id = arg_u64(args, "window_id")?;
    let button_arg = arg_string(args, "button")?;
    let button = match button_arg.as_deref() {
        None => "left",
        Some(name @ ("left" | "middle" | "right")) => name,
        Some(other) => return Err(format!("unknown `button` {other:?}")),
    };
    let position = mouse_position(args, &mods, route, window_id)?;
    let wire = match action.as_str() {
        "move" => json!({ "move": position }),
        "click" => json!({ "click": { "button": button, "position": position } }),
        "double_click" => json!({ "double_click": { "button": button, "position": position } }),
        "down" => json!({ "down": { "button": button, "position": position } }),
        "up" => json!({ "up": { "button": button, "position": position } }),
        "drag" => json!({ "drag": { "button": button, "position": position } }),
        "scroll" => {
            let vertical = arg_f64(args, "vertical")?.unwrap_or(0.0);
            let horizontal = arg_f64(args, "horizontal")?.unwrap_or(0.0);
            if vertical == 0.0 && horizontal == 0.0 {
                return Err(
                    "mouse `scroll` needs a nonzero `vertical` or `horizontal` amount".to_owned()
                );
            }
            json!({
                "scroll": {
                    "vertical": vertical,
                    "horizontal": horizontal,
                    "position": position,
                }
            })
        }
        "path" => {
            let points = args
                .get("points")
                .and_then(Value::as_array)
                .ok_or_else(|| "mouse `path` needs `points`".to_owned())?;
            if !(2..=1000).contains(&points.len()) {
                return Err("mouse `path` needs 2 through 1000 points".to_owned());
            }
            let mut wire_points = Vec::with_capacity(points.len());
            for point in points {
                let (x, y) = match point {
                    Value::Array(pair) if pair.len() == 2 => (pair[0].as_f64(), pair[1].as_f64()),
                    Value::Object(_) => (
                        point.get("x").and_then(Value::as_f64),
                        point.get("y").and_then(Value::as_f64),
                    ),
                    _ => (None, None),
                };
                match (x, y) {
                    (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
                        wire_points.push(json!({ "x": x, "y": y }));
                    }
                    _ => return Err("mouse path points must be finite [x, y] pairs".to_owned()),
                }
            }
            let duration = match arg(args, "duration") {
                None => Value::Null,
                Some(Value::Number(_)) | Some(Value::String(_)) => {
                    let ms = parse_timeout(args, "duration", 0)?;
                    if !(1..=30_000).contains(&ms) {
                        return Err("mouse `duration` must be 1ms through 30s".to_owned());
                    }
                    json!(ms)
                }
                Some(_) => return Err("mouse `duration` must be milliseconds".to_owned()),
            };
            json!({
                "path": {
                    "points": wire_points,
                    "button": button,
                    "mods": mods,
                    "route": route,
                    "duration": duration,
                    "wait_frame": arg_bool(args, "wait_frame", false)?,
                    "timeout": parse_timeout(args, "timeout", 30_000)?,
                    "target": { "window_id": window_id },
                }
            })
        }
        other => {
            return Err(format!(
                "unknown mouse `action` {other:?}: move, click, double_click, down, up, drag, path, scroll"
            ));
        }
    };
    Ok(json!({ "action": wire }))
}

/// One position: exactly one complete cell, pixel, or relative pair.
fn mouse_position(
    args: &Value,
    mods: &[String],
    route: &str,
    window_id: Option<u64>,
) -> Result<Value, String> {
    let cell_column = arg_u32(args, "cell_column")?;
    let cell_row = arg_u32(args, "cell_row")?;
    let x = arg_f64(args, "x")?;
    let y = arg_f64(args, "y")?;
    let relative_x = arg_f64(args, "relative_x")?;
    let relative_y = arg_f64(args, "relative_y")?;
    let cell = cell_column.is_some() && cell_row.is_some();
    let pixel = x.is_some() && y.is_some();
    let relative = relative_x.is_some() && relative_y.is_some();
    if usize::from(cell) + usize::from(pixel) + usize::from(relative) != 1 {
        return Err(
            "mouse needs exactly one complete cell, pixel, or relative coordinate pair".to_owned()
        );
    }
    if relative
        && [relative_x, relative_y].into_iter().flatten().any(|value| !(0.0..=1.0).contains(&value))
    {
        return Err("relative mouse coordinates must be 0 through 1".to_owned());
    }
    Ok(json!({
        "cell_column": cell_column,
        "cell_row": cell_row,
        "x": x,
        "y": y,
        "relative_x": relative_x,
        "relative_y": relative_y,
        "mods": mods,
        "route": route,
        "target": { "window_id": window_id },
    }))
}

fn translate_set_geometry(args: &Value) -> Result<Value, String> {
    let x = arg_i32(args, "x")?;
    let y = arg_i32(args, "y")?;
    let width = arg_u32(args, "width")?;
    let height = arg_u32(args, "height")?;
    if x.is_none() && width.is_none() {
        return Err("`set_geometry` needs `x`/`y`, `width`/`height`, or both".to_owned());
    }
    if x.is_some() != y.is_some() || width.is_some() != height.is_some() {
        return Err("`set_geometry` coordinate and size pairs must be complete".to_owned());
    }
    if width.is_some_and(|value| value == 0) || height.is_some_and(|value| value == 0) {
        return Err("`width` and `height` must be at least 1".to_owned());
    }
    Ok(json!({
        "x": x,
        "y": y,
        "width": width,
        "height": height,
        "target": { "window_id": arg_u64(args, "window_id")? },
    }))
}

fn translate_create_window(args: &Value) -> Result<Value, String> {
    let vivid_target = arg_string(args, "vivid_target")?.unwrap_or_else(|| "terminal".to_owned());
    if vivid_target != "terminal" && vivid_target != "desktop" {
        return Err("`vivid_target` must be \"terminal\" or \"desktop\"".to_owned());
    }
    let mut command: Vec<String> = Vec::new();
    if let Some(program) = arg_string(args, "program")? {
        if arg(args, "command").is_some() {
            return Err("use `program`+`args` or `command`, not both".to_owned());
        }
        command.push(program);
        command.extend(arg_string_list(args, "args")?);
    } else if let Some(array) = arg(args, "command") {
        let items = array.as_array().ok_or_else(|| {
            "`command` must be an array like [\"./run.sh\", \"--fast\"]".to_owned()
        })?;
        for item in items {
            command.push(
                item.as_str()
                    .ok_or_else(|| "`command` must be an array of strings".to_owned())?
                    .to_owned(),
            );
        }
        if command.is_empty() {
            return Err("`command` must name a program".to_owned());
        }
    } else if arg(args, "args").is_some() {
        return Err("`args` needs a `program`".to_owned());
    }
    let class = match arg(args, "class") {
        None => Value::Null,
        Some(Value::String(text)) => {
            let (general, instance) = text
                .split_once(',')
                .map_or((text.as_str(), text.as_str()), |(general, instance)| {
                    (general.trim(), instance.trim())
                });
            json!({ "general": general, "instance": instance })
        }
        Some(Value::Object(_)) => {
            let class = arg(args, "class").unwrap();
            json!({
                "general": class.get("general").and_then(Value::as_str).unwrap_or("Vivido"),
                "instance": class.get("instance").and_then(Value::as_str).unwrap_or("Vivido"),
            })
        }
        Some(_) => {
            return Err("`class` must be \"general,instance\" or {general, instance}".to_owned());
        }
    };
    Ok(json!({
        "vivid_target": vivid_target,
        "ipc_window_id": arg_u64(args, "ipc_window_id")?,
        "no_activate": arg_bool(args, "no_activate", false)?,
        "terminal_options": {
            "working_directory": arg_string(args, "working_directory")?,
            "hold": arg_bool(args, "hold", false)?,
            "command": command,
        },
        "window_identity": {
            "title": arg_string(args, "title")?,
            "class": class,
        },
        "activation_token": Value::Null,
        "option": arg_string_list(args, "config_options")?,
    }))
}

fn translate_vivid_trace(args: &Value) -> Result<Value, String> {
    let after = arg_u64(args, "after")?;
    let tail = arg_bool(args, "tail", false)?;
    let before = arg_u64(args, "before")?;
    let around = arg_u64(args, "around")?;
    let selectors = usize::from(after.is_some())
        + usize::from(tail)
        + usize::from(before.is_some())
        + usize::from(around.is_some());
    if selectors > 1 {
        return Err("`after`, `tail`, `before`, and `around` are mutually exclusive".to_owned());
    }
    let mut params = json!({
        "window_id": arg_u64(args, "window_id")?,
        "after": after,
        "tail": tail,
        "before": before,
        "limit": arg_u16(args, "limit", 1, 512)?.unwrap_or(128),
        "timeout": parse_timeout(args, "timeout", 30_000)?,
        "follow": false,
        "session_id": arg_u64(args, "session_id")?,
        "context_id": arg_u64(args, "context_id")?,
        "surface_id": arg_u64(args, "surface_id")?,
        "track_id": arg_u64(args, "track_id")?,
        "category": arg_string(args, "category")?,
        "recovery_only": arg_bool(args, "recovery_only", false)?,
    });
    if params["category"].is_string() {
        let category = params["category"].as_str().unwrap();
        if !["connection", "lifecycle", "flow", "playback", "recovery", "decode", "render"]
            .contains(&category)
        {
            return Err(format!("unknown trace `category` {category:?}"));
        }
    }
    if let Some(sequence) = around {
        let preceding = arg_u16(args, "preceding", 0, 512)?.unwrap_or(64);
        let following = arg_u16(args, "following", 0, 512)?.unwrap_or(64);
        if u32::from(preceding) + u32::from(following) == 0 {
            return Err("`around` needs a nonzero preceding/following count".to_owned());
        }
        params["around"] =
            json!({ "sequence": sequence, "preceding": preceding, "following": following });
    }
    Ok(params)
}

fn translate_vivid_wait(args: &Value) -> Result<Value, String> {
    let condition = required_string(args, "condition")?.replace('_', "-");
    let wire = match condition.as_str() {
        "revision-after" => 1,
        "milestones" => 2,
        "presentation-after" => 3,
        "pts-after" => 4,
        "clock-started" => 5,
        "buffered-ended" => 6,
        "channel-accepted" => 7,
        "channel-detached" => 8,
        "track-lost" => 9,
        _ => return Err(format!("unknown Vivid wait `condition` {condition:?}")),
    };
    let value = arg_u64(args, "value")?;
    if wire <= 4 && value.is_none() {
        return Err(format!("condition `{condition}` requires `value`"));
    }
    if wire >= 5 && value.is_some() {
        return Err(format!("condition `{condition}` does not accept `value`"));
    }
    Ok(json!({
        "window_id": arg_u64(args, "window_id")?,
        "session_id": required_u64(args, "session_id")?,
        "context_id": required_u64(args, "context_id")?,
        "surface_id": required_u64(args, "surface_id")?,
        "track_id": required_u64(args, "track_id")?,
        "channel_generation": required_u64(args, "channel_generation")?,
        "condition": wire,
        "value": value,
        "timeout": parse_timeout(args, "timeout", 30_000)?,
    }))
}

// --- Descriptions and schemas -------------------------------------------------

/// One-paragraph description for a known method.
fn description_for(method: &str) -> &'static str {
    match method {
        "ping" => "Liveness check. Returns {pong:true}.",
        "reset_terminal" => {
            "Reset one window: discard partial parser state, return to the primary \
            screen, clear client-controlled input modes and the Vivid scene, resume a quarantined \
            PTY. Scrollback and the window ID survive. (Process tier: needs `--allow process`.)"
        }
        "restart_terminal" => {
            "Replace one window's PTY and Vivid service from its retained launch \
            options. The window ID stays stable; a failed replacement leaves the quarantined pane \
            for another attempt. (Process tier: needs `--allow process`.)"
        }
        "create_window" => {
            "Create a window (a tab on headed Linux/Windows) running `program` and \
            return its window ID. IDs are small and monotonic but opaque: discover, never predict. \
            (Process tier: needs `--allow process`.)"
        }
        "close_window" => {
            "Close one window without stopping the instance. Closing the last window \
            of a headed instance exits it; `force` kills the child process group first. \
            (Process tier: needs `--allow process`.)"
        }
        "config" => {
            "Update Vivido configuration (`options` like 'cursor.style=\"Beam\"'), or reset \
            runtime changes with `reset: true`. `window_id: -1` applies globally. \
            (Process tier: needs `--allow process`.)"
        }
        "get_config" => {
            "Read runtime Vivido configuration. `window_id: -1` reads the global config."
        }
        "typing" => {
            "Write literal UTF-8 text to the terminal PTY (no paste handling, no appended \
            Enter; use `key` for Enter). Success means the bytes reached the PTY, not that the \
            application consumed them. (Input tier: needs `--allow input`.)"
        }
        "get_text" => {
            "Read terminal text: the visible viewport, or the newest `rows` physical rows \
            (1-1000) including scrollback. Excludes styling, cursor, and media."
        }
        "find_text" => {
            "Locate `pattern` (or regex) on viewports and return matches with cell and \
            pixel rectangles. Omit `window_id` to search every window."
        }
        "screenshot" => {
            "Capture the last presented client-area frame as a PNG you can see \
            directly, with capture metadata. Supports cell-region `crop` and `max_dim` downscaling. \
            The temp file is deleted after reading; prefer `capture` after acting."
        }
        "key" => {
            "Send one mode-aware keypress: a single character or a named key (Enter, Escape, \
            Tab, arrows, F1-F35, Keypad*), with Ctrl/Alt/Shift/Super modifiers and repeat. This is \
            the only correct way to send Enter or Ctrl-C. (Input tier: needs `--allow input`.)"
        }
        "paste" => {
            "Paste text with bracketed-paste filtering and newline normalization (application \
            route), or into local UI state such as search (ui route). (Input tier: needs \
            `--allow input`.)"
        }
        "drop_file" => {
            "Copy a local regular file to the receiver bound to a window, exactly as a \
            drag would, and wait for its result. Nothing is typed unless `type_path` is true. \
            (Input tier: needs `--allow input`.)"
        }
        "mouse" => {
            "Send a mouse action (move, click, double_click, down, up, drag, path, scroll) \
            with exactly one cell, pixel, or relative coordinate pair. `path` is one bounded \
            press/move/release gesture of 2-1000 physical-pixel points. (Input tier: needs \
            `--allow input`.)"
        }
        "resize" => {
            "Resize to exact grid (`columns`+`rows`) or pixel (`width`+`height`) dimensions, \
            waiting for OS and PTY sizes to agree. (Process tier: needs `--allow process`.)"
        }
        "set_geometry" => {
            "Move the outer frame (`x`+`y`) and/or resize the client area \
            (`width`+`height`) in physical pixels. Returns immediately; subscribe for confirmation. \
            Headless positioning is unsupported. (Process tier: needs `--allow process`.)"
        }
        "set_geometry_batch" => {
            "Apply several per-window geometry updates in one request, in order. \
            (Process tier: needs `--allow process`.)"
        }
        "set_visible" => {
            "Map or unmap a window without destroying it. Mapping deliberately does not \
            take the keyboard. (Process tier: needs `--allow process`.)"
        }
        "set_level" => {
            "Stack a window normally, always on top, or always on bottom. \
            (Process tier: needs `--allow process`.)"
        }
        "focus" => {
            "Request real OS activation. Succeeds only after an actual focused event, else \
            `focus_denied` after two seconds; never succeeds headless. (Process tier: needs \
            `--allow process`.)"
        }
        "signal" => {
            "Send exactly the named signal (INT, TERM, HUP, QUIT, TSTP, CONT, WINCH, KILL, \
            STOP) to the foreground process group. (Process tier: needs `--allow process`.)"
        }
        "exec" => {
            "Run one shell command in a window's working directory and report its result. \
            (Process tier: needs `--allow process`.)"
        }
        "list_windows" => {
            "List every window in creation order: IDs, titles, focus/visibility, \
            grid/pixel sizes, process state, and current screen/frame/output sequences. Start here."
        }
        "inspect" => {
            "Inspect one window: list entry plus cell metrics, scale, scrollback, modes, \
            cursor, selection, shell PID, foreground process, working directory, echo state, exit \
            status, and sequences. Never returns arguments, environment, or secrets."
        }
        "diagnose" => {
            "Capture one correlated metadata-only snapshot: window, renderer, presenter, \
            tracks, flow, connection health, and the newest trace events. Nothing waits; async \
            metrics carry an age."
        }
        "vivid_sessions" => "Enumerate live Vivid sessions (complete owners).",
        "vivid_surfaces" => "Enumerate stable Vivid surfaces.",
        "vivid_surface_status" => "Inspect one fully addressed Vivid surface.",
        "vivid_tracks" => "Enumerate immutable Vivid tracks.",
        "vivid_track_status" => {
            "Inspect one fully addressed Vivid track: revision, milestones, \
            progress, flow counters, playback state."
        }
        "vivid_scene_status" => "Inspect one owned Vivid scene by session.",
        "vivid_trace" => {
            "Query the bounded Vivid metadata journal (no credentials, media, or frame \
            hashes). Selectors (`after`, `tail`, `before`, `around`) are mutually exclusive; poll \
            with `after` instead of following."
        }
        "get_grid" => {
            "Read a structured grid snapshot: positioned cells, widths, styles, wrap flags, \
            cursor, selection, hyperlinks. Prefer over screenshots for content questions. `start_line` \
            + `row_count` address scrollback; `since_screen` returns changed viewport rows."
        }
        "wait_text" => {
            "Wait for visible text (literal or regex, whole viewport or `line`/`rect` \
            scope). Pass `after_screen` from a prior `inspect` so the wait cannot match stale state."
        }
        "wait_output" => {
            "Wait for PTY output bytes (literal, regex, or base64) that may scroll past. \
            Without `after_offset` only future bytes match; an evicted offset fails `sequence_gap`."
        }
        "wait_screen_change" => {
            "Wait for the next semantic screen change after `after_screen` \
            (omit to wait for the next change after registration)."
        }
        "wait_screen_stable" => {
            "Wait until no semantic screen change for `quiet` (default 250ms); \
            with `after_screen`, first require at least one newer screen. The TUI-settled wait."
        }
        "wait_frame" => {
            "Wait for the next presented frame after `after_frame` (omit for the next frame)."
        }
        "wait_exit" => {
            "Wait for the terminal process to exit. Held windows with retained status \
            return immediately."
        }
        "wait_prompt" => {
            "Wait until the shell sits at a prompt (OSC 133 integration required; \
            otherwise never resolves). Returns readiness plus the command generation."
        }
        "wait_command_finish" => {
            "Wait for the next command finish after registration and report its \
            exit code. Register BEFORE sending the command; already-finished commands never resolve \
            a later wait."
        }
        "quit" => {
            "Shut the whole instance down (graceful headless stop; the daemon removes its own \
            endpoint and registry). (Process tier: needs `--allow process`.)"
        }
        "wait_vivid_track" => {
            "Wait for one Vivid track condition within a channel generation \
            (revision-after, milestones, presentation-after, pts-after need `value`; the five \
            lifecycle conditions forbid it)."
        }
        "transcript" => {
            "Read the 1 MiB sanitized byte-exact PTY ring (Vivid markers and Kitty \
            graphics removed). JSON gives offsets plus base64; `raw: true` decodes to text. An \
            evicted offset fails `sequence_gap`."
        }
        _ => "Vivido automation method.",
    }
}

const CAPTURE_DESCRIPTION: &str = "Settle and capture in one operation, returning the screenshot \
    as an image you can see directly plus capture metadata. Optionally require a newer frame, wait \
    for screen stability, and reveal a hidden pane first (Vivida hosts only). Supports cell-region \
    `crop` and `max_dim` downscaling; the temp file is deleted after reading. After acting, prefer \
    `capture` over bare `screenshot`.";

const DRAIN_DESCRIPTION: &str = "Return Vivido automation events since `cursor` (omit for new \
    events from now), keeping one server-side subscription open across calls. The reply carries the \
    events, the next cursor, and whether an `overflow` gap collapsed dropped detail — on overflow, \
    recover with `inspect` plus `get_grid`. Event data never arrives as MCP notifications; poll this.";

const SUBSCRIBE_DESCRIPTION: &str = "One-shot event collection: subscribe, gather matching events \
    for up to `timeout`, then unsubscribe. For continuous polling across turns use `drain_events` \
    instead, which keeps one subscription open and resumes from a cursor.";

fn headless_definitions() -> [(&'static str, &'static str, Value); 4] {
    [
        (
            "headless_list",
            "List every registered session (headless or headed, Vivido or Vivida) with \
             liveness: names for `headless_attach` and `headless_stop`, without ever \
             revealing endpoint paths.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        ),
        (
            "headless_start",
            "Start an ephemeral headless Vivido session (real offscreen renderer, real \
             screenshots) and attach this server to it. Returns the session name plus its windows; \
             the endpoint path is never revealed. Prefer ephemeral sessions for agent work so input \
             cannot reach your visible workspace, and `headless_stop` afterwards. (Process tier: \
             needs `--allow process`.)",
            json!({
                "type": "object",
                "properties": {
                    "session": {
                        "type": "string",
                        "description": "Session name (1-64 ASCII letters, digits, '.', '-', '_'; must not start with '.'). A unique name is generated when omitted.",
                    },
                    "size": {
                        "type": "string",
                        "description": "Initial geometry: COLUMNSxLINES (through real font metrics) or WIDTHxHEIGHTpx (render surface directly; best for screenshots). Default \"1280x720px\".",
                    },
                    "program": {
                        "type": "string",
                        "description": "Program for the first window instead of the default shell.",
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments for `program`.",
                    },
                    "ephemeral": {
                        "type": "boolean",
                        "description": "Tear the session down when its clients disconnect (default true). Ephemeral sessions never leak; keep this on unless the session must outlive this server.",
                    },
                },
                "additionalProperties": false,
            }),
        ),
        (
            "headless_attach",
            "Attach this server to another live session by name (headless or headed, Vivido or \
             Vivida) and report its handshake summary. Later tool calls reach the new target until \
             the next attach. (Process tier: needs `--allow process`.)",
            json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Session or automation name to attach to.",
                    },
                },
                "required": ["target"],
                "additionalProperties": false,
            }),
        ),
        (
            "headless_stop",
            "Stop a headless session: graceful `quit` first (the daemon removes its own endpoint, \
             registry, and log), escalating to `kill-session` when `force` is true or the graceful \
             stop fails. Omit `target` for the attached session. (Process tier: needs \
             `--allow process`.)",
            json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Session to stop. Defaults to the attached session.",
                    },
                    "force": {
                        "type": "boolean",
                        "description": "Skip the graceful stop and kill the session (default false).",
                    },
                },
                "additionalProperties": false,
            }),
        ),
    ]
}

fn window_id_property() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "description": "Window ID. Omit for the focused window; pass explicitly in headless sessions with more than one window.",
    })
}

fn timeout_property(default: &str) -> Value {
    json!({
        "type": ["integer", "string"],
        "description": format!(
            "Maximum wait as milliseconds or a duration like \"500ms\", \"30s\", \"5m\" (1ms-24h). Default \"{default}\"."
        ),
    })
}

fn route_property() -> Value {
    json!({
        "type": "string",
        "enum": ["application", "ui"],
        "description": "\"application\" (default) bypasses Vivido bindings and encodes for the terminal app; \"ui\" runs through Vivido's normal input processor (bindings, search, palette).",
    })
}

fn mods_property() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string", "enum": ["Ctrl", "Alt", "Shift", "Super"] },
        "description": "Modifiers held for this input.",
    })
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn screenshot_schema() -> Value {
    object(
        json!({
            "window_id": window_id_property(),
            "crop": {
                "type": "object",
                "properties": {
                    "col": { "type": "integer", "minimum": 0 },
                    "row": { "type": "integer", "minimum": 0 },
                    "cols": { "type": "integer", "minimum": 1 },
                    "rows": { "type": "integer", "minimum": 1 },
                },
                "required": ["col", "row", "cols", "rows"],
                "additionalProperties": false,
                "description": "Cell region to crop to (zero-based col/row plus size), mapped to pixels with the capture's own cell metrics and padding.",
            },
            "max_dim": {
                "type": "integer",
                "minimum": 64,
                "maximum": 4096,
                "description": "Downscale so the longest edge is at most this many pixels (default 1280). Larger screenshots cost context; crop first when only one region matters.",
            },
            "format": {
                "type": "string",
                "enum": ["png", "jpeg"],
                "description": "Output encoding: lossless png (default, best for text) or smaller lossy jpeg (best for photographic media).",
            },
            "quality": {
                "type": "integer",
                "minimum": 1,
                "maximum": 100,
                "description": "JPEG quality, 1-100 (default 85; only applies to format jpeg).",
            },
        }),
        &[],
    )
}

fn capture_schema() -> Value {
    object(
        json!({
            "window_id": window_id_property(),
            "activate": {
                "type": "boolean",
                "description": "Reveal a hidden pane first via the host's advertised activation method (Vivida hosts only; default false).",
            },
            "after_frame": {
                "type": "integer",
                "minimum": 1,
                "description": "Require a frame newer than this sequence before capturing (take it from a prior inspect).",
            },
            "stable": {
                "type": "boolean",
                "description": "Wait for screen stability before capturing (default false).",
            },
            "quiet": timeout_property("250ms"),
            "timeout": timeout_property("30s"),
            "crop": screenshot_schema()["properties"]["crop"].clone(),
            "max_dim": screenshot_schema()["properties"]["max_dim"].clone(),
            "format": screenshot_schema()["properties"]["format"].clone(),
            "quality": screenshot_schema()["properties"]["quality"].clone(),
        }),
        &[],
    )
}

fn drain_schema() -> Value {
    object(
        json!({
            "cursor": {
                "type": "integer",
                "minimum": 0,
                "description": "Event sequence to resume after (the previous reply's `cursor`). Omit on the first call to subscribe from now.",
            },
            "window_id": {
                "type": "integer",
                "minimum": 1,
                "description": "Subscribe to one window. Omit with `all: true` for every window plus process lifecycle events.",
            },
            "all": {
                "type": "boolean",
                "description": "Receive every window plus process lifecycle events (default false).",
            },
            "events": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Event kinds to keep (see hello `event_kinds`); omit for all kinds.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 1000,
                "description": "Maximum events to return (default 100).",
            },
            "timeout": {
                "type": ["integer", "string"],
                "description": "How long to wait for events before returning what exists (default \"2s\", max \"60s\").",
            },
        }),
        &[],
    )
}

fn subscribe_schema() -> Value {
    object(
        json!({
            "window_id": window_id_property(),
            "all": {
                "type": "boolean",
                "description": "Receive every window plus process lifecycle events (default false).",
            },
            "events": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Event kinds to keep; omit for all kinds.",
            },
            "since_event": {
                "type": "integer",
                "minimum": 0,
                "description": "Replay retained matching events newer than this global sequence before live delivery.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 1000,
                "description": "Maximum events to collect (default 100).",
            },
            "timeout": {
                "type": ["integer", "string"],
                "description": "How long to collect before returning (default \"5s\", max \"60s\").",
            },
        }),
        &[],
    )
}

/// JSON Schema for a known wire method's friendly arguments.
fn schema_for(method: &str) -> Value {
    match method {
        "ping" | "list_windows" | "quit" => object(json!({}), &[]),
        "reset_terminal" | "restart_terminal" | "focus" | "inspect" => {
            object(json!({ "window_id": window_id_property() }), &[])
        }
        "get_config" => object(
            json!({
                "window_id": {
                    "type": "integer",
                    "description": "Window ID, or -1 for the global config.",
                },
            }),
            &[],
        ),
        "get_text" => object(
            json!({
                "window_id": window_id_property(),
                "rows": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "description": "Newest physical rows including scrollback; omit for the visible viewport at its scroll position.",
                },
            }),
            &[],
        ),
        "find_text" => object(
            json!({
                "pattern": { "type": "string", "description": "Literal text (or regex with `regex`)." },
                "regex": { "type": "boolean", "description": "Treat `pattern` as a regex (default false)." },
                "window_id": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Window to search; omit to search every window.",
                },
                "max_matches": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Maximum matches per window (default 50).",
                },
            }),
            &["pattern"],
        ),
        "typing" => object(
            json!({
                "window_id": window_id_property(),
                "text": { "type": "string", "description": "Literal UTF-8 text (max 1 MiB)." },
            }),
            &["text"],
        ),
        "key" => object(
            json!({
                "window_id": window_id_property(),
                "key": { "type": "string", "description": "One character or a named key: Enter, Escape, Tab, Backspace, arrows, Home/End, Insert/Delete, PageUp/PageDown, F1-F35, Keypad0-9 and KeypadDecimal/Divide/Multiply/Subtract/Add/Enter/Equal." },
                "mods": mods_property(),
                "repeat": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Press count (default 1)." },
                "route": route_property(),
            }),
            &["key"],
        ),
        "paste" => object(
            json!({
                "window_id": window_id_property(),
                "text": { "type": "string", "description": "Text to paste (max 1 MiB)." },
                "route": route_property(),
            }),
            &["text"],
        ),
        "drop_file" => object(
            json!({
                "window_id": window_id_property(),
                "path": { "type": "string", "description": "Local regular file to copy; Vivido opens it itself." },
                "at": {
                    "type": "array",
                    "items": { "type": "integer", "minimum": 0 },
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "Drop onto the surface at [column, row]; omit for the window-wide binding.",
                },
                "type_path": { "type": "boolean", "description": "Type the committed remote path afterwards (default false; lands in whatever has focus)." },
                "timeout": timeout_property("10m"),
            }),
            &["path"],
        ),
        "mouse" => object(
            json!({
                "window_id": window_id_property(),
                "action": { "type": "string", "enum": ["move", "click", "double_click", "down", "up", "drag", "path", "scroll"], "description": "The gesture to perform." },
                "button": { "type": "string", "enum": ["left", "middle", "right"], "description": "Button for click/down/up/drag/path (default left)." },
                "cell_column": { "type": "integer", "minimum": 0, "description": "Zero-based cell column (with cell_row)." },
                "cell_row": { "type": "integer", "minimum": 0, "description": "Zero-based cell row (with cell_column)." },
                "x": { "type": "number", "description": "Physical-pixel X in the client area (with y)." },
                "y": { "type": "number", "description": "Physical-pixel Y in the client area (with x)." },
                "relative_x": { "type": "number", "minimum": 0.0, "maximum": 1.0, "description": "Fraction across the client area, 0-1 (with relative_y)." },
                "relative_y": { "type": "number", "minimum": 0.0, "maximum": 1.0, "description": "Fraction down the client area, 0-1 (with relative_x)." },
                "mods": mods_property(),
                "route": route_property(),
                "points": { "type": "array", "minItems": 2, "maxItems": 1000, "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "description": "Physical-pixel [x, y] points for `path` (2-1000)." },
                "duration": { "type": ["integer", "string"], "description": "Pace a `path` gesture across this long (1ms-30s); omit for one-write delivery." },
                "wait_frame": { "type": "boolean", "description": "Delay `path` success until a newer frame presents (default false)." },
                "timeout": timeout_property("30s"),
                "vertical": { "type": "number", "description": "`scroll` vertical amount (positive scrolls up)." },
                "horizontal": { "type": "number", "description": "`scroll` horizontal amount (positive scrolls left)." },
            }),
            &["action"],
        ),
        "resize" => object(
            json!({
                "window_id": window_id_property(),
                "columns": { "type": "integer", "minimum": 2, "description": "Exact grid columns (with rows)." },
                "rows": { "type": "integer", "minimum": 1, "description": "Exact grid rows (with columns)." },
                "width": { "type": "integer", "minimum": 1, "description": "Exact client width in pixels (with height)." },
                "height": { "type": "integer", "minimum": 1, "description": "Exact client height in pixels (with width)." },
            }),
            &[],
        ),
        "set_geometry" => object(
            json!({
                "window_id": window_id_property(),
                "x": { "type": "integer", "description": "Outer frame X in physical pixels (with y)." },
                "y": { "type": "integer", "description": "Outer frame Y in physical pixels (with x)." },
                "width": { "type": "integer", "minimum": 1, "description": "Client width in pixels (with height)." },
                "height": { "type": "integer", "minimum": 1, "description": "Client height in pixels (with width)." },
            }),
            &[],
        ),
        "set_geometry_batch" => object(
            json!({
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "window_id": window_id_property(),
                            "x": { "type": "integer" },
                            "y": { "type": "integer" },
                            "width": { "type": "integer", "minimum": 1 },
                            "height": { "type": "integer", "minimum": 1 },
                        },
                        "additionalProperties": false,
                    },
                    "description": "Per-window geometry updates, applied in order.",
                },
            }),
            &["items"],
        ),
        "set_visible" => object(
            json!({
                "window_id": window_id_property(),
                "visible": { "type": "boolean", "description": "Map (true) or unmap (false) the window." },
            }),
            &["visible"],
        ),
        "set_level" => object(
            json!({
                "window_id": window_id_property(),
                "level": { "type": "string", "enum": ["normal", "always_on_top", "always_on_bottom"] },
            }),
            &["level"],
        ),
        "signal" => object(
            json!({
                "window_id": window_id_property(),
                "signal": { "type": "string", "enum": ["INT", "TERM", "HUP", "QUIT", "TSTP", "CONT", "WINCH", "KILL", "STOP"], "description": "Signal without the SIG prefix." },
            }),
            &["signal"],
        ),
        "exec" => object(
            json!({
                "window_id": window_id_property(),
                "command": { "type": "string", "description": "Shell command (max 64 KiB)." },
                "timeout": timeout_property("60s"),
            }),
            &["command"],
        ),
        "create_window" => object(
            json!({
                "program": { "type": "string", "description": "Program for the new window instead of the default shell." },
                "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments for `program`." },
                "command": { "type": "array", "items": { "type": "string" }, "description": "Program plus arguments in one array (alternative to `program`+`args`)." },
                "working_directory": { "type": "string", "description": "Starting directory for the shell." },
                "hold": { "type": "boolean", "description": "Remain open after the child exits (default false)." },
                "title": { "type": "string", "description": "Window title." },
                "class": { "type": "string", "description": "Wayland app_id as \"general,instance\"." },
                "vivid_target": { "type": "string", "enum": ["terminal", "desktop"], "description": "\"desktop\" makes a grid-less desktop window with no shell (default terminal)." },
                "ipc_window_id": { "type": "integer", "minimum": 1, "description": "Stable ID to assign; omit for automatic assignment." },
                "no_activate": { "type": "boolean", "description": "Map without taking keyboard focus (default false)." },
                "config_options": { "type": "array", "items": { "type": "string" }, "description": "Config overrides like 'cursor.style=\"Beam\"'." },
            }),
            &[],
        ),
        "close_window" => object(
            json!({
                "window_id": window_id_property(),
                "force": { "type": "boolean", "description": "Kill the child process group first (default false)." },
            }),
            &[],
        ),
        "config" => object(
            json!({
                "window_id": { "type": "integer", "description": "Window ID, or -1 for all windows." },
                "options": { "type": "array", "items": { "type": "string" }, "description": "Options like 'cursor.style=\"Beam\"' (or `reset: true`)." },
                "reset": { "type": "boolean", "description": "Clear all runtime configuration changes (default false)." },
            }),
            &[],
        ),
        "diagnose" => object(
            json!({
                "window_id": window_id_property(),
                "trace_limit": { "type": "integer", "minimum": 1, "maximum": 512, "description": "Newest Vivid trace events to include (default 128)." },
            }),
            &[],
        ),
        "vivid_sessions" | "vivid_surfaces" | "vivid_tracks" => {
            object(json!({ "window_id": window_id_property() }), &[])
        }
        "vivid_surface_status" => vivid_identity_schema(false),
        "vivid_track_status" => vivid_identity_schema(true),
        "vivid_scene_status" => object(
            json!({
                "window_id": window_id_property(),
                "session_id": { "type": "integer", "minimum": 0 },
            }),
            &["session_id"],
        ),
        "vivid_trace" => object(
            json!({
                "window_id": window_id_property(),
                "after": { "type": "integer", "minimum": 0, "description": "Exclusive lower sequence bound (forward read)." },
                "tail": { "type": "boolean", "description": "Select the most recent matching records (default false)." },
                "before": { "type": "integer", "minimum": 0, "description": "Exclusive upper sequence bound." },
                "around": { "type": "integer", "minimum": 0, "description": "Sequence to center the query on." },
                "preceding": { "type": "integer", "minimum": 0, "maximum": 512, "description": "Records before `around` (default 64)." },
                "following": { "type": "integer", "minimum": 0, "maximum": 512, "description": "Records after `around` (default 64)." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 512, "description": "Maximum records (default 128)." },
                "timeout": timeout_property("30s"),
                "session_id": { "type": "integer", "minimum": 0 },
                "context_id": { "type": "integer", "minimum": 0 },
                "surface_id": { "type": "integer", "minimum": 0 },
                "track_id": { "type": "integer", "minimum": 0 },
                "category": { "type": "string", "enum": ["connection", "lifecycle", "flow", "playback", "recovery", "decode", "render"] },
                "recovery_only": { "type": "boolean", "description": "Only recovery-related events (default false)." },
            }),
            &[],
        ),
        "get_grid" => object(
            json!({
                "window_id": window_id_property(),
                "start_line": { "type": "integer", "description": "First signed physical grid line (with row_count)." },
                "row_count": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Physical rows to return (with start_line)." },
                "since_screen": { "type": "integer", "minimum": 0, "description": "Return viewport rows changed after this screen sequence (excludes an explicit range)." },
            }),
            &[],
        ),
        "wait_text" => object(
            json!({
                "window_id": window_id_property(),
                "text": { "type": "string", "description": "Text to wait for." },
                "regex": { "type": "boolean", "description": "Treat `text` as a regex (default false; patterns capped at 8 KiB)." },
                "after_screen": { "type": "integer", "minimum": 0, "description": "Require a screen newer than this sequence." },
                "line": { "type": "integer", "description": "Restrict to one viewport row (0 top, -1 bottom)." },
                "rect": {
                    "type": "object",
                    "properties": {
                        "col": { "type": "integer", "minimum": 0 },
                        "row": { "type": "integer", "minimum": 0 },
                        "width": { "type": "integer", "minimum": 1, "maximum": 1000 },
                        "height": { "type": "integer", "minimum": 1, "maximum": 1000 },
                    },
                    "required": ["col", "row", "width", "height"],
                    "additionalProperties": false,
                    "description": "Restrict to a COL,ROW,WIDTH,HEIGHT viewport rectangle.",
                },
                "timeout": timeout_property("30s"),
            }),
            &["text"],
        ),
        "wait_output" => object(
            json!({
                "window_id": window_id_property(),
                "pattern": { "type": "string", "description": "Bytes to wait for." },
                "regex": { "type": "boolean", "description": "Treat `pattern` as a regex (default false)." },
                "base64": { "type": "boolean", "description": "Treat `pattern` as base64-encoded bytes (default false)." },
                "after_offset": { "type": "integer", "minimum": 0, "description": "Transcript byte offset to start after; omit to match only future bytes." },
                "timeout": timeout_property("30s"),
            }),
            &["pattern"],
        ),
        "wait_screen_change" => object(
            json!({
                "window_id": window_id_property(),
                "after_screen": { "type": "integer", "minimum": 0 },
                "timeout": timeout_property("30s"),
            }),
            &[],
        ),
        "wait_screen_stable" => object(
            json!({
                "window_id": window_id_property(),
                "quiet": timeout_property("250ms"),
                "after_screen": { "type": "integer", "minimum": 0 },
                "timeout": timeout_property("30s"),
            }),
            &[],
        ),
        "wait_frame" => object(
            json!({
                "window_id": window_id_property(),
                "after_frame": { "type": "integer", "minimum": 0 },
                "timeout": timeout_property("30s"),
            }),
            &[],
        ),
        "wait_exit" | "wait_prompt" | "wait_command_finish" => object(
            json!({
                "window_id": window_id_property(),
                "timeout": timeout_property("30s"),
            }),
            &[],
        ),
        "wait_vivid_track" => object(
            json!({
                "window_id": window_id_property(),
                "session_id": { "type": "integer", "minimum": 0 },
                "context_id": { "type": "integer", "minimum": 0 },
                "surface_id": { "type": "integer", "minimum": 0 },
                "track_id": { "type": "integer", "minimum": 0 },
                "channel_generation": { "type": "integer", "minimum": 0, "description": "Generation scoping this wait." },
                "condition": { "type": "string", "enum": ["revision-after", "milestones", "presentation-after", "pts-after", "clock-started", "buffered-ended", "channel-accepted", "channel-detached", "track-lost"] },
                "value": { "type": "integer", "minimum": 0, "description": "Threshold for the first four conditions." },
                "timeout": timeout_property("30s"),
            }),
            &[
                "session_id",
                "context_id",
                "surface_id",
                "track_id",
                "channel_generation",
                "condition",
            ],
        ),
        "transcript" => object(
            json!({
                "window_id": window_id_property(),
                "after_offset": { "type": "integer", "minimum": 0, "description": "First retained byte offset; omit for the newest bytes." },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "description": "Maximum bytes (default 65536)." },
                "raw": { "type": "boolean", "description": "Decode to text instead of JSON metadata (default false)." },
            }),
            &[],
        ),
        _ => object(json!({}), &[]),
    }
}

fn vivid_identity_schema(with_track: bool) -> Value {
    let mut properties = json!({
        "window_id": window_id_property(),
        "session_id": { "type": "integer", "minimum": 0 },
        "context_id": { "type": "integer", "minimum": 0 },
        "surface_id": { "type": "integer", "minimum": 0 },
    });
    let mut required = vec!["session_id", "context_id", "surface_id"];
    if with_track {
        properties["track_id"] = json!({ "type": "integer", "minimum": 0 });
        required.push("track_id");
    }
    object(properties, &required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validates_names_and_modifiers() {
        assert!(translate("key", &json!({"key": "Enter"})).is_ok());
        assert!(translate("key", &json!({"key": "x"})).is_ok());
        assert!(translate("key", &json!({"key": "F12"})).is_ok());
        assert!(translate("key", &json!({"key": "Nope"})).is_err());
        assert!(translate("key", &json!({"key": "a", "mods": ["Hyper"]})).is_err());
    }

    #[test]
    fn mouse_requires_exactly_one_coordinate_pair() {
        assert!(translate("mouse", &json!({"action": "click", "x": 10, "y": 20})).is_ok());
        assert!(translate("mouse", &json!({"action": "click"})).is_err());
        assert!(
            translate(
                "mouse",
                &json!({"action": "click", "x": 10, "y": 20, "cell_column": 1, "cell_row": 2}),
            )
            .is_err()
        );
    }

    #[test]
    fn durations_parse_like_the_cli() {
        assert_eq!(parse_duration("500").unwrap(), 500);
        assert_eq!(parse_duration("500ms").unwrap(), 500);
        assert_eq!(parse_duration("30s").unwrap(), 30_000);
        assert_eq!(parse_duration("5m").unwrap(), 300_000);
        assert_eq!(parse_duration("1h").unwrap(), 3_600_000);
        assert!(parse_duration("soon").is_err());
    }

    #[test]
    fn modifiers_and_routes_fold_case() {
        let params = translate("key", &json!({"key": "c", "mods": ["ctrl", "SHIFT"]})).unwrap();
        assert_eq!(params["mods"], json!(["Ctrl", "Shift"]));
        let params = translate("key", &json!({"key": "c", "route": "UI"})).unwrap();
        assert_eq!(params["route"], "ui");
        let params = translate("set_level", &json!({"level": "Always_On_Top"})).unwrap();
        assert_eq!(params["level"], "always_on_top");
    }

    #[test]
    fn named_keys_fold_case_but_chars_do_not() {
        let params = translate("key", &json!({"key": "enter"})).unwrap();
        assert_eq!(params["key"], "Enter");
        let params = translate("key", &json!({"key": "f12"})).unwrap();
        assert_eq!(params["key"], "F12");
        let params = translate("key", &json!({"key": "a"})).unwrap();
        assert_eq!(params["key"], "a");
        assert!(translate("key", &json!({"key": "Nope"})).is_err());
        assert!(translate("key", &json!({"key": "éx"})).is_err());
    }

    #[test]
    fn mouse_accepts_dashed_actions_and_rejects_noop_scrolls() {
        let params =
            translate("mouse", &json!({"action": "double-click", "x": 1, "y": 2})).unwrap();
        assert!(params["action"]["double_click"].is_object());
        assert!(translate("mouse", &json!({"action": "scroll", "x": 1, "y": 2})).is_err());
        assert!(
            translate("mouse", &json!({"action": "scroll", "x": 1, "y": 2, "vertical": 3})).is_ok()
        );
    }

    #[test]
    fn vivid_conditions_accept_snake_case() {
        let base = json!({
            "session_id": 1, "context_id": 2, "surface_id": 3, "track_id": 4,
            "channel_generation": 5, "value": 7,
        });
        for (spelling, wire) in [("pts-after", 4), ("pts_after", 4), ("clock_started", 5)] {
            let mut args = base.clone();
            args["condition"] = json!(spelling);
            if wire >= 5 {
                args.as_object_mut().unwrap().remove("value");
            }
            let params = translate("wait_vivid_track", &args).unwrap();
            assert_eq!(params["condition"], wire, "{spelling}");
        }
    }

    #[test]
    fn drop_at_accepts_col_row_string() {
        let file = std::env::temp_dir().join(format!("vivido-mcp-at-{}.txt", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        let params =
            translate("drop_file", &json!({"path": file.to_string_lossy(), "at": "10,4"})).unwrap();
        assert_eq!(params["at"], json!([10, 4]));
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn create_window_accepts_command_array() {
        let params =
            translate("create_window", &json!({"command": ["./run.sh", "--fast"]})).unwrap();
        assert_eq!(params["terminal_options"]["command"], json!(["./run.sh", "--fast"]));
        assert!(translate("create_window", &json!({"program": "sh", "command": ["sh"]})).is_err());
        assert!(translate("create_window", &json!({"command": []})).is_err());
    }

    #[test]
    fn single_strings_coerce_to_lists() {
        let params = translate("key", &json!({"key": "c", "mods": "Ctrl"})).unwrap();
        assert_eq!(params["mods"], json!(["Ctrl"]));
        let params = translate("config", &json!({"options": "cursor.style=\"Beam\""})).unwrap();
        assert_eq!(params["options"], json!(["cursor.style=\"Beam\""]));
    }

    #[test]
    fn exec_rejects_empty_commands() {
        assert!(translate("exec", &json!({"command": "  "})).is_err());
        assert!(translate("exec", &json!({"command": "ls"})).is_ok());
    }

    #[test]
    fn unknown_arguments_fail_with_a_suggestion() {
        let error = translate("key", &json!({"key": "Enter", "windowd_id": 3})).unwrap_err();
        assert!(error.contains("windowd_id"), "{error}");
        assert!(error.contains("window_id"), "{error}");
        assert!(suggest("get_tex", &["get_text", "get_grid"]).as_deref() == Some("get_text"));
        assert!(suggest("zzzzzz", &["get_text"]).is_none());
    }

    #[test]
    fn vivid_wait_conditions_map_to_wire_values() {
        let params = translate(
            "wait_vivid_track",
            &json!({
                "session_id": 1, "context_id": 2, "surface_id": 3, "track_id": 4,
                "channel_generation": 5, "condition": "presentation-after", "value": 900,
            }),
        )
        .unwrap();
        assert_eq!(params["condition"], 3);
        assert!(
            translate(
                "wait_vivid_track",
                &json!({
                    "session_id": 1, "context_id": 2, "surface_id": 3, "track_id": 4,
                    "channel_generation": 5, "condition": "clock-started", "value": 1,
                }),
            )
            .is_err()
        );
    }
}
