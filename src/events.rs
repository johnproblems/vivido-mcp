//! Event polling without MCP notifications.
//!
//! Clients rarely act on mid-turn notifications, so Vivido events are pulled,
//! not pushed. [`DrainState`] keeps one server-side subscription open across
//! `drain_events` calls on its own connection and returns events since a
//! cursor; when the connection drops it resubscribes with `since_event` replay.
//! An `overflow` event — the server's admission that it collapsed dropped
//! detail — is passed through as a visible gap, never swallowed.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::ipc::{Connection, Failure, Frame, IpcError};

/// What a subscription listens to. A filter change resubscribes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrainFilter {
    /// One window, or `None` with `all` for every window plus lifecycle events.
    pub window_id: Option<u64>,
    /// Every window plus process lifecycle events.
    pub all: bool,
    /// Event kinds to keep; empty means all kinds.
    pub events: Vec<String>,
}

impl DrainFilter {
    fn signature(&self) -> String {
        let mut kinds = self.events.clone();
        kinds.sort();
        format!("{:?}:{}:{}", self.window_id, self.all, kinds.join(","))
    }

    fn subscribe_params(&self, since_event: Option<u64>) -> Value {
        json!({
            "window_id": self.window_id,
            "all": self.all,
            "events": self.events,
            "since_event": since_event,
        })
    }
}

/// One persistent server-side subscription plus its resume cursor.
#[derive(Default)]
pub struct DrainState {
    conn: Option<Connection>,
    endpoint: Option<PathBuf>,
    filter: Option<String>,
    cursor: Option<u64>,
    /// Replay events that arrived before the subscribe acknowledgement.
    pending: VecDeque<Value>,
}

impl DrainState {
    /// Poll for events: buffered and queued ones first, then live ones until `timeout`.
    pub fn drain(
        &mut self,
        endpoint: &Path,
        filter: DrainFilter,
        cursor_override: Option<u64>,
        limit: usize,
        timeout: Duration,
        allowed_kinds: Option<&[String]>,
    ) -> Result<Value, Failure> {
        if let Some(allowed) = allowed_kinds {
            for kind in &filter.events {
                if !allowed.contains(kind) {
                    return Err(Failure::Server(IpcError {
                        code: "invalid_params".to_owned(),
                        message: format!(
                            "unknown event kind {kind:?}; this endpoint advertises: {}",
                            allowed.join(", ")
                        ),
                        data: None,
                    }));
                }
            }
        }
        if let Some(cursor) = cursor_override {
            self.cursor = Some(cursor);
            self.pending.clear();
        }
        let signature = filter.signature();
        let changed = self.endpoint.as_deref() != Some(endpoint)
            || self.filter.as_deref() != Some(&signature)
            || self.conn.is_none();
        if changed {
            self.subscribe(endpoint, &filter, &signature)?;
        }
        let mut events = Vec::new();
        let mut overflow = false;
        while events.len() < limit {
            if let Some(buffered) = self.pending.pop_front() {
                if buffered.get("type").and_then(Value::as_str) == Some("overflow") {
                    overflow = true;
                }
                if let Some(sequence) = buffered.get("sequence").and_then(Value::as_u64) {
                    self.cursor = Some(sequence);
                }
                events.push(buffered);
            } else {
                break;
            }
        }
        let deadline = Instant::now() + timeout;
        loop {
            if events.len() >= limit {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let conn = self.conn.as_mut().expect("subscribed above");
            match conn.recv_frame(Some(remaining)) {
                Ok(Frame::Event { event_sequence, window_id, event, .. }) => {
                    if event.get("type").and_then(Value::as_str) == Some("overflow") {
                        overflow = true;
                    }
                    self.cursor = Some(event_sequence);
                    events.push(json!({
                        "sequence": event_sequence,
                        "window_id": window_id,
                        "type": event.get("type").cloned().unwrap_or(Value::Null),
                        "data": event.get("data").cloned().unwrap_or(Value::Null),
                    }));
                }
                // No request is outstanding; a stray response is ignored.
                Ok(Frame::Response { .. }) => {}
                Err(Failure::Transport(message)) if message.contains("timed out") => break,
                Err(error) => {
                    // The connection died mid-poll. Drop it so the next call
                    // resubscribes with `since_event` replay from the cursor;
                    // return what arrived so far rather than failing the call.
                    self.conn = None;
                    self.endpoint = None;
                    self.filter = None;
                    if events.is_empty() {
                        return Err(error);
                    }
                    break;
                }
            }
        }
        Ok(json!({
            "events": events,
            "cursor": self.cursor,
            "overflow": overflow,
            "resubscribed": changed,
            "note": overflow.then_some(
                "An overflow gap collapsed dropped detail; recover with `inspect` plus `get_grid`."
            ),
        }))
    }

    /// Drop the subscription and cursor. The next `drain` starts from now.
    pub fn reset(&mut self) -> Value {
        self.conn = None;
        self.endpoint = None;
        self.filter = None;
        self.cursor = None;
        self.pending.clear();
        json!({ "reset": true })
    }

    fn subscribe(
        &mut self,
        endpoint: &Path,
        filter: &DrainFilter,
        signature: &str,
    ) -> Result<(), Failure> {
        let mut conn = Connection::connect(endpoint)?;
        // A fresh subscription replays from the cursor; a cursor-less first call
        // subscribes from now.
        let id = conn.send_request("subscribe", filter.subscribe_params(self.cursor))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Failure::Transport(
                    "timed out waiting for subscribe acknowledgement".to_owned(),
                ));
            }
            match conn.recv_frame(Some(remaining))? {
                Frame::Response { id: reply, ok: true, .. } if reply == id => break,
                Frame::Response { id: reply, ok: false, error, .. } if reply == id => {
                    return Err(Failure::Server(error.unwrap_or(IpcError {
                        code: "invalid_response".to_owned(),
                        message: "subscribe failed without an error body".to_owned(),
                        data: None,
                    })));
                }
                // Replay may precede the acknowledgement; buffer it for the drain loop.
                Frame::Event { event_sequence, window_id, event, .. } => {
                    self.pending.push_back(json!({
                        "sequence": event_sequence,
                        "window_id": window_id,
                        "type": event.get("type").cloned().unwrap_or(Value::Null),
                        "data": event.get("data").cloned().unwrap_or(Value::Null),
                    }));
                }
                Frame::Response { .. } => {}
            }
        }
        self.conn = Some(conn);
        self.endpoint = Some(endpoint.to_owned());
        self.filter = Some(signature.to_owned());
        Ok(())
    }
}

/// One-shot collection for the `subscribe` tool: subscribe on a fresh
/// connection, gather events until `limit` or `timeout`, unsubscribe, close.
pub fn collect_once(
    endpoint: &Path,
    filter: DrainFilter,
    since_event: Option<u64>,
    limit: usize,
    timeout: Duration,
) -> Result<Value, Failure> {
    let mut conn = Connection::connect(endpoint)?;
    let id = conn.send_request("subscribe", filter.subscribe_params(since_event))?;
    let mut subscription_id = None;
    let mut events = Vec::new();
    let mut overflow = false;
    let mut cursor = since_event;
    let deadline = Instant::now() + timeout;
    loop {
        if events.len() >= limit {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match conn.recv_frame(Some(remaining)) {
            Ok(Frame::Response { id: reply, ok: true, result, .. }) if reply == id => {
                subscription_id = result
                    .as_ref()
                    .and_then(|value| value.get("subscription_id"))
                    .and_then(Value::as_u64);
            }
            Ok(Frame::Response { id: reply, ok: false, error, .. }) if reply == id => {
                return Err(Failure::Server(error.unwrap_or(IpcError {
                    code: "invalid_response".to_owned(),
                    message: "subscribe failed without an error body".to_owned(),
                    data: None,
                })));
            }
            Ok(Frame::Event { event_sequence, window_id, event, .. }) => {
                if event.get("type").and_then(Value::as_str) == Some("overflow") {
                    overflow = true;
                }
                cursor = Some(event_sequence);
                events.push(json!({
                    "sequence": event_sequence,
                    "window_id": window_id,
                    "type": event.get("type").cloned().unwrap_or(Value::Null),
                    "data": event.get("data").cloned().unwrap_or(Value::Null),
                }));
            }
            Ok(Frame::Response { .. }) => {}
            Err(Failure::Transport(message)) if message.contains("timed out") => break,
            Err(error) => {
                if events.is_empty() {
                    return Err(error);
                }
                break;
            }
        }
    }
    if let Some(subscription) = subscription_id {
        // Best effort: closing the connection cancels the subscription anyway.
        let _ = conn.request("unsubscribe", json!({ "subscription_id": subscription }));
    }
    Ok(json!({
        "events": events,
        "cursor": cursor,
        "overflow": overflow,
        "note": overflow.then_some(
            "An overflow gap collapsed dropped detail; recover with `inspect` plus `get_grid`."
        ),
    }))
}
