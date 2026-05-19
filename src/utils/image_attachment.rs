use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

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

pub fn open_path(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("image no longer exists: {}", path.display()));
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open {}", path.display()))?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open {}", path.display()))?;
        return Ok(());
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open {}", path.display()))?;
        Ok(())
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
