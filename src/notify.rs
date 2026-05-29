use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

const MAX_TERMINAL_TITLE_CHARS: usize = 240;

pub fn is_supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        return command_available("osascript");
    }

    #[cfg(target_os = "linux")]
    {
        return command_available("notify-send");
    }

    #[cfg(target_os = "windows")]
    {
        return command_available("pwsh") || command_available("powershell");
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

pub fn notify_event(event: crate::sound::SoundEvent, detail: Option<&str>) {
    let (title, subtitle, body) = notification_content(event, detail);

    #[cfg(target_os = "macos")]
    {
        let osascript_title = with_crab_title(&title);
        let script = build_osascript(&osascript_title, &subtitle, &body);
        let _ = Command::new("osascript").arg("-e").arg(script).spawn();
        return;
    }

    #[cfg(target_os = "linux")]
    {
        let summary = if subtitle.is_empty() {
            title.to_string()
        } else {
            format!("{} - {}", title, subtitle)
        };

        let _ = Command::new("notify-send")
            .arg("-a")
            .arg("crabcode")
            .arg(summary)
            .arg(body)
            .spawn();
        return;
    }

    #[cfg(target_os = "windows")]
    {
        let script = build_windows_toast_script(title, subtitle, body);
        if command_available("pwsh") {
            let _ = Command::new("pwsh")
                .arg("-NoProfile")
                .arg("-Command")
                .arg(&script)
                .spawn();
            return;
        }

        let _ = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(script)
            .spawn();
        return;
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (title, subtitle, body);
    }
}

pub fn notify_terminal_bell() {
    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"\x07");
    let _ = stdout.flush();
}

pub fn terminal_bell_supported() -> bool {
    env_eq("ZED_TERM", "true") || env_eq("TERM_PROGRAM", "zed")
}

pub fn terminal_title_supported() -> bool {
    terminal_bell_supported()
}

pub fn set_terminal_title(title: &str) -> io::Result<()> {
    if !io::stdout().is_terminal() {
        return Ok(());
    }

    write_terminal_title(&sanitize_terminal_title(title))
}

pub fn clear_terminal_title() -> io::Result<()> {
    if !io::stdout().is_terminal() {
        return Ok(());
    }

    write_terminal_title("")
}

fn write_terminal_title(title: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]0;{}\x07", title)?;
    stdout.flush()
}

fn sanitize_terminal_title(title: &str) -> String {
    let mut sanitized = String::new();
    let mut chars_written = 0;
    let mut pending_space = false;

    for ch in title.chars() {
        if ch.is_whitespace() {
            pending_space = !sanitized.is_empty();
            continue;
        }

        if is_disallowed_terminal_title_char(ch) {
            continue;
        }

        if pending_space {
            let remaining = MAX_TERMINAL_TITLE_CHARS.saturating_sub(chars_written);
            if remaining > 1 {
                sanitized.push(' ');
                chars_written += 1;
            }
            pending_space = false;
        }

        if chars_written >= MAX_TERMINAL_TITLE_CHARS {
            break;
        }

        sanitized.push(ch);
        chars_written += 1;
    }

    sanitized
}

fn is_disallowed_terminal_title_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{0000}'..='\u{001F}'
            | '\u{007F}'..='\u{009F}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

fn env_eq(key: &str, expected: &str) -> bool {
    std::env::var(key)
        .map(|value| value.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn notification_content(
    event: crate::sound::SoundEvent,
    detail: Option<&str>,
) -> (String, String, String) {
    match event {
        crate::sound::SoundEvent::Complete => {
            let subtitle = match detail {
                Some(stats) if !stats.trim().is_empty() => {
                    format!("Response complete - {}", stats.trim())
                }
                _ => "Response complete".to_string(),
            };
            (
                "crabcode".to_string(),
                subtitle,
                "Your assistant response is ready.".to_string(),
            )
        }
        crate::sound::SoundEvent::Error => (
            "crabcode".to_string(),
            "Action failed".to_string(),
            "Something went wrong while processing your request.".to_string(),
        ),
        crate::sound::SoundEvent::Permission => (
            "crabcode".to_string(),
            "Permission required".to_string(),
            "A tool is requesting permission.".to_string(),
        ),
        crate::sound::SoundEvent::Question => (
            "crabcode".to_string(),
            "Question".to_string(),
            "The assistant needs your input.".to_string(),
        ),
    }
}

fn command_available(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

#[cfg(target_os = "macos")]
fn build_osascript(title: &str, subtitle: &str, body: &str) -> String {
    let mut script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(body),
        escape_applescript(title),
    );

    if !subtitle.is_empty() {
        script.push_str(&format!(" subtitle \"{}\"", escape_applescript(subtitle)));
    }

    script
}

#[cfg(target_os = "macos")]
fn with_crab_title(title: &str) -> String {
    if title.trim().is_empty() {
        return "🦀 crabcode".to_string();
    }
    if title.starts_with('🦀') {
        return title.to_string();
    }
    format!("🦀 {}", title)
}

#[cfg(target_os = "macos")]
fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "windows")]
fn build_windows_toast_script(title: &str, subtitle: &str, body: &str) -> String {
    let heading = if subtitle.is_empty() {
        title.to_string()
    } else {
        format!("{} - {}", title, subtitle)
    };

    let heading = escape_xml(&heading);
    let body = escape_xml(body);

    format!(
        r#"$null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
$null = [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]
$template = "<toast><visual><binding template='ToastText02'><text id='1'>{}</text><text id='2'>{}</text></binding></visual></toast>"
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
$notifier = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('crabcode')
$notifier.Show($toast)"#,
        heading, body
    )
}

#[cfg(target_os = "windows")]
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::sanitize_terminal_title;
    use super::MAX_TERMINAL_TITLE_CHARS;

    #[test]
    fn terminal_title_sanitizer_strips_controls_and_collapses_space() {
        let sanitized = sanitize_terminal_title("  crab\tcode\n\x1b\x07\u{202E} running  ");

        assert_eq!(sanitized, "crab code running");
    }

    #[test]
    fn terminal_title_sanitizer_truncates_long_titles() {
        let title = "x".repeat(MAX_TERMINAL_TITLE_CHARS + 10);
        let sanitized = sanitize_terminal_title(&title);

        assert_eq!(sanitized.len(), MAX_TERMINAL_TITLE_CHARS);
    }
}
