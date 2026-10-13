#![cfg(unix)]

// CLI surface: startup validation, env fallbacks, and help text.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use serde_json::json;

use common::MockServer;

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_vivido-mcp")
}

#[test]
fn bad_tier_fails_fast() {
    let output = Command::new(exe()).arg("--allow").arg("everything").output().expect("spawn");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown tier"), "{stderr}");
}

#[test]
fn bad_target_fails_fast() {
    let output = Command::new(exe()).arg("--target").arg("../escape").output().expect("spawn");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--target"), "{stderr}");
}

#[test]
fn bad_max_dim_fails_fast() {
    let output = Command::new(exe()).args(["--max-image-dim", "31"]).output().expect("spawn");
    assert!(!output.status.success());
}

#[test]
fn help_shows_examples() {
    let output = Command::new(exe()).arg("--help").output().expect("spawn");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--allow input"), "{help}");
    assert!(help.contains("VIVIDO_MCP_ALLOW"), "{help}");
}

#[test]
fn allow_env_unlocks_tiers() {
    let mock = MockServer::start();
    let mut child = Command::new(exe())
        .arg("--socket")
        .arg(&mock.socket)
        .env("VIVIDO_MCP_ALLOW", "input")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "key", "arguments": {"key": "Enter"}},
    });
    writeln!(stdin, "{call}").expect("write");
    stdin.flush().expect("flush");
    let mut reply = String::new();
    reader.read_line(&mut reply).expect("read");
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("parse");
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    drop(stdin);
    assert!(child.wait().expect("wait").success());
}

#[test]
fn vivido_bin_env_is_honored() {
    let mock = MockServer::start();
    let mut child = Command::new(exe())
        .arg("--socket")
        .arg(&mock.socket)
        .args(["--allow", "process"])
        .env("VIVIDO_BIN", "/nonexistent/vivido-xyz")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "headless_start", "arguments": {"session": "env-probe"}},
    });
    writeln!(stdin, "{call}").expect("write");
    stdin.flush().expect("flush");
    let mut reply = String::new();
    reader.read_line(&mut reply).expect("read");
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("parse");
    assert_eq!(reply["result"]["isError"], true);
    let message =
        reply["result"]["structuredContent"]["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("/nonexistent/vivido-xyz"), "{message}");
    drop(stdin);
    assert!(child.wait().expect("wait").success());
}

#[test]
fn connect_logs_server_version_and_timing() {
    let mock = MockServer::start();
    let mut child = Command::new(exe())
        .arg("--socket")
        .arg(&mock.socket)
        .env("VIVIDO_MCP_LOG_TIMINGS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let mut reader = BufReader::new(stdout);
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "ping", "arguments": {}},
    });
    writeln!(stdin, "{call}").expect("write");
    stdin.flush().expect("flush");
    let mut reply = String::new();
    reader.read_line(&mut reply).expect("read");
    drop(stdin);
    assert!(child.wait().expect("wait").success());
    let mut logged = String::new();
    BufReader::new(stderr).read_to_string(&mut logged).expect("read stderr");
    assert!(logged.contains("9.9.9-mock"), "{logged}");
    assert!(logged.contains("timing"), "{logged}");
    assert!(logged.contains("tools"), "{logged}");
}

#[test]
fn initialize_logs_client_and_oversize_results() {
    let mock = MockServer::start();
    mock.on("get_text", std::sync::Arc::new(|_| Ok(json!({"text": "x".repeat(2 * 1024 * 1024)}))));
    let mut child = Command::new(exe())
        .arg("--socket")
        .arg(&mock.socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let mut reader = BufReader::new(stdout);
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"clientInfo": {"name": "probe-client", "version": "7"}, "protocolVersion": "2025-06-18"},
    });
    writeln!(stdin, "{init}").expect("write");
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "get_text", "arguments": {}},
    });
    writeln!(stdin, "{call}").expect("write");
    stdin.flush().expect("flush");
    let mut line = String::new();
    reader.read_line(&mut line).expect("read init");
    line.clear();
    reader.read_line(&mut line).expect("read call");
    drop(stdin);
    assert!(child.wait().expect("wait").success());
    let mut logged = String::new();
    BufReader::new(stderr).read_to_string(&mut logged).expect("read stderr");
    assert!(logged.contains("probe-client 7"), "{logged}");
    assert!(logged.contains("get_text"), "{logged}");
    assert!(logged.contains("bytes"), "{logged}");
}
