//! Screenshot post-processing: crop, downscale, encode, clean up.
//!
//! Vivido returns screenshots as a private temp-file `path` plus capture metadata.
//! A sandboxed or remote MCP client cannot see that path, so this module reads the
//! PNG, optionally crops it to a cell region, downscales it to bound context use,
//! and returns base64 image content — then deletes the temp file. The path never
//! appears in tool output.

use std::fs;

use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat};
use serde_json::{Value, json};

/// Default longest-edge cap for returned screenshots.
pub const DEFAULT_MAX_DIM: u32 = 1280;

/// Output encoding for a returned screenshot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Lossless PNG: best for text and flat terminal colors.
    Png,
    /// Lossy JPEG: smaller for photographic Vivid media.
    Jpeg,
}

impl OutputFormat {
    fn mime(self) -> &'static str {
        match self {
            OutputFormat::Png => "image/png",
            OutputFormat::Jpeg => "image/jpeg",
        }
    }
}

/// A screenshot ready for MCP image content.
pub struct ProcessedImage {
    /// Base64-encoded image bytes in [`ProcessedImage::mime`] encoding.
    pub base64_image: String,
    /// MIME type for the MCP image block.
    pub mime: &'static str,
    /// Metadata for the text block and `structuredContent` (no path).
    pub metadata: Value,
}

/// Read, crop, downscale, and encode one `screenshot` result.
pub fn process_screenshot(
    result: &Value,
    args: &Value,
    default_max_dim: u32,
) -> Result<ProcessedImage, String> {
    let path = result
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "screenshot reply is missing its path".to_owned())?;
    let bytes = fs::read(path).map_err(|error| format!("cannot read screenshot PNG: {error}"))?;
    let image = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .map_err(|error| format!("cannot decode screenshot PNG: {error}"))?;
    let captured_width = image.width();
    let captured_height = image.height();

    let cell = result.get("cell").cloned().unwrap_or(json!({}));
    let padding = result.get("padding").cloned().unwrap_or(json!({}));
    let cell_w = cell.get("width").and_then(Value::as_u64).unwrap_or(0);
    let cell_h = cell.get("height").and_then(Value::as_u64).unwrap_or(0);
    let pad_x = padding.get("x").and_then(Value::as_u64).unwrap_or(0);
    let pad_y = padding.get("y").and_then(Value::as_u64).unwrap_or(0);

    let format = match args.get("format").and_then(Value::as_str) {
        None | Some("png") => OutputFormat::Png,
        Some("jpeg") | Some("jpg") => OutputFormat::Jpeg,
        Some(other) => return Err(format!("`format` must be \"png\" or \"jpeg\", not {other:?}")),
    };
    let quality = args.get("quality").and_then(Value::as_u64).unwrap_or(85);
    if format == OutputFormat::Jpeg && !(1..=100).contains(&quality) {
        return Err("`quality` must be 1 through 100".to_owned());
    }

    let mut image = image.to_rgba8();
    let mut crop_pixels = Value::Null;
    let mut crop_clamped = false;
    if let Some(crop) = args.get("crop").filter(|value| !value.is_null()) {
        let (x, y, width, height) = crop_box(crop, cell_w, cell_h, pad_x, pad_y)?;
        let (clamped, was_clamped) = clamp_box(x, y, width, height, image.width(), image.height())?;
        let (x, y, width, height) = clamped;
        crop_clamped = was_clamped;
        image = image::imageops::crop_imm(&image, x, y, width, height).to_image();
        crop_pixels = json!({ "x": x, "y": y, "width": width, "height": height });
    }

    let max_dim = args
        .get("max_dim")
        .and_then(Value::as_u64)
        .map(|value| value.clamp(64, 4096) as u32)
        .unwrap_or(default_max_dim);
    let longest = image.width().max(image.height());
    let downscaled_from = if longest > max_dim {
        let scale = f64::from(max_dim) / f64::from(longest);
        let width = ((f64::from(image.width()) * scale).round() as u32).max(1);
        let height = ((f64::from(image.height()) * scale).round() as u32).max(1);
        let from = json!({ "width": image.width(), "height": image.height() });
        image =
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
        from
    } else {
        Value::Null
    };

    let mut encoded = Vec::new();
    match format {
        OutputFormat::Png => PngEncoder::new(&mut encoded)
            .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgba8)
            .map_err(|error| format!("cannot re-encode screenshot PNG: {error}"))?,
        // JPEG has no alpha channel; drop it (screenshots are opaque).
        OutputFormat::Jpeg => {
            let flat = image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
                let pixel = image.get_pixel(x, y);
                image::Rgb([pixel[0], pixel[1], pixel[2]])
            });
            JpegEncoder::new_with_quality(&mut encoded, quality as u8)
                .write_image(flat.as_raw(), flat.width(), flat.height(), ExtendedColorType::Rgb8)
                .map_err(|error| format!("cannot encode screenshot JPEG: {error}"))?;
        }
    }

    // The caller owns temp cleanup; never let a stale frame linger, and never
    // report the path — only whether cleanup succeeded.
    let temp_deleted = fs::remove_file(path).is_ok();

    let base64_image = base64::engine::general_purpose::STANDARD.encode(&encoded);
    let metadata = json!({
        "window_id": result.get("window_id").cloned().unwrap_or(Value::Null),
        "frame_sequence": result.get("frame_sequence").cloned().unwrap_or(Value::Null),
        "width": image.width(),
        "height": image.height(),
        "captured_width": captured_width,
        "captured_height": captured_height,
        "scale_factor": result.get("scale_factor").cloned().unwrap_or(Value::Null),
        "cell": cell,
        "padding": padding,
        "crop": args.get("crop").cloned().unwrap_or(Value::Null),
        "crop_pixels": crop_pixels,
        "crop_clamped": crop_clamped,
        "downscaled_from": downscaled_from,
        "max_dim": max_dim,
        "format": format.mime().split('/').nth(1).unwrap_or("png"),
        "mime": format.mime(),
        "bytes": base64_image.len(),
        "temp_deleted": temp_deleted,
    });
    Ok(ProcessedImage { base64_image, mime: format.mime(), metadata })
}

/// A pixel box: `(x, y, width, height)`.
type CropBox = (u32, u32, u32, u32);

/// Map a `{col,row,cols,rows}` cell region to pixel coordinates using the
/// capture's own metrics. Padding is read, never derived: with dynamic padding
/// off, the sub-cell remainder collects at the right and bottom.
fn crop_box(
    crop: &Value,
    cell_w: u64,
    cell_h: u64,
    pad_x: u64,
    pad_y: u64,
) -> Result<CropBox, String> {
    if cell_w == 0 || cell_h == 0 {
        return Err("capture is missing cell metrics; cannot crop".to_owned());
    }
    let number = |key: &str| {
        crop.get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("`crop.{key}` must be an integer"))
    };
    let (col, row, cols, rows) = (number("col")?, number("row")?, number("cols")?, number("rows")?);
    if cols == 0 || rows == 0 {
        return Err("`crop` cols and rows must be at least 1".to_owned());
    }
    let x = pad_x + col.saturating_mul(cell_w);
    let y = pad_y + row.saturating_mul(cell_h);
    let width = cols.saturating_mul(cell_w);
    let height = rows.saturating_mul(cell_h);
    Ok((
        x.min(u64::from(u32::MAX)) as u32,
        y.min(u64::from(u32::MAX)) as u32,
        width.min(u64::from(u32::MAX)) as u32,
        height.min(u64::from(u32::MAX)) as u32,
    ))
}

/// Clamp a crop box to the image, refusing a region fully outside the capture.
/// Returns the box plus whether clamping shrank it.
fn clamp_box(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    image_w: u32,
    image_h: u32,
) -> Result<(CropBox, bool), String> {
    if x >= image_w || y >= image_h {
        return Err("crop region is outside the capture".to_owned());
    }
    let clamped = (x, y, width.min(image_w - x).max(1), height.min(image_h - y).max(1));
    Ok((clamped, clamped != (x, y, width, height)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_uses_padding_not_derivation() {
        // cell 10x20, padding (5, 3): col 1, row 2, 3x4 cells ->
        // x = 5 + 10 = 15, y = 3 + 40 = 43, w = 30, h = 80.
        assert_eq!(
            crop_box(&json!({"col": 1, "row": 2, "cols": 3, "rows": 4}), 10, 20, 5, 3).unwrap(),
            (15, 43, 30, 80)
        );
    }

    #[test]
    fn crop_outside_capture_is_an_error() {
        assert!(clamp_box(500, 0, 10, 10, 100, 100).is_err());
        assert_eq!(clamp_box(90, 90, 50, 50, 100, 100).unwrap(), ((90, 90, 10, 10), true));
        assert_eq!(clamp_box(10, 10, 20, 20, 100, 100).unwrap(), ((10, 10, 20, 20), false));
    }
}
