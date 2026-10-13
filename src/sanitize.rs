//! Redaction of endpoint material from tool output.
//!
//! The socket path, `VIVIDO_SOCKET`, and session tokens must never appear in tool
//! output: model-visible text is routinely copied into logs, transcripts, and chats.
//! Every tool result passes through [`Redactor`] before it reaches the MCP client.
//! Operator-facing diagnostics on stderr are unaffected — the operator already owns
//! the endpoint.

use serde_json::Value;

/// Substring patterns that mark a value as endpoint material even when the exact
/// socket path was not registered (a second session's socket, a pipe path on Windows).
const SOCKET_MARKERS: &[&str] = &[".sock", "vivido-session-", "VIVIDO_SOCKET", r"\\.\pipe\"];

/// JSON keys whose values are always secrets, matched case-insensitively by substring.
const SECRET_KEYS: &[&str] =
    &["token", "secret", "authenticator", "capability", "password", "vivid_root"];

/// Replacement text for redacted material.
pub const REDACTED: &str = "[redacted]";

/// Redacts endpoint paths and secret material from JSON tool output.
#[derive(Debug, Default)]
pub struct Redactor {
    needles: Vec<String>,
}

impl Redactor {
    /// An empty redactor. Register every socket path the server touches.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one exact string (a socket path, an env value) for redaction.
    pub fn add(&mut self, needle: &str) {
        let trimmed = needle.trim();
        if !trimmed.is_empty() && !self.needles.iter().any(|known| known == trimmed) {
            self.needles.push(trimmed.to_owned());
        }
    }

    /// Redact `value` in place, returning it for call chaining.
    pub fn redact(&self, mut value: Value) -> Value {
        self.redact_value(&mut value, None);
        value
    }

    fn redact_value(&self, value: &mut Value, key: Option<&str>) {
        match value {
            Value::String(text) => {
                if key.is_some_and(is_secret_key) {
                    *text = REDACTED.to_owned();
                    return;
                }
                // Registered paths are replaced in place so surrounding
                // context survives ("connected to [redacted] ok").
                for needle in &self.needles {
                    if text.contains(needle) {
                        *text = text.replace(needle, REDACTED);
                    }
                }
                // Anything still socket-shaped is unregistered endpoint
                // material (another session's socket, a pipe path): redact all.
                if SOCKET_MARKERS.iter().any(|marker| text.contains(marker)) {
                    *text = REDACTED.to_owned();
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.redact_value(item, None);
                }
            }
            Value::Object(map) => {
                for (member_key, member) in map.iter_mut() {
                    self.redact_value(member, Some(member_key));
                }
            }
            _ => {}
        }
    }
}

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|secret| lower.contains(secret))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn redacts_registered_socket_paths() {
        let mut redactor = Redactor::new();
        redactor.add("/run/user/1000/vivido/session-abc.sock");
        let value = redactor.redact(json!({
            "socket": "/run/user/1000/vivido/session-abc.sock",
            "note": "connected to /run/user/1000/vivido/session-abc.sock ok",
            "session": "build",
        }));
        assert_eq!(value["socket"], REDACTED);
        assert_eq!(value["note"], "connected to [redacted] ok");
        assert_eq!(value["session"], "build");
    }

    #[test]
    fn redacts_unregistered_socket_like_strings() {
        let redactor = Redactor::new();
        let value = redactor.redact(json!({
            "path": "/tmp/other/session-deadbeef.sock",
            "pipe": "\\\\.\\pipe\\vivido-session-deadbeef",
        }));
        assert_eq!(value["path"], REDACTED);
        assert_eq!(value["pipe"], REDACTED);
    }

    #[test]
    fn redacts_secret_keys_and_keeps_the_rest() {
        let redactor = Redactor::new();
        let value = redactor.redact(json!({
            "session_token": "abc123",
            "channel_authenticator": "zzz",
            "remote_path": "/home/u/firmware.bin",
            "window_id": 3,
        }));
        assert_eq!(value["session_token"], REDACTED);
        assert_eq!(value["channel_authenticator"], REDACTED);
        assert_eq!(value["remote_path"], "/home/u/firmware.bin");
        assert_eq!(value["window_id"], 3);
    }
}
