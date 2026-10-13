#![cfg(unix)]

// Tool generation from the live hello: coverage, tiers, and version tracking.

mod common;

use serde_json::json;

use common::{MOCK_EVENT_KINDS, MOCK_METHODS, MockServer};
use vivido_mcp::ipc::{HelloInfo, MethodCapability};
use vivido_mcp::tiers::Tier;
use vivido_mcp::tools;

fn mock_hello() -> HelloInfo {
    HelloInfo {
        server_version: "9.9.9-mock".to_owned(),
        headless: true,
        session: Some("mock".to_owned()),
        automation_name: Some("mock".to_owned()),
        methods: MOCK_METHODS.iter().map(|entry| entry.0.to_owned()).collect(),
        capabilities: MOCK_METHODS
            .iter()
            .map(|entry| MethodCapability {
                name: entry.0.to_owned(),
                class: entry.1.to_owned(),
                mutating: entry.2,
                host_claimed: entry.3,
            })
            .collect(),
        event_kinds: MOCK_EVENT_KINDS.iter().map(|s| s.to_string()).collect(),
        limits: json!({}),
    }
}

#[test]
fn every_observe_method_is_exposed() {
    let catalog = tools::build_catalog(&mock_hello());
    for (method, class, mutating, _) in MOCK_METHODS {
        if *method == "hello" {
            continue;
        }
        let tool = tools::find(&catalog, method)
            .unwrap_or_else(|| panic!("hello method `{method}` has no tool"));
        // A method hello marks mutating is never placed in observe.
        if *mutating {
            assert_ne!(tool.tier, Tier::Observe, "{method} is mutating");
        }
        if !mutating && *class == "observe" {
            assert_eq!(tool.tier, Tier::Observe, "{method} should be observe");
        }
    }
}

#[test]
fn mutating_observe_claim_is_overruled() {
    let catalog = tools::build_catalog(&mock_hello());
    let sneaky = tools::find(&catalog, "sneaky_observe").expect("sneaky tool");
    assert_ne!(sneaky.tier, Tier::Observe);
    assert_eq!(sneaky.tier, Tier::Process);
}

#[test]
fn unknown_methods_track_versions_without_hand_edits() {
    let catalog = tools::build_catalog(&mock_hello());
    let future = tools::find(&catalog, "future_method").expect("future tool");
    assert_eq!(future.tier, Tier::Process);
    // It forwards arguments verbatim over the live connection.
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &["process"]);
    let output = server.call("future_method", json!({"anything": [1, 2]}));
    assert!(!output.is_error, "{}", output.structured);
    let recorded = mock.recorded();
    let (_, params) = recorded.iter().find(|(method, _)| method == "future_method").unwrap();
    assert_eq!(*params, json!({"anything": [1, 2]}));
}

#[test]
fn host_claimed_observe_method_stays_observe() {
    let catalog = tools::build_catalog(&mock_hello());
    let layout = tools::find(&catalog, "vivida_layout").expect("layout tool");
    assert_eq!(layout.tier, Tier::Observe);
    assert!(layout.description.contains("Host-provided"), "{}", layout.description);
}

#[test]
fn composites_and_alias_are_present() {
    let catalog = tools::build_catalog(&mock_hello());
    for name in [
        "capture",
        "drain_events",
        "headless_start",
        "headless_attach",
        "headless_stop",
        "headless_list",
    ] {
        assert!(tools::find(&catalog, name).is_some(), "{name} missing");
    }
    assert_eq!(tools::find(&catalog, "headless_list").unwrap().tier, Tier::Observe);
    assert!(tools::find(&catalog, "hello").is_none());
    let alias = tools::find(&catalog, "type_text").expect("type_text alias");
    assert_eq!(alias.tier, Tier::Input);
}

#[test]
fn live_catalog_replaces_fallback_on_connect() {
    let mock = MockServer::start();
    let server = common::test_server(&mock, &[]);
    assert!(server.connected());
    // The live mock advertises host methods the fallback cannot know.
    assert!(tools::find(server.catalog(), "vivida_layout").is_some());
    assert!(tools::find(server.catalog(), "future_method").is_some());
}

#[test]
fn annotations_mirror_tiers() {
    let catalog = tools::build_catalog(&mock_hello());
    let observe = tools::find(&catalog, "list_windows").unwrap().listing();
    assert_eq!(observe["annotations"]["readOnlyHint"], true);
    assert_eq!(observe["annotations"]["destructiveHint"], false);
    let input = tools::find(&catalog, "key").unwrap().listing();
    assert_eq!(input["annotations"]["readOnlyHint"], false);
    assert_eq!(input["annotations"]["destructiveHint"], false);
    let process = tools::find(&catalog, "quit").unwrap().listing();
    assert_eq!(process["annotations"]["destructiveHint"], true);
    for tool in [&observe, &input, &process] {
        assert_eq!(tool["annotations"]["openWorldHint"], false);
    }
}
