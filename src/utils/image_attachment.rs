use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{ColorType, DynamicImage, GenericImageView, ImageEncoder, ImageFormat};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
const MAX_PROMPT_IMAGE_DIMENSION: u32 = 2048;

#[derive(Debug, Clone)]
pub struct PromptImage {
    pub data_url: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
}

pub fn is_supported_image_path(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    let supported_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false);

    supported_extension && image::image_dimensions(path).is_ok()
}

pub fn mime_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

pub fn data_url_for_path(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read image {}", path.display()))?;
    let mime_type = mime_type_for_path(path);
    let encoded = general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime_type};base64,{encoded}"))
}

pub fn prompt_image_for_path(path: &Path, preserve_original: bool) -> Result<PromptImage> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read image {}", path.display()))?;
    prompt_image_from_bytes(path, bytes, preserve_original)
}

fn prompt_image_from_bytes(
    path: &Path,
    bytes: Vec<u8>,
    preserve_original: bool,
) -> Result<PromptImage> {
    let source_format = image::guess_format(&bytes).ok().and_then(|format| {
        matches!(
            format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
        )
        .then_some(format)
    });

    let image = image::load_from_memory(&bytes)
        .with_context(|| format!("failed to decode image {}", path.display()))?;
    let (width, height) = image.dimensions();
    let can_keep_original = preserve_original
        || (width <= MAX_PROMPT_IMAGE_DIMENSION && height <= MAX_PROMPT_IMAGE_DIMENSION);

    let (output_bytes, output_format, output_width, output_height) = if can_keep_original {
        if let Some(format) = source_format.filter(|format| can_preserve_source_bytes(*format)) {
            (bytes, format, width, height)
        } else {
            let output_format = ImageFormat::Png;
            let output_bytes = encode_image(&image, output_format)
                .with_context(|| format!("failed to encode image {}", path.display()))?;
            (output_bytes, output_format, width, height)
        }
    } else {
        let resized = image.resize(
            MAX_PROMPT_IMAGE_DIMENSION,
            MAX_PROMPT_IMAGE_DIMENSION,
            FilterType::Triangle,
        );
        let output_format = source_format
            .filter(|format| can_preserve_source_bytes(*format))
            .unwrap_or(ImageFormat::Png);
        let output_bytes = encode_image(&resized, output_format)
            .with_context(|| format!("failed to encode image {}", path.display()))?;
        (
            output_bytes,
            output_format,
            resized.width(),
            resized.height(),
        )
    };

    let media_type = format_to_mime(output_format).to_string();
    let encoded = general_purpose::STANDARD.encode(output_bytes);
    Ok(PromptImage {
        data_url: format!("data:{media_type};base64,{encoded}"),
        media_type,
        width: output_width,
        height: output_height,
    })
}

fn can_preserve_source_bytes(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    )
}

fn encode_image(image: &DynamicImage, format: ImageFormat) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();

    match format {
        ImageFormat::Jpeg => {
            let mut encoder = JpegEncoder::new_with_quality(&mut buffer, 85);
            encoder.encode_image(image)?;
        }
        ImageFormat::WebP => {
            let rgba = image.to_rgba8();
            let encoder = WebPEncoder::new_lossless(&mut buffer);
            encoder.write_image(
                rgba.as_raw(),
                image.width(),
                image.height(),
                ColorType::Rgba8.into(),
            )?;
        }
        _ => {
            let rgba = image.to_rgba8();
            let encoder = PngEncoder::new(&mut buffer);
            encoder.write_image(
                rgba.as_raw(),
                image.width(),
                image.height(),
                ColorType::Rgba8.into(),
            )?;
        }
    }

    Ok(buffer)
}

fn format_to_mime(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => "image/png",
    }
}

pub fn normalize_pasted_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let unwrapped = unwrap_quotes(trimmed);
    if let Some(path) = file_url_to_path(unwrapped) {
        return Some(path);
    }

    if let Some(parts) = shlex::split(trimmed) {
        if parts.len() == 1 {
            let part = unwrap_quotes(parts[0].trim());
            if let Some(path) = file_url_to_path(part) {
                return Some(path);
            }
            return Some(PathBuf::from(part));
        }
    }

    Some(PathBuf::from(unwrapped))
}

pub fn image_paths_from_paste(text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(parts) = shlex::split(text) {
        for part in parts {
            if let Some(path) = normalize_pasted_path(&part) {
                if is_supported_image_path(&path) {
                    paths.push(path);
                }
            }
        }
    }

    if paths.is_empty() {
        for line in text.lines() {
            if let Some(path) = normalize_pasted_path(line) {
                if is_supported_image_path(&path) {
                    paths.push(path);
                }
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

#[cfg(not(target_os = "android"))]
pub fn paste_image_to_temp_png() -> Result<PathBuf> {
    let mut clipboard = arboard::Clipboard::new().context("failed to access clipboard")?;

    if let Ok(files) = clipboard.get().file_list() {
        if let Some(path) = files.into_iter().find(|path| is_supported_image_path(path)) {
            return Ok(path);
        }
    }

    let image = clipboard
        .get_image()
        .context("clipboard does not contain an image")?;
    let bytes = image.bytes.into_owned();
    let rgba = image::RgbaImage::from_raw(image.width as u32, image.height as u32, bytes)
        .ok_or_else(|| anyhow!("clipboard image had invalid RGBA data"))?;
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut png, image::ImageFormat::Png)
        .context("failed to encode clipboard image as PNG")?;

    let mut temp = tempfile::Builder::new()
        .prefix("crabcode-clipboard-")
        .suffix(".png")
        .tempfile()
        .context("failed to create clipboard image file")?;
    temp.write_all(&png.into_inner())
        .context("failed to write clipboard image file")?;
    let (_file, path) = temp.keep().context("failed to persist clipboard image")?;
    Ok(path)
}

#[cfg(target_os = "android")]
pub fn paste_image_to_temp_png() -> Result<PathBuf> {
    let temp = tempfile::Builder::new()
        .prefix("crabcode-attachment-")
        .suffix(".tmp")
        .tempfile()
        .context("failed to create image picker output file")?;
    let (_file, path) = temp
        .keep()
        .context("failed to persist image picker output file")?;

    let status = Command::new("termux-storage-get")
        .arg(&path)
        .status()
        .context(
            "failed to run termux-storage-get; install Termux:API and the termux-api package",
        )?;
    if !status.success() {
        let _ = std::fs::remove_file(&path);
        anyhow::bail!("Termux image picker was cancelled or failed")
    }

    let Some(extension) = detected_image_extension(&path) else {
        let _ = std::fs::remove_file(&path);
        anyhow::bail!("Termux image picker did not return a supported image")
    };

    let image_path = path.with_extension(extension);
    std::fs::rename(&path, &image_path).context("failed to save selected Termux image")?;

    Ok(image_path)
}

#[cfg(target_os = "android")]
fn detected_image_extension(path: &Path) -> Option<&'static str> {
    let bytes = std::fs::read(path).ok()?;
    image_extension(image::guess_format(&bytes).ok()?)
}

fn image_extension(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("png"),
        ImageFormat::Jpeg => Some("jpg"),
        ImageFormat::Gif => Some("gif"),
        ImageFormat::WebP => Some("webp"),
        _ => None,
    }
}

fn unwrap_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn file_url_to_path(value: &str) -> Option<PathBuf> {
    if !value.starts_with("file://") {
        return None;
    }

    url::Url::parse(value).ok()?.to_file_path().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_extension_matches_supported_image_formats() {
        assert_eq!(image_extension(ImageFormat::Png), Some("png"));
        assert_eq!(image_extension(ImageFormat::Jpeg), Some("jpg"));
        assert_eq!(image_extension(ImageFormat::Gif), Some("gif"));
        assert_eq!(image_extension(ImageFormat::WebP), Some("webp"));
        assert_eq!(image_extension(ImageFormat::Bmp), None);
    }
}
