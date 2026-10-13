#![cfg(unix)]

// Screenshots as images: content blocks, no paths, temp cleanup, crop, downscale.

mod common;

use std::sync::Arc;

use base64::Engine as _;
use serde_json::json;

use common::MockServer;

fn decode_image(output: &vivido_mcp::server::ToolOutput, expected_mime: &str) -> image::RgbaImage {
    let block =
        output.content.iter().find(|block| block["type"] == "image").expect("image content block");
    assert_eq!(block["mimeType"], expected_mime);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(block["data"].as_str().unwrap())
        .expect("base64 png");
    image::load_from_memory(&bytes).expect("decode png").to_rgba8()
}

fn screenshot_handler(mock: &MockServer, width: u32, height: u32, cell: (u32, u32)) {
    let path = mock.write_png(width, height);
    let result = mock.screenshot_result(&path, 3, cell, (0, 0));
    mock.on("screenshot", Arc::new(move |_| Ok(result.clone())));
}

#[test]
fn screenshot_returns_an_image_and_no_path() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100, (10, 10));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("screenshot", json!({"window_id": 3}));
    assert!(!output.is_error, "{}", output.structured);
    let image = decode_image(&output, "image/png");
    // 200x100 is under the default cap: untouched.
    assert_eq!((image.width(), image.height()), (200, 100));
    // No file path anywhere in the output.
    let text = output.structured.to_string();
    assert!(!text.contains("path"), "{text}");
    assert!(!text.contains(".png"), "{text}");
    assert_eq!(output.structured["window_id"], 3);
    assert_eq!(output.structured["frame_sequence"], 42);
    // The temp file is deleted afterwards.
    let shots: Vec<_> = std::fs::read_dir(mock.socket.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .collect();
    assert!(shots.is_empty(), "temp PNG was not deleted: {shots:?}");
}

#[test]
fn crop_selects_exact_cell_pixels() {
    let mock = MockServer::start();
    // 200x100, cell 10x10: crop col 2, row 1, 4x2 cells ->
    // pixels x=20, y=10, w=40, h=20 — inside the red top-left quadrant.
    screenshot_handler(&mock, 200, 100, (10, 10));
    let server = &mut common::test_server(&mock, &[]);
    let output =
        server.call("screenshot", json!({"crop": {"col": 2, "row": 1, "cols": 4, "rows": 2}}));
    assert!(!output.is_error, "{}", output.structured);
    let image = decode_image(&output, "image/png");
    assert_eq!((image.width(), image.height()), (40, 20));
    assert_eq!(
        output.structured["crop_pixels"],
        json!({"x": 20, "y": 10, "width": 40, "height": 20})
    );
    // Top-left quadrant is red.
    assert_eq!(*image.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
    assert_eq!(*image.get_pixel(39, 19), image::Rgba([255, 0, 0, 255]));
}

#[test]
fn crop_honors_padding() {
    let mock = MockServer::start();
    let path = mock.write_png(200, 100);
    let result = mock.screenshot_result(&path, 3, (10, 20), (5, 3));
    mock.on("screenshot", Arc::new(move |_| Ok(result.clone())));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call(
        "screenshot",
        json!({"crop": {"col": 1, "row": 2, "cols": 3, "rows": 2}, "max_dim": 4096}),
    );
    assert!(!output.is_error, "{}", output.structured);
    // x = 5 + 1*10 = 15, y = 3 + 2*20 = 43, w = 30, h = 40.
    assert_eq!(
        output.structured["crop_pixels"],
        json!({"x": 15, "y": 43, "width": 30, "height": 40})
    );
}

#[test]
fn large_screenshots_downscale_by_default() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 2000, 1000, (10, 10));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("screenshot", json!({}));
    assert!(!output.is_error, "{}", output.structured);
    let image = decode_image(&output, "image/png");
    assert_eq!((image.width(), image.height()), (1280, 640));
    assert_eq!(output.structured["downscaled_from"], json!({"width": 2000, "height": 1000}));
}

#[test]
fn crop_outside_the_capture_fails_cleanly() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100, (10, 10));
    let server = &mut common::test_server(&mock, &[]);
    let output =
        server.call("screenshot", json!({"crop": {"col": 500, "row": 0, "cols": 1, "rows": 1}}));
    assert!(output.is_error);
    assert_eq!(output.structured["error"]["code"], "image_error");
}

#[test]
fn capture_waits_then_returns_an_image() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100, (10, 10));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call(
        "capture",
        json!({"window_id": 3, "after_frame": 41, "stable": true, "quiet": "100ms"}),
    );
    assert!(!output.is_error, "{}", output.structured);
    decode_image(&output, "image/png");
    assert_eq!(output.structured["waited_frame"], true);
    assert_eq!(output.structured["waited_stable"], true);
    let recorded = mock.recorded();
    let methods: Vec<&str> = recorded.iter().map(|(method, _)| method.as_str()).collect();
    assert!(methods.contains(&"wait_frame"), "{methods:?}");
    assert!(methods.contains(&"wait_screen_stable"), "{methods:?}");
    assert!(methods.contains(&"screenshot"), "{methods:?}");
}

#[test]
fn jpeg_format_returns_smaller_viewable_image() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 400, 300, (10, 10));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("screenshot", json!({"format": "jpeg", "quality": 70}));
    assert!(!output.is_error, "{}", output.structured);
    let block = output.content.iter().find(|block| block["type"] == "image").expect("image block");
    assert_eq!(block["mimeType"], "image/jpeg");
    assert_eq!(output.structured["mime"], "image/jpeg");
    assert_eq!(output.structured["format"], "jpeg");
    let image = decode_image(&output, "image/jpeg");
    assert_eq!((image.width(), image.height()), (400, 300));
    assert!(output.structured["bytes"].as_u64().unwrap() > 1000);
    screenshot_handler(&mock, 400, 300, (10, 10));
    let bad = server.call("screenshot", json!({"format": "gif"}));
    assert_eq!(bad.structured["error"]["code"], "image_error");
}

#[test]
fn metadata_reports_size_and_clamping() {
    let mock = MockServer::start();
    screenshot_handler(&mock, 200, 100, (10, 10));
    let server = &mut common::test_server(&mock, &[]);
    let output = server.call("screenshot", json!({}));
    assert_eq!(output.structured["mime"], "image/png");
    assert_eq!(output.structured["max_dim"], 1280);
    assert!(output.structured["bytes"].as_u64().unwrap() > 100);
    assert_eq!(output.structured["crop_clamped"], false);
    // A crop running past the edge clamps and says so.
    screenshot_handler(&mock, 200, 100, (10, 10));
    let output =
        server.call("screenshot", json!({"crop": {"col": 18, "row": 8, "cols": 10, "rows": 10}}));
    assert!(!output.is_error, "{}", output.structured);
    assert_eq!(output.structured["crop_clamped"], true);
    assert_eq!(
        output.structured["crop_pixels"],
        json!({"x": 180, "y": 80, "width": 20, "height": 20})
    );
    screenshot_handler(&mock, 200, 100, (10, 10));
    let explicit = server.call("screenshot", json!({"max_dim": 512}));
    assert_eq!(explicit.structured["max_dim"], 512);
}
