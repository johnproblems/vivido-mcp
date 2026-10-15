#![cfg(unix)]

// Phase 2 improvements: server-level behavior changes over the mock endpoint.

mod common;

use std::sync::Arc;

use serde_json::json;

use common::MockServer;

fn screenshot_handler(mock: &MockServer, width: u32, height: u32) {
    let path = mock.write_png(width, height);
    let result = mock.screenshot_result(&path, 3, (10, 10), (0, 0));
    mock.on("screenshot", Arc::new(move |_| Ok(result.clone())));
}

fn pngs_left(mock: &MockServer) -> usize {
    std::fs::read_dir(mock.socket.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .count()
}

#[test]
fn capture_accepts_format_and_quality() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100);
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("capture", json!({"format": "jpeg", "quality": 70}));
    assert!(!output.is_error, "{}", output.structured);
    assert_eq!(output.structured["mime"], "image/jpeg");
    assert_eq!(output.structured["format"], "jpeg");
}

#[test]
fn crop_unknown_keys_fail_cleanly() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100);
    let server = &mut common::test_server(&mock, &[]);
    let output = server
        .call("screenshot", json!({"crop": {"col": 0, "row": 0, "cols": 1, "rows": 1, "zoom": 2}}));
    assert!(output.is_error);
    assert_eq!(output.structured["error"]["code"], "image_error");
    assert!(
        output.structured["error"]["message"].as_str().unwrap().contains("zoom"),
        "{}",
        output.structured
    );
}

#[test]
fn schemas_document_accepted_coercions() {
    let catalog = vivido_mcp::tools::fallback_catalog();
    for name in ["drain_events", "subscribe"] {
        let tool = vivido_mcp::tools::find(&catalog, name).unwrap();
        let events = &tool.schema["properties"]["events"];
        assert!(
            events["type"].as_array().unwrap().iter().any(|t| t == "string"),
            "{name}: {}",
            tool.schema
        );
    }
    let mouse = vivido_mcp::tools::find(&catalog, "mouse").unwrap();
    let actions = mouse.schema["properties"]["action"]["enum"].as_array().unwrap();
    assert!(actions.iter().any(|action| action == "double-click"), "{actions:?}");
}

#[test]
fn passthrough_tools_reject_non_object_args() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &["process"]);
    let output = server.call("future_method", json!([1, 2]));
    assert!(output.is_error);
    assert_eq!(output.structured["error"]["code"], "invalid_args");
}

#[test]
fn composites_validate_window_id_before_the_wire() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100);
    let server = &mut common::test_server(&mock, &[]);
    for (tool, args) in [
        ("screenshot", json!({"window_id": "foo"})),
        ("screenshot", json!({"window_id": 1.5})),
        ("capture", json!({"window_id": [3]})),
    ] {
        let output = server.call(tool, args);
        assert!(output.is_error, "{tool}");
        assert_eq!(output.structured["error"]["code"], "invalid_args", "{tool}");
    }
    assert!(mock.recorded().iter().all(|(method, _)| method != "screenshot"));
}

#[test]
fn capture_rejects_non_boolean_flags() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100);
    let server = &mut common::test_server(&mock, &[]);
    for (flag, value) in [("activate", json!("yes")), ("stable", json!(1))] {
        let output = server.call("capture", json!({flag: value}));
        assert!(output.is_error, "{flag}");
        assert_eq!(output.structured["error"]["code"], "invalid_args", "{flag}");
    }
}

#[test]
fn event_filters_reject_non_string_kinds() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &[]);
    for (tool, args) in [
        ("drain_events", json!({"events": ["bell", 42], "timeout": "50ms"})),
        ("subscribe", json!({"events": [{}], "timeout": "50ms"})),
    ] {
        let output = server.call(tool, args);
        assert!(output.is_error, "{tool}");
        assert_eq!(output.structured["error"]["code"], "invalid_args", "{tool}");
    }
}

#[test]
fn headless_start_validates_types_before_spawning() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &["process"]);
    for args in [
        json!({"session": 42}),
        json!({"size": 42}),
        json!({"program": 42}),
        json!({"ephemeral": "yes"}),
        json!({"args": ["ok", 42]}),
        json!({"args": "solo"}),
        json!({"args": ["--fast"]}),
    ] {
        let output = server.call("headless_start", args);
        assert!(output.is_error);
        assert_eq!(output.structured["error"]["code"], "invalid_args", "{}", output.structured);
    }
}

#[test]
fn attach_and_stop_distinguish_missing_from_mistyped_targets() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &["process"]);
    let missing = server.call("headless_attach", json!({}));
    assert_eq!(missing.structured["error"]["code"], "invalid_args");
    assert!(
        missing.structured["error"]["message"].as_str().unwrap().contains("missing required"),
        "{}",
        missing.structured
    );
    for (tool, args) in [
        ("headless_attach", json!({"target": 42})),
        ("headless_stop", json!({"target": 42})),
        ("headless_stop", json!({"target": "x", "force": "yes"})),
    ] {
        let output = server.call(tool, args);
        assert!(output.is_error, "{tool}");
        assert_eq!(output.structured["error"]["code"], "invalid_args", "{tool}");
    }
}

#[test]
fn session_context_reports_the_tool_count() {
    let mock = MockServer::start();
    let server = common::test_server(&mock, &["input"]);
    let context = server.context();
    assert!(context.contains("enabled tiers: observe,input"), "{context}");
    assert!(context.contains("9.9.9-mock"), "{context}");
    assert!(context.contains(&format!("{} tools", server.catalog().len())), "{context}");
}

#[test]
fn failed_screenshots_still_delete_the_temp_file() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100);
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("screenshot", json!({"format": "gif"}));
    assert_eq!(output.structured["error"]["code"], "image_error");
    assert_eq!(pngs_left(&mock), 0);
    screenshot_handler(&mock, 200, 100);
    let output =
        server.call("screenshot", json!({"crop": {"col": 500, "row": 0, "cols": 1, "rows": 1}}));
    assert_eq!(output.structured["error"]["code"], "image_error");
    assert_eq!(pngs_left(&mock), 0);
}

#[test]
fn quality_is_validated_for_every_format() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100);
    let server = &mut common::test_server(&mock, &[]);
    for args in [json!({"quality": 999}), json!({"format": "png", "quality": 0})] {
        let output = server.call("screenshot", args);
        assert!(output.is_error);
        assert_eq!(output.structured["error"]["code"], "image_error", "{}", output.structured);
    }
}

#[test]
fn screenshot_metadata_reports_raw_encoded_bytes() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100);
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("screenshot", json!({}));
    assert!(!output.is_error, "{}", output.structured);
    let bytes = output.structured["bytes"].as_u64().unwrap();
    let encoded = output.structured["encoded_bytes"].as_u64().unwrap();
    assert!(encoded > 100, "{}", output.structured);
    assert!(encoded < bytes, "{}", output.structured);
}

#[test]
fn unknown_tools_fall_back_to_prefixes() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("headless", json!({}));
    assert!(output.is_error);
    assert_eq!(output.structured["error"]["code"], "tool_not_found");
    let message = output.structured["error"]["message"].as_str().unwrap();
    assert!(message.contains("did you mean"), "{message}");
    assert!(message.contains("headless_"), "{message}");
}
