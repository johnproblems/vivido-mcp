#![cfg(unix)]

// Protocol conformance: hello-first, version refusal, framing, typed errors.

mod common;

use serde_json::{Value, json};
use std::sync::Arc;

use common::MockServer;
use vivido_mcp::ipc::{Connection, Failure};

#[test]
fn client_sends_hello_first() {
    let mock = MockServer::start_with(true);
    let mut conn = Connection::connect(&mock.socket).expect("connect");
    let hello = conn.hello().expect("hello");
    assert_eq!(hello.server_version, "9.9.9-mock");
    assert!(hello.headless);
    let recorded = mock.recorded();
    assert_eq!(recorded[0].0, "hello");
}

#[test]
fn unknown_protocol_version_is_refused_clearly() {
    let mock = MockServer::start();
    mock.set_protocol_version(99);
    let mut conn = Connection::connect(&mock.socket).expect("connect");
    let error = conn.hello().unwrap_err();
    assert!(matches!(error, Failure::Protocol(_)), "{error:?}");
    assert!(error.message().contains("99"), "{}", error.message());
    assert!(error.message().contains('2'), "{}", error.message());
}

#[test]
fn oversized_requests_fail_client_side() {
    let mock = MockServer::start();
    let mut conn = Connection::connect(&mock.socket).expect("connect");
    conn.hello().expect("hello");
    let big = "x".repeat(2 * 1024 * 1024);
    let error = conn.request("typing", json!({"text": big})).unwrap_err();
    assert!(matches!(error, Failure::Protocol(_)), "{error:?}");
    assert!(error.message().contains("1 MiB"), "{}", error.message());
}

#[test]
fn server_errors_stay_typed() {
    let mock = MockServer::start();
    mock.on(
        "inspect",
        Arc::new(|_| Err(("window_not_found".to_owned(), "no such window".to_owned()))),
    );
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("inspect", json!({"window_id": 4242}));
    assert!(output.is_error);
    assert_eq!(output.structured["error"]["code"], "window_not_found");
}

#[test]
fn stray_events_on_the_main_connection_are_ignored() {
    // Scripted peer: answer hello, then answer ping with an event frame
    // followed by the correlated response. The client must skip the event
    // and still correlate the reply.
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("s.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut writer = stream;
        let mut line = String::new();
        reader.read_line(&mut line).expect("read hello");
        let hello: Value = serde_json::from_str(&line).expect("parse hello");
        assert_eq!(hello["method"], "hello");
        let reply = json!({
            "version": 2, "id": hello["id"], "ok": true,
            "result": {"protocol_version": 2, "methods": [], "method_capabilities": []},
        });
        writeln!(writer, "{reply}").expect("write hello");
        line.clear();
        reader.read_line(&mut line).expect("read ping");
        let ping: Value = serde_json::from_str(&line).expect("parse ping");
        let event = json!({
            "version": 2, "subscription_id": 7, "event_sequence": 1,
            "event": {"type": "bell", "data": {}},
        });
        writeln!(writer, "{event}").expect("write stray event");
        let reply = json!({"version": 2, "id": ping["id"], "ok": true, "result": {"pong": true}});
        writeln!(writer, "{reply}").expect("write pong");
    });
    let mut conn = Connection::connect(&path).expect("connect");
    conn.hello().expect("hello");
    let value: Value = conn.request("ping", json!({})).expect("ping");
    assert_eq!(value, json!({"pong": true}));
    assert_eq!(conn.stray_events(), 1);
}

#[test]
fn bad_method_names_never_reach_the_wire() {
    let mock = MockServer::start();
    let mut conn = Connection::connect(&mock.socket).expect("connect");
    conn.hello().expect("hello");
    assert!(conn.request("key; rm -rf", json!({})).is_err());
    assert!(mock.recorded().iter().all(|(method, _)| method != "key; rm -rf"));
}
