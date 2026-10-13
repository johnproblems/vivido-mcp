//! A stdio MCP server exposing Vivido as tools.
//!
//! JSON-RPC 2.0 over newline-delimited stdin/stdout, hand-rolled like
//! `vvagent mcp`: the surface is a few protocol methods plus tool dispatch, and
//! this binary is one an agent runs, so its dependency footprint stays small.
//!
//! Two rules govern every tool result:
//!
//! - **Typed errors stay typed.** A failed tool returns `isError` with a stable
//!   machine code (`tier_refused`, `window_not_found`, ...), not prose.
//! - **Endpoint material never leaks.** The socket path, `VIVIDO_SOCKET`, and
//!   tokens are redacted from every result before it is written.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::server::Server;

/// MCP protocol version, matching `vvagent mcp`.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// Tool results larger than this earn a stderr note: giant grids and transcripts
/// can swamp a client's context window.
const OVERSIZE_BYTES: usize = 1024 * 1024;

/// Serve MCP on stdio until EOF.
pub fn serve(server: &mut Server) {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) => {
                eprintln!("vivido-mcp: stdin error: {error}");
                return;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(error) => {
                if write_response(
                    &mut stdout,
                    &error_response(Value::Null, -32700, &error.to_string()),
                )
                .is_err()
                {
                    return;
                }
                continue;
            }
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        // A notification has no id and takes no reply, whatever its method.
        if id.is_null() {
            continue;
        }

        if method.starts_with("$/") {
            // Client housekeeping (`$/cancelRequest`, `$/progress`): acknowledge.
            if write_response(&mut stdout, &ok_response(id, json!({}))).is_err() {
                return;
            }
            continue;
        }

        let response = match method {
            "initialize" => {
                log_client(&params);
                ok_response(id, initialize())
            }
            "tools/list" => ok_response(
                id,
                json!({
                    "tools": server.catalog().iter().map(|tool| tool.listing()).collect::<Vec<_>>(),
                }),
            ),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                if name.is_empty() {
                    error_response(id, -32602, "tools/call needs a `name`")
                } else {
                    let args = params.get("arguments").cloned().unwrap_or(json!({}));
                    let output = server.call(name, args);
                    let bytes = output.structured.to_string().len();
                    if bytes > OVERSIZE_BYTES {
                        eprintln!(
                            "vivido-mcp: note: `{name}` returned {bytes} bytes; consider cropping, limits, or offsets"
                        );
                    }
                    ok_response(
                        id,
                        json!({
                            "content": output.content,
                            "structuredContent": output.structured,
                            "isError": output.is_error,
                        }),
                    )
                }
            }
            "prompts/list" => ok_response(id, crate::prompts::list()),
            "prompts/get" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match crate::prompts::get(name, &args, Some(&server.context())) {
                    Ok(prompt) => ok_response(id, prompt),
                    Err(message) => error_response(id, -32602, &message),
                }
            }
            "ping" => ok_response(id, json!({})),
            _ => error_response(id, -32601, &format!("unknown method `{method}`")),
        };
        if write_response(&mut stdout, &response).is_err() {
            return;
        }
    }
}

fn write_response(out: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).unwrap_or_else(|_| "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32603,\"message\":\"cannot encode response\"}}".to_owned());
    line.push('\n');
    out.write_all(line.as_bytes()).and_then(|()| out.flush())
}

/// Log which client connected, for operator diagnostics.
fn log_client(params: &Value) {
    let name = params
        .get("clientInfo")
        .and_then(|info| info.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("unknown client");
    let version = params
        .get("clientInfo")
        .and_then(|info| info.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    let protocol = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("?");
    eprintln!("vivido-mcp: client {name} {version} (mcp {protocol})");
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": { "listChanged": false },
            "prompts": { "listChanged": false },
        },
        "serverInfo": { "name": "vivido-mcp", "version": env!("CARGO_PKG_VERSION") },
        "instructions":
            "See and drive Vivido terminals. Start with list_windows and inspect; read sequences \
             before acting; act, then wait for what you expect (never sleep); verify with capture \
             or get_grid. Tools are tiered: observe-only by default, input needs `--allow input`, \
             windows/processes/headless need `--allow process`. Prefer an ephemeral headless \
             session for agent work. The vivido_workflow prompt teaches the full loop.",
    })
}
