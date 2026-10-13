#![cfg(unix)]

// End-to-end over stdio: spawn the real binary against a mock endpoint and
// speak MCP to it.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

use common::MockServer;

struct Mcp {
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
    _child: std::process::Child,
}

impl Mcp {
    fn spawn(mock: &MockServer, args: &[&str]) -> Self {
        let exe = env!("CARGO_BIN_EXE_vivido-mcp");
        let mut command = Command::new(exe);
        command.arg("--socket").arg(&mock.socket).args(args);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn vivido-mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        Self { stdin, reader: BufReader::new(stdout), next_id: 1, _child: child }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{line}").expect("write request");
        self.stdin.flush().expect("flush");
        let mut reply = String::new();
        self.reader.read_line(&mut reply).expect("read reply");
        serde_json::from_str(&reply).expect("parse reply")
    }

    fn notify(&mut self, method: &str) {
        let line = json!({"jsonrpc": "2.0", "method": method});
        writeln!(self.stdin, "{line}").expect("write notification");
        self.stdin.flush().expect("flush");
    }
}

#[test]
fn initialize_lists_live_tools() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &[]);
    let reply = mcp.request("initialize", json!({}));
    assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(reply["result"]["serverInfo"]["name"], "vivido-mcp");
    let reply = mcp.request("tools/list", json!({}));
    let names: Vec<String> = reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    for expected in ["list_windows", "screenshot", "key", "quit", "capture", "drain_events"] {
        assert!(names.contains(&expected.to_owned()), "{names:?}");
    }
    // Live host methods from the mock handshake are listed too.
    assert!(names.contains(&"vivida_layout".to_owned()), "{names:?}");
}

#[test]
fn tool_calls_return_typed_results() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &[]);
    mcp.request("initialize", json!({}));
    let reply = mcp.request("tools/call", json!({"name": "ping", "arguments": {}}));
    assert_eq!(reply["result"]["isError"], false);
    let reply = mcp.request("tools/call", json!({"name": "key", "arguments": {"key": "Enter"}}));
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(reply["result"]["structuredContent"]["error"]["code"], "tier_refused");
    let reply = mcp.request("tools/call", json!({"name": "nope", "arguments": {}}));
    assert_eq!(reply["result"]["isError"], true);
}

#[test]
fn allow_flags_unlock_tiers() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &["--allow", "input"]);
    mcp.request("initialize", json!({}));
    let reply = mcp.request("tools/call", json!({"name": "key", "arguments": {"key": "Enter"}}));
    assert_eq!(reply["result"]["isError"], false);
}

#[test]
fn prompts_teach_the_workflow() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &[]);
    mcp.request("initialize", json!({}));
    let reply = mcp.request("prompts/list", json!({}));
    assert_eq!(reply["result"]["prompts"].as_array().unwrap().len(), 3);
    let reply =
        mcp.request("prompts/get", json!({"name": "vivido_workflow", "arguments": {"goal": "x"}}));
    let text = reply["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("DISCOVER"), "{text}");
    assert!(text.contains("Session context:"), "{text}");
}

#[test]
fn notifications_take_no_reply() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &[]);
    mcp.request("initialize", json!({}));
    mcp.notify("notifications/initialized");
    // The next request still gets the next reply: no frame was emitted for it.
    let reply = mcp.request("ping", json!({}));
    assert!(reply.get("result").is_some(), "{reply}");
}

#[test]
fn missing_tool_name_is_a_protocol_error() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &[]);
    mcp.request("initialize", json!({}));
    let reply = mcp.request("tools/call", json!({"arguments": {}}));
    assert_eq!(reply["error"]["code"], -32602);
}

#[test]
fn dollar_methods_are_acknowledged() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &[]);
    mcp.request("initialize", json!({}));
    let reply = mcp.request("$/cancelRequest", json!({}));
    assert!(reply.get("result").is_some(), "{reply}");
}

#[test]
fn id_less_calls_take_no_reply() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &[]);
    mcp.request("initialize", json!({}));
    // A tools/call without an id is a notification: no reply frame, so the
    // next request still receives the next reply.
    let line = serde_json::json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "ping", "arguments": {}}});
    writeln!(mcp.stdin, "{line}").expect("write");
    mcp.stdin.flush().expect("flush");
    let reply = mcp.request("ping", json!({}));
    assert_eq!(reply["id"], 2);
}

#[test]
fn unknown_tools_suggest_neighbours() {
    let mock = MockServer::start();
    let mcp = &mut Mcp::spawn(&mock, &[]);
    mcp.request("initialize", json!({}));
    let reply = mcp.request("tools/call", json!({"name": "get_tex", "arguments": {}}));
    assert_eq!(reply["result"]["isError"], true);
    let message = reply["result"]["structuredContent"]["error"]["message"].as_str().unwrap();
    assert!(message.contains("get_text"), "{message}");
}
