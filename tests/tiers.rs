#![cfg(unix)]

// Tier gating: every mutating method is refused by default, and refusals name --allow.

mod common;

use std::sync::Arc;

use serde_json::json;

use common::MockServer;
use vivido_mcp::tiers::Tier;

#[test]
fn every_non_observe_tool_is_refused_by_default() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &[]);
    let mut refused = 0;
    let mut allowed = 0;
    // Tier is checked before argument validation, so near-empty args isolate
    // the gate (event tools get a short timeout so the test stays fast).
    let catalog = server.catalog().to_vec();
    for tool in &catalog {
        let args = if tool.name == "drain_events" || tool.name == "subscribe" {
            json!({"timeout": "50ms"})
        } else {
            json!({})
        };
        let output = server.call(&tool.name, args);
        if tool.tier == Tier::Observe {
            // Observe tools get past the gate (they may still fail for
            // missing arguments or mock behavior — anything but tier_refused).
            assert_ne!(
                output.structured["error"]["code"].as_str(),
                Some("tier_refused"),
                "{} should pass the tier gate",
                tool.name
            );
            allowed += 1;
        } else {
            assert!(output.is_error, "{} should be refused", tool.name);
            assert_eq!(output.structured["error"]["code"], "tier_refused", "{}", tool.name);
            let message =
                output.structured["error"]["message"].as_str().unwrap_or_default().to_owned();
            assert!(message.contains("--allow"), "{message}");
            assert!(message.contains(tool.tier.allow_name()), "{message}");
            refused += 1;
        }
    }
    assert!(refused > 10, "expected many gated tools, got {refused}");
    assert!(allowed > 10, "expected many observe tools, got {allowed}");
}

#[test]
fn input_tier_opens_input_but_not_process() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &["input"]);
    for (tool, args) in [
        ("typing", json!({"text": "hi"})),
        ("type_text", json!({"text": "hi"})),
        ("key", json!({"key": "Enter"})),
        ("paste", json!({"text": "hi"})),
        ("mouse", json!({"action": "move", "x": 10, "y": 20})),
    ] {
        let output = server.call(tool, args);
        assert!(!output.is_error, "{tool}: {}", output.structured);
    }
    for tool in ["quit", "create_window", "signal", "close_window", "headless_start"] {
        let output = server.call(tool, json!({}));
        assert_eq!(output.structured["error"]["code"], "tier_refused", "{tool}");
    }
}

#[test]
fn process_tier_opens_everything() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &["process"]);
    // Spot-check one tool per tier plus arg validation still applying.
    let output = server.call("list_windows", json!({}));
    assert!(!output.is_error, "{}", output.structured);
    let output = server.call("key", json!({"key": "Enter"}));
    assert!(!output.is_error, "{}", output.structured);
    let output = server.call("quit", json!({}));
    assert!(!output.is_error, "{}", output.structured);
    let output = server.call("key", json!({"key": "Nope"}));
    assert_eq!(output.structured["error"]["code"], "invalid_args");
}

#[test]
fn wire_shapes_match_the_upstream_contract() {
    let mock = MockServer::start();
    mock.on(
        "transcript",
        Arc::new(|_| {
            Ok(json!({
                "data": "aGVsbG8K",
                "truncated": false,
                "start_offset": 0,
                "end_offset": 6,
                "current_end_offset": 6,
            }))
        }),
    );
    let server = &mut common::test_server(&mock, &["process"]);
    // Nested targets, wait `common`, numeric Vivid conditions: the exact
    // envelopes the CLI sends.
    let calls = [
        ("key", json!({"key": "Enter", "window_id": 3})),
        ("wait_text", json!({"text": "hi", "window_id": 3, "timeout": 5000})),
        (
            "wait_vivid_track",
            json!({
                "session_id": 1, "context_id": 2, "surface_id": 3, "track_id": 4,
                "channel_generation": 5, "condition": "pts-after", "value": 7,
            }),
        ),
        ("mouse", json!({"action": "click", "cell_column": 1, "cell_row": 2})),
        ("transcript", json!({"window_id": 3, "raw": true})),
    ];
    for (tool, args) in calls {
        let output = server.call(tool, args);
        assert!(!output.is_error, "{tool}: {}", output.structured);
    }
    let recorded = mock.recorded();
    let params = |method: &str| {
        recorded.iter().find(|(name, _)| name == method).map(|(_, params)| params.clone()).unwrap()
    };
    assert_eq!(params("key")["target"]["window_id"], 3);
    assert_eq!(params("wait_text")["common"]["timeout"], 5000);
    assert_eq!(params("wait_text")["common"]["target"]["window_id"], 3);
    assert_eq!(params("wait_vivid_track")["condition"], 4);
    assert_eq!(params("mouse")["action"]["click"]["position"]["cell_column"], 1);
    assert_eq!(params("transcript")["target"]["window_id"], 3);
    let raw = server.call("transcript", json!({"window_id": 3, "raw": true}));
    assert!(!raw.is_error, "{}", raw.structured);
    assert_eq!(raw.structured["text"], "hello\n");
}

#[test]
fn disconnected_errors_suggest_headless_start() {
    let server = &mut vivido_mcp::server::Server::new(vivido_mcp::server::ServerConfig {
        allowance: vivido_mcp::tiers::parse_allow(&["observe".to_owned()]).unwrap(),
        socket: Some(std::path::PathBuf::from("/nonexistent/vivido-mcp-xyz.sock")),
        target: None,
        vivido_bin: "vivido".to_owned(),
        default_max_dim: 1280,
    });
    assert!(!server.connected());
    let output = server.call("ping", serde_json::json!({}));
    assert!(output.is_error);
    assert_eq!(output.structured["error"]["code"], "transport_error");
    let message = output.structured["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("headless_start"), "{message}");
    assert!(!message.contains(".sock"), "{message}");
}

#[test]
fn transcript_raw_reports_lossiness() {
    let mock = MockServer::start();
    mock.on(
        "transcript",
        Arc::new(|_| {
            Ok(serde_json::json!({
                "data": "//4=",
                "truncated": false,
                "start_offset": 0,
                "end_offset": 2,
                "current_end_offset": 2,
            }))
        }),
    );
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("transcript", serde_json::json!({"raw": true}));
    assert!(!output.is_error, "{}", output.structured);
    assert_eq!(output.structured["lossy"], true);
    assert_eq!(output.structured["bytes"], 2);
}

#[test]
fn typos_fail_as_invalid_args() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &["process"]);
    for (tool, args) in [
        ("key", serde_json::json!({"key": "Enter", "windowd_id": 3})),
        ("capture", serde_json::json!({"stabel": true})),
        ("drain_events", serde_json::json!({"limti": 5})),
    ] {
        let output = server.call(tool, args);
        assert!(output.is_error, "{tool}");
        assert_eq!(output.structured["error"]["code"], "invalid_args", "{tool}");
    }
}
