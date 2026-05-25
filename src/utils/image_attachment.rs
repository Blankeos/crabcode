use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

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

pub fn open_path(path: &Path, config: &crate::config::ImagesConfig) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("image no longer exists: {}", path.display()));
    }

    match &config.open_with {
        crate::config::ImageOpenWith::Auto => open_auto(path),
        crate::config::ImageOpenWith::System => open_system(path),
        crate::config::ImageOpenWith::Editor => open_editor(path).or_else(|_| open_system(path)),
        crate::config::ImageOpenWith::Command(command) => open_custom_command(path, command),
    }
}

fn open_auto(path: &Path) -> Result<()> {
    if let Some(command) = detected_editor_command() {
        if spawn_command(&command, &[path.to_string_lossy().into_owned()]).is_ok() {
            return Ok(());
        }
    }

    open_system(path)
}

fn open_editor(path: &Path) -> Result<()> {
    if let Some(command) = detected_editor_command() {
        return spawn_command(&command, &[path.to_string_lossy().into_owned()]);
    }

    for var in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(var) {
            if !value.trim().is_empty() {
                return spawn_shell_command(&value, path);
            }
        }
    }

    Err(anyhow!("no editor command detected"))
}

fn detected_editor_command() -> Option<String> {
    if is_zed_terminal() {
        return Some("zed".to_string());
    }

    if has_cursor_env() {
        return Some("cursor".to_string());
    }

    if let Some(app) = std::env::var_os("TERM_PROGRAM")
        .and_then(|value| value.into_string().ok())
        .map(|value| value.to_ascii_lowercase())
    {
        if app.contains("cursor") {
            return Some("cursor".to_string());
        }
    }

    if let Some(command) = detected_editor_from_process_tree() {
        return Some(command);
    }

    if let Some(app) = std::env::var_os("TERM_PROGRAM")
        .and_then(|value| value.into_string().ok())
        .map(|value| value.to_ascii_lowercase())
    {
        if app.contains("vscode") || app == "code" {
            return Some("code".to_string());
        }
    }

    if std::env::var_os("VSCODE_IPC_HOOK_CLI").is_some()
        || std::env::var_os("VSCODE_INJECTION").is_some()
        || std::env::var_os("VSCODE_CWD").is_some()
    {
        return Some("code".to_string());
    }

    None
}

fn has_cursor_env() -> bool {
    std::env::var_os("CURSOR_TRACE_ID").is_some()
        || std::env::var_os("CURSOR_AGENT").is_some()
        || std::env::var_os("CURSOR_CLI").is_some()
}

fn editor_command_from_process_name(name: &str) -> Option<&'static str> {
    let normalized = name.to_ascii_lowercase();
    if normalized.contains("cursor") {
        Some("cursor")
    } else if normalized.contains("zed") {
        Some("zed")
    } else if normalized.contains("visual studio code")
        || normalized.contains("vscode")
        || normalized.contains("code helper")
        || normalized.ends_with("/code")
        || normalized == "code"
    {
        Some("code")
    } else {
        None
    }
}

#[cfg(unix)]
fn detected_editor_from_process_tree() -> Option<String> {
    let mut pid = std::process::id();
    for _ in 0..32 {
        let parent = parent_pid(pid)?;
        if parent == 0 || parent == pid {
            return None;
        }

        if let Some(command) = process_command(parent).and_then(|name| {
            editor_command_from_process_name(&name).map(std::string::ToString::to_string)
        }) {
            return Some(command);
        }

        pid = parent;
    }
    None
}

#[cfg(not(unix))]
fn detected_editor_from_process_tree() -> Option<String> {
    None
}

#[cfg(unix)]
fn parent_pid(pid: u32) -> Option<u32> {
    let output = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .ok()
}

#[cfg(unix)]
fn process_command(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!command.is_empty()).then_some(command)
}

fn is_zed_terminal() -> bool {
    env_eq("ZED_TERM", "true")
        || std::env::var("TERM_PROGRAM")
            .map(|value| value.eq_ignore_ascii_case("zed"))
            .unwrap_or(false)
}

fn env_eq(key: &str, expected: &str) -> bool {
    std::env::var(key)
        .map(|value| value.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn open_custom_command(path: &Path, command: &crate::config::ImageOpenCommandConfig) -> Result<()> {
    let path_arg = path.to_string_lossy();
    let mut args = command
        .args
        .iter()
        .map(|arg| arg.replace("{path}", &path_arg))
        .collect::<Vec<_>>();
    if args.is_empty() {
        args.push(path_arg.into_owned());
    }

    spawn_command(&command.command, &args)
}

fn spawn_command(command: &str, args: &[String]) -> Result<()> {
    Command::new(command)
        .args(args)
        .spawn()
        .with_context(|| format!("failed to run image opener command `{}`", command))?;
    Ok(())
}

fn spawn_shell_command(command: &str, path: &Path) -> Result<()> {
    let path_text = path.to_string_lossy();
    let quoted_path = shlex::try_quote(&path_text)
        .map_err(|err| anyhow!("failed to quote image path {}: {}", path.display(), err))?;
    let shell_command = format!("{} {}", command, quoted_path);
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", &shell_command])
            .spawn()
            .with_context(|| format!("failed to run image opener command `{}`", command))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new("sh")
            .args(["-c", &shell_command])
            .spawn()
            .with_context(|| format!("failed to run image opener command `{}`", command))?;
        Ok(())
    }
}

fn open_system(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open {}", path.display()))?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open {}", path.display()))?;
        return Ok(());
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        Command::new("xdg-open")
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
