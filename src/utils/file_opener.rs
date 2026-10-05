use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    Spawned,
    Suspend(String),
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
