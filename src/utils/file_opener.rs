use anyhow::{anyhow, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    Spawned,
    Suspend(String),
}

/// Make a path absolute without filesystem access or canonicalization.
/// Preserve symlinks and parent components: `link/../file` must resolve relative
/// to the symlink target, not the directory containing the link. Nonexistent
/// paths are accepted unchanged apart from prepending the current directory.
pub fn absolute_file_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to determine current directory")?
            .join(path)
    };
    if !absolute.is_absolute() {
        return Err(anyhow!("cannot make path absolute: {}", path.display()));
    }
    Ok(absolute)
}

const TEXT_SAMPLE_BYTES: usize = 64 * 1024;

/// Conservative prefix heuristic, not a MIME detector. Read at most 64 KiB + 5
/// bytes (UTF-8 boundary lookahead and EOF sentinel); never read directories or special files.
/// Binary data beyond this prefix and unknown UTF-8 containers can go undetected.
pub(crate) fn is_plain_text_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    let mut sample = Vec::with_capacity(TEXT_SAMPLE_BYTES + 5);
    if file
        .take((TEXT_SAMPLE_BYTES + 5) as u64)
        .read_to_end(&mut sample)
        .is_err()
    {
        return false;
    }
    let truncated = sample.len() > TEXT_SAMPLE_BYTES + 4;
    sample.truncate(TEXT_SAMPLE_BYTES + 4);
    text_sample(&sample, truncated)
}

fn text_sample(sample: &[u8], truncated: bool) -> bool {
    if sample.contains(&0) || has_binary_signature(sample) {
        return false;
    }
    let text = match std::str::from_utf8(sample) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            // Only a partial final code point at the bounded-read edge is allowed.
            std::str::from_utf8(&sample[..error.valid_up_to()]).unwrap()
        }
        Err(_) => return false,
    };
    !text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\u{c}'))
}

fn has_binary_signature(sample: &[u8]) -> bool {
    let header = sample.strip_prefix(b"\xef\xbb\xbf").unwrap_or(sample);
    let header = header.trim_ascii_start();
    // Accept offset PDF headers at line starts, not prose mentioning "%PDF-".
    // This is deliberately narrower than what permissive PDF readers accept.
    // Short printable container signatures (BM, OTTO, RIFF, etc.) also occur in
    // ordinary text; their binary headers are left to the NUL/UTF-8/control checks.
    header.starts_with(b"%PDF-")
        || sample[..sample.len().min(1024)]
            .split(|byte| matches!(byte, b'\n' | b'\r'))
            .any(|line| line.trim_ascii_start().starts_with(b"%PDF-"))
        || [
            b"%!".as_slice(),
            b"{\\rtf",
            b"PK\x03\x04",
            b"PK\x05\x06",
            b"PK\x07\x08",
            b"\x1f\x8b",
            b"\xfd7zXZ\0",
            b"7z\xbc\xaf\x27\x1c",
            b"\x89PNG",
            b"\xff\xd8\xff",
            b"GIF87a",
            b"GIF89a",
            b"II*\0",
            b"MM\0*",
            b"\xd0\xcf\x11\xe0",
            b"\x7fELF",
            b"\0asm",
        ]
        .iter()
        .any(|signature| header.starts_with(signature))
        || sample.get(257..262) == Some(b"ustar".as_slice())
}

#[allow(dead_code)] // All command builders are exercised by platform-neutral tests.
#[derive(Debug, Clone, Copy)]
enum RevealPlatform {
    Macos,
    Windows,
    Linux,
}

fn reveal_commands(platform: RevealPlatform, path: &Path) -> Result<Vec<Command>> {
    let mut commands = Vec::new();
    match platform {
        RevealPlatform::Macos => {
            let mut command = Command::new("open");
            command.arg("-R").arg(path);
            commands.push(command);
        }
        RevealPlatform::Windows => {
            let mut command = Command::new("explorer.exe");
            // Keep the switch separate so Windows quotes only the pathname.
            command.arg("/select,").arg(path);
            commands.push(command);
        }
        RevealPlatform::Linux => {
            let uri = url::Url::from_file_path(path)
                .map_err(|_| anyhow!("cannot create file URI for {}", path.display()))?;
            let mut command = Command::new("dbus-send");
            command.args([
                "--session",
                "--print-reply",
                "--reply-timeout=5000",
                "--type=method_call",
                "--dest=org.freedesktop.FileManager1",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ]);
            // dbus-send uses commas as array separators, even within a file URI.
            command.arg(format!("array:string:{}", uri.as_str().replace(',', "%2C")));
            command.arg("string:");
            commands.push(command);
            let mut fallback = Command::new("xdg-open");
            fallback.arg(path.parent().unwrap_or(path));
            commands.push(fallback);
        }
    }
    Ok(commands)
}

/// Select a path in the platform file manager; Linux falls back to its parent.
/// Arguments are passed directly, never through a shell. Nonzero exits are errors.
pub fn reveal_file_path(path: &Path) -> Result<()> {
    let path = absolute_file_path(path)?;
    #[cfg(target_os = "macos")]
    let platform = RevealPlatform::Macos;
    #[cfg(target_os = "windows")]
    let platform = RevealPlatform::Windows;
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let platform = RevealPlatform::Linux;
    let mut failures = Vec::new();
    for mut command in reveal_commands(platform, &path)? {
        let name = command.get_program().to_string_lossy().into_owned();
        match command.stdin(Stdio::null()).output() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => failures.push(format!(
                "{name}: {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    Err(anyhow!(
        "failed to reveal {}: {}",
        path.display(),
        failures.join("; ")
    ))
}

pub fn expand_editor_open_command(
    template: &str,
    path: &Path,
    line: usize,
    column: usize,
) -> Result<String> {
    let line = line.max(1);
    let column = column.max(1);
    let raw_path = path.to_string_lossy();
    let quoted_path = shlex::try_quote(&raw_path)
        .map_err(|err| anyhow!("failed to quote file path {}: {}", path.display(), err))?;
    let location = format!("{}:{}:{}", raw_path, line, column);
    let quoted_location = shlex::try_quote(&location)
        .map_err(|err| anyhow!("failed to quote file location {}: {}", path.display(), err))?;

    let mut command = template.to_string();
    let line_text = line.to_string();
    let column_text = column.to_string();
    let replacements = [
        ("{pathname_raw}", raw_path.as_ref()),
        ("{pathname}", quoted_path.as_ref()),
        ("{filename}", quoted_path.as_ref()),
        ("{location}", quoted_location.as_ref()),
        ("{column}", column_text.as_str()),
        ("{path_raw}", raw_path.as_ref()),
        ("{path}", quoted_path.as_ref()),
        ("{line}", line_text.as_str()),
        ("{col}", column_text.as_str()),
    ];
    for (needle, value) in replacements {
        command = command.replace(needle, value);
    }

    if !template_has_path_placeholder(template) {
        command = format!("{} {}", command.trim_end(), quoted_path);
    }

    Ok(command)
}

fn template_has_path_placeholder(template: &str) -> bool {
    [
        "{pathname_raw}",
        "{pathname}",
        "{filename}",
        "{location}",
        "{path_raw}",
        "{path}",
    ]
    .iter()
    .any(|token| template.contains(token))
}

fn open_with_editor_template(
    template: &str,
    path: &Path,
    line: usize,
    column: usize,
    suspend: bool,
) -> Result<OpenOutcome> {
    let command = expand_editor_open_command(template, path, line, column)?;
    if suspend {
        return Ok(OpenOutcome::Suspend(command));
    }
    spawn_shell_script(&command)?;
    Ok(OpenOutcome::Spawned)
}

pub(crate) fn spawn_shell_script(command: &str) -> Result<()> {
    spawn_shell_script_with_error_handler(command, |error| {
        crate::push_toast(crate::toast::Toast::new(
            error,
            crate::toast::ToastLevel::Error,
            None,
        ));
    })
}

fn spawn_shell_script_with_error_handler(
    command: &str,
    on_error: impl FnOnce(String) + Send + 'static,
) -> Result<()> {
    #[cfg(target_os = "windows")]
    let mut shell = Command::new("cmd");
    #[cfg(target_os = "windows")]
    shell.args(["/C", command]);
    #[cfg(not(target_os = "windows"))]
    let mut shell = Command::new("sh");
    #[cfg(not(target_os = "windows"))]
    shell.args(["-c", command]);

    // Non-suspended commands must not consume input or write over the TUI.
    let mut child = shell
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to run editor command `{command}`"))?;
    let command = command.to_string();
    std::thread::spawn(move || {
        let error = match child.wait() {
            Ok(status) if status.success() => return,
            Ok(status) => format!("Editor command `{command}` exited with {status}"),
            Err(err) => format!("Failed to wait for editor command `{command}`: {err}"),
        };
        on_error(error);
    });
    Ok(())
}

pub fn open_file_path(path: &Path, editor: &crate::config::EditorConfig) -> Result<OpenOutcome> {
    open_file(path, None, editor)
}

pub fn open_file_path_at_location(
    path: &Path,
    line: usize,
    column: usize,
    editor: &crate::config::EditorConfig,
) -> Result<OpenOutcome> {
    open_file(path, Some((line.max(1), column.max(1))), editor)
}

fn open_file(
    path: &Path,
    location: Option<(usize, usize)>,
    editor: &crate::config::EditorConfig,
) -> Result<OpenOutcome> {
    if !path.exists() {
        return Err(anyhow!("file no longer exists: {}", path.display()));
    }

    let (opener, suspend) = editor.opener_for_path(path);
    if let Some(template) = opener {
        if template.trim() == "system" {
            open_system(path)?;
            return Ok(OpenOutcome::Spawned);
        }
        let (line, column) = location.unwrap_or((1, 1));
        return open_with_editor_template(template, path, line, column, suspend);
    }

    open_detected_editor_or_system(path, location, detected_editor_command().as_deref())
}

fn open_detected_editor_or_system(
    path: &Path,
    location: Option<(usize, usize)>,
    command: Option<&str>,
) -> Result<OpenOutcome> {
    if let Some(command) = command {
        let args = match location {
            Some((line, column)) => editor_location_args(command, path, line, column),
            None => vec![path.to_string_lossy().into_owned()],
        };
        if spawn_command(command, &args).is_ok() {
            return Ok(OpenOutcome::Spawned);
        }
    }
    open_system(path)?;
    Ok(OpenOutcome::Spawned)
}

pub fn open_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).with_context(|| format!("invalid url: {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(anyhow!("unsupported url scheme: {}", parsed.scheme()));
    }

    open_system_url(parsed.as_str())
}

fn editor_location_args(command: &str, path: &Path, line: usize, column: usize) -> Vec<String> {
    let path_text = path.to_string_lossy();
    let command_name = std::path::Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();

    if command_name.contains("zed") {
        vec![format!("{}:{}:{}", path_text, line.max(1), column.max(1))]
    } else if command_name.contains("code") || command_name.contains("cursor") {
        vec![
            "-g".to_string(),
            format!("{}:{}:{}", path_text, line.max(1), column.max(1)),
        ]
    } else {
        vec![path_text.into_owned()]
    }
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

fn spawn_command(command: &str, args: &[String]) -> Result<()> {
    Command::new(command)
        .args(args)
        .spawn()
        .with_context(|| format!("failed to run opener command `{}`", command))?;
    Ok(())
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

fn open_system_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .spawn()
            .with_context(|| format!("failed to open {url}"))?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .with_context(|| format!("failed to open {url}"))?;
        return Ok(());
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .with_context(|| format!("failed to open {url}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn absolute_paths_preserve_symlink_parent_resolution() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("elsewhere/child")).unwrap();
        std::fs::write(root.path().join("notes"), "wrong file").unwrap();
        std::fs::write(root.path().join("elsewhere/notes"), "intended file").unwrap();
        std::os::unix::fs::symlink("elsewhere/child", root.path().join("link")).unwrap();
        let path = root.path().join("link/../notes");
        let absolute = absolute_file_path(&path).unwrap();
        assert_eq!(absolute, path);
        assert_eq!(std::fs::read_to_string(absolute).unwrap(), "intended file");
    }

    #[test]
    fn printable_container_prefixes_do_not_reject_text() {
        for text in [
            "BM",
            "BMakefile\nall:\n\techo hello\n",
            "OTTO is a name\n",
            "MZ",
            "RIFF",
            "wOFF",
            "wOF2",
            "BZh",
            "Rar!",
            "SQLite format 3",
        ] {
            assert!(text_sample(text.as_bytes(), false), "{text:?}");
        }
        // Real headers for these formats still contain non-text bytes.
        for header in [
            b"BM\x3a\0\0\0".as_slice(),
            b"OTTO\0\x01\0\x10",
            b"RIFF\x24\0\0\0WAVE",
            b"MZ\x90\0",
            b"wOFFOTTO\0\0\x01\0",
            b"wOF2OTTO\0\0\x01\0",
            b"SQLite format 3\0",
            b"BZh9\x31\x41\x59\x26\x53\x59\xff",
            b"Rar!\x1a\x07\0",
        ] {
            assert!(!text_sample(header, false), "{header:?}");
        }
    }

    #[test]
    fn text_detection_is_content_based_and_conservative() {
        let root = tempfile::tempdir().unwrap();
        for (name, bytes, expected) in [
            ("extensionless", b"hello\n".as_slice(), true),
            ("unicode", "こんにちは 🦀\n".as_bytes(), true),
            ("empty", b"".as_slice(), true),
            ("looks-binary.png", b"actually text".as_slice(), true),
            ("bom", "\u{feff}text".as_bytes(), true),
            ("nul.txt", b"text\0data".as_slice(), false),
            ("invalid.txt", b"text\xff".as_slice(), false),
            ("incomplete.txt", b"text\xe2\x82".as_slice(), false),
            ("pdf", b"%PDF-1.7\nASCII only".as_slice(), false),
            ("pdf-offset", b"prefix\n%PDF-1.7\n".as_slice(), false),
            ("pdf-docs.md", b"PDF begins with %PDF-".as_slice(), true),
            ("pdf-bom", b"\xef\xbb\xbf%PDF-1.7\n".as_slice(), false),
            ("ps", b"%!PS-Adobe-3.0\n".as_slice(), false),
            ("rtf", b"{\\rtf1 hello}".as_slice(), false),
            ("gif", b"GIF89aASCII".as_slice(), false),
            ("zip", b"PK\x03\x04hello".as_slice(), false),
        ] {
            let path = root.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(is_plain_text_file(&path), expected, "{name}");
        }
        assert!(!is_plain_text_file(root.path()));
        assert!(!is_plain_text_file(&root.path().join("missing")));
    }

    #[test]
    fn bounded_text_sample_handles_utf8_edges_and_checks_lookahead() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("text");
        // Split each possible position of a four-byte code point at both edges.
        for edge in [TEXT_SAMPLE_BYTES, TEXT_SAMPLE_BYTES + 4] {
            for offset in 1..=3 {
                let mut bytes = vec![b'a'; edge - offset];
                bytes.extend_from_slice("🦀 more text".as_bytes());
                std::fs::write(&path, bytes).unwrap();
                assert!(is_plain_text_file(&path), "edge {edge}, offset {offset}");
            }
        }
        let mut bytes = vec![b'a'; TEXT_SAMPLE_BYTES - 1];
        bytes.extend_from_slice(b"\xf0\xff\x80\x80rest");
        std::fs::write(&path, bytes).unwrap();
        assert!(!is_plain_text_file(&path));
        let mut bytes = vec![b'a'; TEXT_SAMPLE_BYTES + 3];
        bytes.push(0xe2); // EOF exactly at the sample cap is not a truncated sample.
        std::fs::write(&path, bytes).unwrap();
        assert!(!is_plain_text_file(&path));
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_not_text() {
        assert!(!is_plain_text_file(Path::new("/dev/null")));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_text_falls_back_without_errors() {
        use std::os::unix::fs::PermissionsExt;
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "text").unwrap();
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0)).unwrap();
        // Elevated test runners may still read mode-000 files.
        if std::fs::File::open(file.path()).is_err() {
            assert!(!is_plain_text_file(file.path()));
            let mut editor = suspended_editor();
            editor.text = Some(crate::config::configuration::EditorOpener {
                open: "text-editor".into(),
                suspend: false,
            });
            assert_eq!(
                editor.opener_for_path(file.path()),
                (editor.open.as_deref(), true)
            );
        }
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn text_priority_and_suspension_are_independent() {
        use crate::config::configuration::EditorOpener;
        let root = tempfile::tempdir().unwrap();
        let mut editor = suspended_editor();
        editor.text = Some(EditorOpener {
            open: "text-editor +{line} -- {pathname}".into(),
            suspend: true,
        });
        let path = root.path().join("notes");
        std::fs::write(&path, "🦀").unwrap();
        assert_eq!(
            open_file_path_at_location(&path, 9, 2, &editor).unwrap(),
            OpenOutcome::Suspend(
                expand_editor_open_command("text-editor +{line} -- {pathname}", &path, 9, 2)
                    .unwrap()
            )
        );
        std::fs::write(&path, b"%PDF-1.7").unwrap();
        assert_eq!(
            editor.opener_for_path(&path),
            (editor.open.as_deref(), true)
        );
        assert_eq!(
            editor.opener_for_path(root.path()),
            (editor.open.as_deref(), true)
        );
        assert_eq!(
            editor.opener_for_path(&root.path().join("missing")),
            (editor.open.as_deref(), true)
        );
    }

    #[test]
    fn absolute_paths_preserve_parents_without_requiring_existence() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            absolute_file_path(Path::new("nonexistent/../a/./b")).unwrap(),
            cwd.join("nonexistent/../a/./b")
        );
        assert_eq!(absolute_file_path(Path::new("")).unwrap(), cwd);
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            absolute_file_path(&root.path().join("missing/../leaf")).unwrap(),
            root.path().join("missing/../leaf")
        );
        #[cfg(unix)]
        assert_eq!(
            absolute_file_path(Path::new("/../../a")).unwrap(),
            PathBuf::from("/../../a")
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_paths_do_not_resolve_symlinks() {
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("elsewhere", root.path().join("link")).unwrap();
        assert_eq!(
            absolute_file_path(&root.path().join("link/file")).unwrap(),
            root.path().join("link/file")
        );
    }

    #[test]
    fn reveal_command_arguments_are_shell_free_and_uri_encoded() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("odd 'name',; $(touch bad) 🦀.txt");
        let args = |command: &Command| {
            command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let mac = reveal_commands(RevealPlatform::Macos, &path).unwrap();
        assert_eq!(mac[0].get_program(), "open");
        assert_eq!(
            args(&mac[0]),
            vec!["-R".to_string(), path.to_string_lossy().into_owned()]
        );
        let windows = reveal_commands(RevealPlatform::Windows, &path).unwrap();
        assert_eq!(windows[0].get_program(), "explorer.exe");
        assert_eq!(
            args(&windows[0]),
            vec!["/select,".to_string(), path.to_string_lossy().into_owned()]
        );
        let linux = reveal_commands(RevealPlatform::Linux, &path).unwrap();
        assert_eq!(linux[0].get_program(), "dbus-send");
        let dbus_args = args(&linux[0]);
        assert!(dbus_args.contains(&"org.freedesktop.FileManager1.ShowItems".to_string()));
        let uri_arg = dbus_args
            .iter()
            .find(|a| a.starts_with("array:string:file://"))
            .unwrap();
        let uri = uri_arg.strip_prefix("array:string:").unwrap();
        assert!(!uri.contains(','));
        assert!(!uri.contains(' '));
        assert_eq!(url::Url::parse(uri).unwrap().to_file_path().unwrap(), path);
        assert_eq!(linux[1].get_program(), "xdg-open");
        assert_eq!(
            args(&linux[1]),
            vec![root.path().to_string_lossy().into_owned()]
        );
    }

    fn suspended_editor() -> crate::config::EditorConfig {
        crate::config::EditorConfig {
            open: Some("my-editor {path} +{line}:{column}".to_string()),
            suspend: true,
            ..Default::default()
        }
    }

    #[test]
    fn same_editor_handles_text_images_and_binary_files() {
        let root = tempfile::tempdir().unwrap();
        let editor = suspended_editor();
        for name in ["notes.txt", "screenshot.png", "archive.bin"] {
            let path = root.path().join(name);
            std::fs::write(&path, [0, 255, 0]).unwrap();
            assert_eq!(
                open_file_path(&path, &editor).unwrap(),
                OpenOutcome::Suspend(
                    expand_editor_open_command(editor.open.as_deref().unwrap(), &path, 1, 1)
                        .unwrap()
                )
            );
        }
    }

    #[test]
    fn suspended_opener_preserves_and_clamps_locations() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let editor = suspended_editor();
        for (line, column) in [(12, 4), (0, 0)] {
            assert_eq!(
                open_file_path_at_location(file.path(), line, column, &editor).unwrap(),
                OpenOutcome::Suspend(
                    expand_editor_open_command(
                        editor.open.as_deref().unwrap(),
                        file.path(),
                        line,
                        column
                    )
                    .unwrap()
                )
            );
        }
    }

    #[test]
    fn missing_files_are_errors_not_launches() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing.png");
        assert!(open_file_path(&path, &suspended_editor())
            .unwrap_err()
            .to_string()
            .contains("file no longer exists"));
        assert!(open_file_path_at_location(&path, 12, 4, &suspended_editor()).is_err());
    }

    #[test]
    fn rejects_non_web_url_schemes() {
        for url in ["file:///tmp/file", "javascript:alert(1)", "not a url"] {
            assert!(open_url(url).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn successful_shell_command_receives_quoted_path_and_reports_no_error() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image with spaces.png");
        let result_path = root.path().join("received-path.txt");
        let quoted_result = shlex::try_quote(result_path.to_str().unwrap()).unwrap();
        let template = format!("printf '%s' {{path}} > {quoted_result}");
        let command = expand_editor_open_command(&template, &path, 1, 1).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_shell_script_with_error_handler(&command, move |error| {
            tx.send(error).unwrap();
        })
        .unwrap();
        // The sender is dropped after a successful exit, proving the child was reaped.
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_secs(5)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
        assert_eq!(
            std::fs::read_to_string(result_path).unwrap(),
            path.to_str().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_shell_commands_report_errors_asynchronously() {
        for command in [
            "exit 23 # {path}",
            "crabcode_nonexistent_editor_9267 # {path}",
        ] {
            let (tx, rx) = std::sync::mpsc::channel();
            spawn_shell_script_with_error_handler(command, move |error| {
                tx.send(error).unwrap();
            })
            .unwrap();
            let error = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            assert!(error.contains(command));
            assert!(error.contains("exited with"));
        }
    }

    #[test]
    fn editor_location_args_use_zed_path_line_column_syntax() {
        let path = Path::new("/tmp/project/src/main.rs");

        assert_eq!(
            editor_location_args("zed", path, 12, 4),
            vec!["/tmp/project/src/main.rs:12:4"]
        );
    }

    #[test]
    fn editor_location_args_use_goto_for_code_and_cursor() {
        let path = Path::new("/tmp/project/src/main.rs");

        assert_eq!(
            editor_location_args("code", path, 12, 4),
            vec!["-g", "/tmp/project/src/main.rs:12:4"]
        );
        assert_eq!(
            editor_location_args("cursor", path, 12, 4),
            vec!["-g", "/tmp/project/src/main.rs:12:4"]
        );
    }

    #[test]
    fn expands_helix_open_template() {
        let path = Path::new("/tmp/project/src/main.rs");
        assert_eq!(
            expand_editor_open_command("hx -- {pathname}:{line}:{column}", path, 12, 4).unwrap(),
            "hx -- /tmp/project/src/main.rs:12:4"
        );
        assert_eq!(
            expand_editor_open_command("hx -- {location}", path, 12, 4).unwrap(),
            "hx -- /tmp/project/src/main.rs:12:4"
        );
    }

    #[test]
    fn expands_quoted_path_with_spaces() {
        let path = Path::new("/tmp/my file.rs");
        assert_eq!(
            expand_editor_open_command("hx -- {pathname}:{line}:{col}", path, 3, 1).unwrap(),
            "hx -- '/tmp/my file.rs':3:1"
        );
        assert_eq!(
            expand_editor_open_command("hx -- {location}", path, 3, 1).unwrap(),
            "hx -- '/tmp/my file.rs:3:1'"
        );
    }

    #[test]
    fn appends_path_when_template_has_no_placeholder() {
        let path = Path::new("/tmp/project/src/main.rs");
        assert_eq!(
            expand_editor_open_command("zed", path, 1, 1).unwrap(),
            "zed /tmp/project/src/main.rs"
        );
    }
}
