#![cfg(unix)]

// Real headless coverage: drive `vivido --headless --ephemeral` end to end.
//
// These tests need the real binary (`VIVIDO_BIN` or `vivido` on PATH with a
// wgpu adapter, hardware or llvmpipe). They are ignored by default and run in
// CI's headless job:
//
//   VIVIDO_BIN=/path/to/vivido cargo test --test headless_real -- --ignored

use serde_json::json;

fn vivido_bin() -> Option<String> {
    if let Ok(bin) = std::env::var("VIVIDO_BIN") {
        return Some(bin);
    }
    let output =
        std::process::Command::new("sh").args(["-c", "command -v vivido"]).output().ok()?;
    if output.status.success() { Some("vivido".to_owned()) } else { None }
}

fn process_server() -> Option<vivido_mcp::server::Server> {
    let bin = vivido_bin()?;
    let exe = std::path::Path::new(&bin);
    if bin.contains('/') && !exe.exists() {
        return None;
    }
    Some(vivido_mcp::server::Server::new(vivido_mcp::server::ServerConfig {
        allowance: vivido_mcp::tiers::parse_allow(&["process".to_owned()]).unwrap(),
        socket: None,
        target: None,
        vivido_bin: bin,
        default_max_dim: 1280,
    }))
}

#[test]
#[ignore]
fn headless_session_runs_ls_and_reads_the_result() {
    let server = &mut process_server().expect("vivido binary available");
    let session = format!("mcp-test-{}", std::process::id());
    let started = server.call("headless_start", json!({"session": session}));
    assert!(!started.is_error, "{}", started.structured);
    assert_eq!(started.structured["session"], session);
    assert_eq!(started.structured["size"], "1280x720px");
    assert_eq!(started.structured["ephemeral"], true);
    let listed = server.call("headless_list", json!({}));
    assert!(!listed.is_error, "{}", listed.structured);
    let names: Vec<String> = listed.structured["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|session| session["name"].as_str().map(str::to_owned))
        .collect();
    assert!(names.contains(&session), "{names:?}");

    let windows = server.call("list_windows", json!({}));
    assert!(!windows.is_error, "{}", windows.structured);
    let window_id = windows.structured["windows"][0]["window_id"].as_u64().unwrap();

    let typed = server.call("typing", json!({"window_id": window_id, "text": "echo hello-mcp\n"}));
    // Note: typing never appends Enter; the newline above is literal input.
    assert!(!typed.is_error, "{}", typed.structured);
    let waited = server.call(
        "wait_output",
        json!({"window_id": window_id, "pattern": "hello-mcp", "timeout": "30s"}),
    );
    assert!(!waited.is_error, "{}", waited.structured);

    let shot = server.call("screenshot", json!({"window_id": window_id}));
    assert!(!shot.is_error, "{}", shot.structured);
    assert!(shot.content.iter().any(|block| block["type"] == "image"));

    let stopped = server.call("headless_stop", json!({"target": session}));
    assert!(!stopped.is_error, "{}", stopped.structured);
    assert_eq!(stopped.structured["stopped"], true);
}
