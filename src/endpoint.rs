//! Endpoint discovery for Vivido and Vivida instances.
//!
//! Mirrors `vivido msg` resolution order so this server reaches the same instance
//! the CLI would:
//!
//! 1. `--socket PATH`.
//! 2. `--target NAME`, else inherited `VIVIDO_SESSION`, else `VIVIDA_TARGET`. A name
//!    that is not running is an error, never a silent fall-through.
//! 3. Inherited `VIVIDO_SOCKET`, if it still connects.
//! 4. The only live headless session, when there is exactly one.
//! 5. On Unix, the newest live windowed instance on the current display.
//!
//! Every candidate is verified by actually connecting, and owner-only filesystem
//! checks match the CLI: symlinks are refused, sockets and registries must belong
//! to this user, and registries must be closed to group and world.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Environment variable naming the headless session a client should reach.
pub const VIVIDO_SESSION_ENV: &str = "VIVIDO_SESSION";
/// Vivida-side spelling of [`VIVIDO_SESSION_ENV`], honored by the same discovery.
pub const VIVIDA_TARGET_ENV: &str = "VIVIDA_TARGET";
/// Environment variable carrying an inherited endpoint path.
pub const VIVIDO_SOCKET_ENV: &str = "VIVIDO_SOCKET";

/// One resolved endpoint: a local socket path (Unix) or pipe path (Windows).
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// Local endpoint path. Never include this in tool output.
    pub path: PathBuf,
    /// Short human label for stderr diagnostics (the session name when known).
    pub label: String,
}

/// Resolve which endpoint to connect to.
pub fn resolve(socket: Option<PathBuf>, target: Option<&str>) -> Result<Endpoint, String> {
    if let Some(path) = socket {
        check_owned_socket(&path)?;
        return Ok(Endpoint { path, label: "explicit --socket".to_owned() });
    }

    if let Some(name) = target
        .map(str::to_owned)
        .or_else(|| env::var(VIVIDO_SESSION_ENV).ok())
        .or_else(|| env::var(VIVIDA_TARGET_ENV).ok())
    {
        let registry = registered_instance(&name)
            .map_err(|_| format!("no running Vivido instance named {name:?}"))?;
        check_owned_socket(&registry.socket)
            .map_err(|_| format!("no running Vivido instance named {name:?}"))?;
        if !probe(&registry.socket) {
            return Err(format!("no running Vivido instance named {name:?}"));
        }
        return Ok(Endpoint { path: registry.socket, label: format!("target {name:?}") });
    }

    if let Ok(path) = env::var(VIVIDO_SOCKET_ENV) {
        let candidate = PathBuf::from(&path);
        if check_owned_socket(&candidate).is_ok() && probe(&candidate) {
            return Ok(Endpoint { path: candidate, label: "inherited VIVIDO_SOCKET".to_owned() });
        }
    }

    let sessions = list_registries().unwrap_or_default();
    let live: Vec<SessionRegistry> =
        sessions.into_iter().filter(|session| probe(&session.socket)).collect();
    if let [only] = live.as_slice()
        && check_owned_socket(&only.socket).is_ok()
    {
        return Ok(Endpoint {
            path: only.socket.clone(),
            label: format!("only live session {:?}", only.name),
        });
    }

    #[cfg(unix)]
    if let Some(path) = newest_windowed_socket()? {
        return Ok(Endpoint { path, label: "newest windowed instance".to_owned() });
    }

    if live.len() > 1 {
        let mut names: Vec<String> = live.iter().map(|session| session.name.clone()).collect();
        names.sort();
        return Err(format!(
            "multiple Vivido sessions found: {}; specify --target NAME or --socket PATH, \
             or set {VIVIDO_SESSION_ENV}",
            names.join(", ")
        ));
    }
    Err(format!(
        "no Vivido endpoint found; start Vivido, launch a headless session, or pass \
         --socket PATH / --target NAME (or set {VIVIDO_SESSION_ENV})"
    ))
}

/// Quick liveness probe: the endpoint accepts a connection.
fn probe(path: &Path) -> bool {
    crate::ipc::probe(path)
}

/// Published same-user rendezvous for one running instance.
#[derive(Clone, Debug)]
struct SessionRegistry {
    name: String,
    socket: PathBuf,
    headless: bool,
    pid: Option<u64>,
}

/// One registered session, shaped for tool output. The socket path is
/// deliberately absent: names are enough to attach or stop.
#[derive(Clone, Debug)]
pub struct SessionSummary {
    /// Session name for `headless_attach` / `headless_stop`.
    pub name: String,
    /// Whether the session runs headless.
    pub headless: bool,
    /// Daemon process id, when the registry records one.
    pub pid: Option<u64>,
    /// Whether the endpoint accepts a connection right now.
    pub live: bool,
}

/// Every registered session with liveness. Never fails: without a registry
/// there are simply no sessions.
pub fn list_sessions() -> Vec<SessionSummary> {
    runtime_root().map(|root| list_sessions_in_root(&root)).unwrap_or_default()
}

fn list_sessions_in_root(root: &Path) -> Vec<SessionSummary> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let is_registry = path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|file| file.starts_with("session-") && file.ends_with(".json"));
        if !is_registry {
            continue;
        }
        if let Ok(registry) = read_registry_file(&path) {
            sessions.push(SessionSummary {
                name: registry.name,
                headless: registry.headless,
                pid: registry.pid,
                live: probe(&registry.socket),
            });
        }
    }
    sessions.sort_by(|left, right| left.name.cmp(&right.name));
    sessions
}

/// Read and validate the registry for one named instance.
fn registered_instance(name: &str) -> Result<SessionRegistry, String> {
    validate_session_name(name)?;
    let paths = SessionPaths::for_session(name)?;
    let registry = read_registry_file(&paths.registry)?;
    if registry.name != name {
        return Err(format!("registry for {name:?} names a different instance"));
    }
    Ok(registry)
}

/// Every live-looking registered session, reaping none (a client never deletes rendezvous).
fn list_registries() -> Result<Vec<SessionRegistry>, String> {
    let root = runtime_root()?;
    let entries = fs::read_dir(&root).map_err(|_| "no session registry".to_owned())?;
    let mut sessions = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let is_registry = path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|file| file.starts_with("session-") && file.ends_with(".json"));
        if !is_registry {
            continue;
        }
        if let Ok(registry) = read_registry_file(&path) {
            sessions.push(registry);
        }
    }
    Ok(sessions)
}

fn read_registry_file(path: &Path) -> Result<SessionRegistry, String> {
    require_private_file(path)?;
    let bytes = fs::read(path).map_err(|error| format!("cannot read registry: {error}"))?;
    if bytes.len() > 16 * 1024 {
        return Err("registry file exceeds 16 KiB".to_owned());
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "registry file is not valid JSON".to_owned())?;
    Ok(SessionRegistry {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "registry is missing its name".to_owned())?
            .to_owned(),
        headless: value.get("headless").and_then(Value::as_bool).unwrap_or(false),
        pid: value.get("pid").and_then(Value::as_u64),
        socket: value
            .get("socket")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| "registry is missing its socket".to_owned())?,
    })
}

/// On Unix, the newest live windowed instance on the current display.
#[cfg(unix)]
fn newest_windowed_socket() -> Result<Option<PathBuf>, String> {
    let mut candidates = Vec::new();
    let prefix = socket_prefix();
    let entries = fs::read_dir(socket_dir()).map_err(|error| error.to_string())?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let matches = path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|file| file.starts_with(&prefix) && file.ends_with(".sock"));
        if matches {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates.reverse();
    for path in candidates {
        if check_owned_socket(&path).is_err() {
            continue;
        }
        if probe(&path) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Directory holding windowed-instance sockets.
#[cfg(all(unix, not(target_os = "macos")))]
fn socket_dir() -> PathBuf {
    if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(path).join("vivido");
        if is_private_dir(&dir) {
            return dir;
        }
    }
    env::temp_dir()
}

/// macOS keeps windowed sockets in the temporary directory.
#[cfg(target_os = "macos")]
fn socket_dir() -> PathBuf {
    env::temp_dir()
}

/// File prefix matching sockets on the current display server.
#[cfg(all(unix, not(target_os = "macos")))]
fn socket_prefix() -> String {
    let display = env::var("WAYLAND_DISPLAY").or_else(|_| env::var("DISPLAY")).unwrap_or_default();
    format!("Vivido-{}", display.replace('/', "-"))
}

/// File prefix matching sockets on macOS.
#[cfg(target_os = "macos")]
fn socket_prefix() -> String {
    String::from("Vivido")
}

/// Refuse anything but an owner-held non-symlink socket path.
fn check_owned_socket(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| format!("endpoint {} does not exist", path.display()))?;
        let owner = effective_uid();
        if metadata.file_type().is_symlink() || metadata.uid() != owner {
            return Err(format!("endpoint {} is not owned by uid {owner}", path.display()));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        // Named pipes carry an owner-and-SYSTEM-only DACL plus peer SID checks on
        // both ends; the path itself is `\\.\pipe\...`, not a filesystem object.
        if path.as_os_str().is_empty() {
            return Err("empty endpoint path".to_owned());
        }
        Ok(())
    }
}

/// A registry file must be a regular owner-owned file with no group/world bits.
#[cfg(unix)]
fn require_private_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err(format!("refusing unsafe registry file {}", path.display()));
    }
    Ok(())
}

#[cfg(windows)]
fn require_private_file(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path).is_err() {
        return Err(format!("missing registry file {}", path.display()));
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_private_dir(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        !metadata.file_type().is_symlink()
            && metadata.is_dir()
            && metadata.uid() == effective_uid()
            && metadata.mode() & 0o077 == 0
    })
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Rendezvous file for one session: `session-<hash>.json` where `<hash>` is the
/// first 16 bytes of sha256(name) in hex. The registry names the live endpoint,
/// which may differ from the default socket when an explicit `--socket` was used.
struct SessionPaths {
    registry: PathBuf,
}

impl SessionPaths {
    fn for_session(name: &str) -> Result<Self, String> {
        let root = runtime_root()?;
        session_paths_in_root(name, &root)
    }
}

/// First 16 bytes of sha256(name), hex-encoded: the rendezvous filename fragment.
fn session_hash(name: &str) -> String {
    hex::encode(&Sha256::digest(name.as_bytes())[..16])
}

fn session_paths_in_root(name: &str, root: &Path) -> Result<SessionPaths, String> {
    validate_session_name(name)?;
    let registry = root.join(format!("session-{}.json", session_hash(name)));
    Ok(SessionPaths { registry })
}

/// Validate a session name before it becomes part of a filename or pipe name.
pub fn validate_session_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 || name.starts_with('.') {
        return Err(format!(
            "invalid session name {name:?}: 1-64 characters, must not start with '.'"
        ));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_')
    {
        return Err(format!(
            "invalid session name {name:?}: ASCII letters, digits, '.', '-', '_' only"
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn runtime_root() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(path);
        return Ok(path.join("vivido"));
    }
    Ok(PathBuf::from(format!("/tmp/vivido-{}", effective_uid())))
}

#[cfg(windows)]
fn runtime_root() -> Result<PathBuf, String> {
    if let Some(root) = env::var_os("VIVIDO_RUNTIME_DIR") {
        return Ok(PathBuf::from(root));
    }
    env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join("vivido").join("sessions"))
        .ok_or_else(|| "LOCALAPPDATA is unavailable".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_hash_matches_upstream_layout() {
        // sha256("build") starts with 44575cf5...; the rendezvous file is derived
        // from it, so a pinned constant catches any drift in the derivation.
        assert_eq!(session_hash("build"), "44575cf5b28512d75644bf54a517dcef");
        let paths = session_paths_in_root("build", Path::new("/run/user/1000/vivido")).unwrap();
        assert_eq!(
            paths.registry,
            PathBuf::from("/run/user/1000/vivido/session-44575cf5b28512d75644bf54a517dcef.json")
        );
    }

    #[test]
    fn rejects_bad_session_names() {
        assert!(validate_session_name("build-1_ok.x").is_ok());
        for bad in ["", ".hidden", "../escape", "has space", "semi;colon", &"n".repeat(65)] {
            assert!(validate_session_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn explicit_target_never_falls_through() {
        let error = resolve(None, Some("definitely-not-running-xyz")).unwrap_err();
        assert!(error.contains("definitely-not-running-xyz"), "{error}");
    }
}

#[cfg(all(test, unix))]
mod listing_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn lists_sessions_with_liveness() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let live_sock = dir.path().join("live.sock");
        let _listener = UnixListener::bind(&live_sock).unwrap();
        let write_reg = |file: &str, value: Value| {
            let path = dir.path().join(file);
            std::fs::write(&path, value.to_string()).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        write_reg(
            "session-aaa.json",
            serde_json::json!({
                "name": "a-live",
                "socket": live_sock.to_string_lossy(),
                "headless": true,
                "pid": 123,
            }),
        );
        write_reg(
            "session-bbb.json",
            serde_json::json!({"name": "b-dead", "socket": "/nonexistent/x.sock"}),
        );
        std::fs::write(dir.path().join("other.json"), "{}").unwrap();

        let sessions = list_sessions_in_root(dir.path());
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "a-live");
        assert!(sessions[0].live);
        assert!(sessions[0].headless);
        assert_eq!(sessions[0].pid, Some(123));
        assert_eq!(sessions[1].name, "b-dead");
        assert!(!sessions[1].live);
        assert!(!sessions[1].headless);
    }
}
