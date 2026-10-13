//! Headless session lifecycle: start an ephemeral session, attach, stop.
//!
//! A headless session runs the complete Vivido runtime — terminal, input
//! encoders, Vivid presenter, offscreen renderer — with no window and no
//! compositor. It is the deterministic target for agent work: input cannot
//! reach the operator's visible workspace, and screenshots are real rendered
//! frames. Ephemeral sessions additionally tear themselves down instead of
//! leaking when the launcher is gone or clients disconnect.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::endpoint::validate_session_name;

/// How long to wait for the launcher to report the session as serving. The
/// parent does not exit until the daemon is actually serving, and reports a
/// timeout itself after 30 seconds — so 35 seconds here only trips on a hung
/// launcher, in which case the daemon may still come up on its own.
const START_TIMEOUT: Duration = Duration::from_secs(35);

/// Options for [`start`].
pub struct StartOptions {
    /// Session name, or `None` to generate a unique one.
    pub session: Option<String>,
    /// `--headless-size` value (`120x40` or `1280x720px`).
    pub size: Option<String>,
    /// Program for the first window instead of the default shell.
    pub program: Option<String>,
    /// Arguments for `program`.
    pub args: Vec<String>,
    /// Pass `--ephemeral` so the session tears itself down.
    pub ephemeral: bool,
}

/// A started session. The socket path is connection material: register it with
/// the [`crate::sanitize::Redactor`] and never emit it in tool output.
pub struct StartedSession {
    /// Live session name.
    pub session: String,
    /// Endpoint path for the new connection.
    pub socket: PathBuf,
}

/// Start a headless session and return its name plus endpoint path.
pub fn start(vivido_bin: &str, options: StartOptions) -> Result<StartedSession, String> {
    let session = match options.session {
        Some(name) => {
            validate_session_name(&name)?;
            name
        }
        None => generate_session_name(),
    };
    if let Some(size) = &options.size {
        validate_size(size)?;
    }
    let mut command = Command::new(vivido_bin);
    command.arg("--headless").arg("--session").arg(&session);
    if options.ephemeral {
        command.arg("--ephemeral");
    }
    if let Some(size) = &options.size {
        command.arg("--headless-size").arg(size);
    }
    if let Some(program) = &options.program {
        command.arg("--").arg(program).args(&options.args);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let child = command.spawn().map_err(|error| format!("cannot run `{vivido_bin}`: {error}"))?;

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    let output = receiver.recv_timeout(START_TIMEOUT).map_err(|_| {
        format!(
            "timed out waiting for `{vivido_bin}` to start session {session:?}; the daemon may \
             still be initializing its renderer — retry `headless_attach` for {session:?}"
        )
    })?;
    let output = output.map_err(|error| format!("cannot start session {session:?}: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cannot start session {session:?}: {}", first_line(&detail, 300)));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let socket = parse_socket_export(&stdout).ok_or_else(|| {
        "the launcher did not report an endpoint; the session may have failed to come up".to_owned()
    })?;
    Ok(StartedSession { session, socket: PathBuf::from(socket) })
}

/// Stop a session: graceful `quit` first (unless `force`), escalating to
/// `kill-session` when the graceful stop fails or the session stays up.
/// Every path verifies the endpoint actually went down. Returns how it stopped.
pub fn stop(
    vivido_bin: &str,
    target: &str,
    force: bool,
    graceful: impl FnOnce() -> Result<(), String>,
) -> Result<&'static str, String> {
    validate_session_name(target)?;
    if !force {
        match graceful() {
            Ok(()) if wait_down(target) => return Ok("quit"),
            Ok(()) => {
                eprintln!(
                    "headless_stop: graceful quit of {target:?} answered but the session stays up; escalating"
                );
            }
            Err(error) => {
                eprintln!(
                    "headless_stop: graceful quit of {target:?} failed ({error}); escalating"
                );
            }
        }
    }
    let output = Command::new(vivido_bin)
        .args(["kill-session", "--target", target])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run `{vivido_bin} kill-session`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot stop session {target:?}: {}",
            first_line(&String::from_utf8_lossy(&output.stderr), 300)
        ));
    }
    if wait_down(target) {
        Ok("kill-session")
    } else {
        Err(format!("session {target:?} is still accepting connections after kill-session"))
    }
}

/// Poll until the session's endpoint stops accepting connections (up to 5s).
/// An unresolvable or already-gone session counts as down.
fn wait_down(target: &str) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let up = crate::endpoint::resolve(None, Some(target))
            .map(|endpoint| crate::ipc::probe(&endpoint.path))
            .unwrap_or(false);
        if !up {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Generate a unique session name that cannot collide with a running session.
fn generate_session_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.subsec_nanos())
        .unwrap_or(0);
    format!("vivido-mcp-{}-{nanos:09}", std::process::id())
}

/// Validate a `--headless-size` value: `COLUMNSxLINES` or `WIDTHxHEIGHTpx`.
fn validate_size(size: &str) -> Result<(), String> {
    let pixels = size.strip_suffix("px").unwrap_or(size);
    let (first, second) = pixels
        .split_once('x')
        .ok_or_else(|| format!("invalid size {size:?}: expected 120x40 or 1280x720px"))?;
    let valid = first.parse::<u32>().is_ok_and(|value| value > 0)
        && second.parse::<u32>().is_ok_and(|value| value > 0);
    if valid {
        Ok(())
    } else {
        Err(format!("invalid size {size:?}: expected 120x40 or 1280x720px"))
    }
}

/// Parse the `VIVIDO_SOCKET=...; export VIVIDO_SOCKET` launcher report.
fn parse_socket_export(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("VIVIDO_SOCKET=") {
            let path = rest.split(';').next()?.trim();
            if !path.is_empty() {
                return Some(path.to_owned());
            }
        }
    }
    None
}

fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().next().unwrap_or("unknown error").trim();
    if line.is_empty() {
        return "unknown error".to_owned();
    }
    let mut clipped: String = line.chars().take(max_chars).collect();
    if clipped.len() < line.len() {
        clipped.push('…');
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_launcher_exports() {
        let stdout = "VIVIDO_SOCKET=/run/user/1000/vivido/session-abc.sock; export VIVIDO_SOCKET\n\
             VIVIDO_SESSION=build; export VIVIDO_SESSION\n";
        assert_eq!(
            parse_socket_export(stdout).as_deref(),
            Some("/run/user/1000/vivido/session-abc.sock")
        );
        assert_eq!(parse_socket_export("nothing here\n"), None);
    }

    #[test]
    fn validates_sizes() {
        for size in ["120x40", "1280x720px"] {
            assert!(validate_size(size).is_ok(), "{size}");
        }
        for size in ["120", "x40", "120x", "0x40", "huge", "120x40pt", "80X24"] {
            assert!(validate_size(size).is_err(), "{size}");
        }
    }

    #[test]
    fn missing_sessions_count_as_down() {
        assert!(wait_down("definitely-not-running-xyz"));
    }

    #[test]
    fn generated_names_are_valid() {
        assert!(validate_session_name(&generate_session_name()).is_ok());
    }
}
