#![cfg(unix)]

// drain_events: cursors, replay, overflow gaps, and resubscription.

mod common;

use serde_json::json;

use common::MockServer;

#[test]
fn drain_replays_since_cursor_and_advances_it() {
    let mock = MockServer::start();
    for sequence in 10..=15 {
        mock.push_event(sequence, Some(3), "bell", json!({}));
    }
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("drain_events", json!({"cursor": 12, "timeout": "300ms"}));
    assert!(!output.is_error, "{}", output.structured);
    let sequences: Vec<u64> = output.structured["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["sequence"].as_u64().unwrap())
        .collect();
    assert_eq!(sequences, vec![13, 14, 15]);
    assert_eq!(output.structured["cursor"], 15);
    assert_eq!(output.structured["overflow"], false);
}

#[test]
fn live_events_arrive_within_the_timeout() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &[]);
    // First call subscribes from now; nothing is queued yet.
    let output = server.call("drain_events", json!({"timeout": "100ms"}));
    assert!(!output.is_error, "{}", output.structured);
    assert_eq!(output.structured["events"].as_array().unwrap().len(), 0);
    // A later event is delivered live on the still-open subscription.
    mock.push_event(21, Some(3), "bell", json!({}));
    let output = server.call("drain_events", json!({"timeout": "2s"}));
    assert!(!output.is_error, "{}", output.structured);
    assert_eq!(output.structured["events"].as_array().unwrap().len(), 1);
    assert_eq!(output.structured["cursor"], 21);
}

#[test]
fn overflow_is_a_visible_gap() {
    let mock = MockServer::start();
    mock.push_event(30, Some(3), "overflow", json!({"gap": {"from": 20, "to": 29}}));
    mock.push_event(31, Some(3), "bell", json!({}));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("drain_events", json!({"cursor": 29, "timeout": "300ms"}));
    assert!(!output.is_error, "{}", output.structured);
    assert_eq!(output.structured["overflow"], true);
    assert!(output.structured["note"].is_string(), "{}", output.structured);
    let kinds: Vec<String> = output.structured["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_owned())
        .collect();
    assert!(kinds.contains(&"overflow".to_owned()), "{kinds:?}");
}

#[test]
fn unknown_event_kinds_fail_fast() {
    let mock = MockServer::start();
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("drain_events", json!({"events": ["nope"], "timeout": "100ms"}));
    assert!(output.is_error);
    assert_eq!(output.structured["error"]["code"], "invalid_params");
}

#[test]
fn unsubscribe_resets_the_cursor() {
    let mock = MockServer::start();
    mock.push_event(40, Some(3), "bell", json!({}));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("drain_events", json!({"cursor": 39, "timeout": "300ms"}));
    assert_eq!(output.structured["cursor"], 40);
    let output = server.call("unsubscribe", json!({}));
    assert!(!output.is_error, "{}", output.structured);
    // After a reset, the next drain subscribes from now: the old event is gone.
    let output = server.call("drain_events", json!({"timeout": "100ms"}));
    assert!(!output.is_error, "{}", output.structured);
    assert_eq!(output.structured["events"].as_array().unwrap().len(), 0);
}

#[test]
fn one_shot_subscribe_collects_then_leaves() {
    let mock = MockServer::start();
    mock.push_event(50, Some(3), "bell", json!({}));
    mock.push_event(51, Some(3), "bell", json!({}));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("subscribe", json!({"since_event": 49, "timeout": "300ms"}));
    assert!(!output.is_error, "{}", output.structured);
    assert_eq!(output.structured["events"].as_array().unwrap().len(), 2);
    assert_eq!(output.structured["cursor"], 51);
    // The one-shot connection unsubscribed; the recorded stream shows it.
    let methods: Vec<String> = mock.recorded().iter().map(|(method, _)| method.clone()).collect();
    assert!(methods.contains(&"unsubscribe".to_owned()), "{methods:?}");
}

#[test]
fn events_accepts_a_single_kind_string() {
    let mock = MockServer::start();
    mock.push_event(60, Some(3), "bell", json!({}));
    mock.push_event(61, Some(3), "output", json!({}));
    let server = &mut common::test_server(&mock, &[]);
    let output =
        server.call("drain_events", json!({"cursor": 59, "events": "bell", "timeout": "300ms"}));
    assert!(!output.is_error, "{}", output.structured);
    let kinds: Vec<String> = output.structured["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, vec!["bell".to_owned()]);
    let bad = server.call("drain_events", json!({"events": 42, "timeout": "50ms"}));
    assert_eq!(bad.structured["error"]["code"], "invalid_args");
}
