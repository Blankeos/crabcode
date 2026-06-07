use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

const MAX_TERMINAL_TITLE_CHARS: usize = 240;

#[cfg(target_os = "macos")]
const MACOS_NOTIFIER_CACHE_VERSION: &str = "macos-notifier-v3";
#[cfg(target_os = "macos")]
const MACOS_NOTIFIER_APP_NAME: &str = "Crabcode Notifier.app";
#[cfg(target_os = "macos")]
const MACOS_NOTIFIER_BUNDLE_ID: &str = "tl.carlo.crabcode.notifier";
#[cfg(target_os = "macos")]
const MACOS_NOTIFIER_EXECUTABLE: &str = "CrabcodeNotifier";
#[cfg(target_os = "macos")]
const MACOS_NOTIFIER_ICON_FILE: &str = "CrabcodeNotifier";
#[cfg(target_os = "macos")]
const MACOS_NOTIFIER_MARKER: &str = ".crabcode-notifier-ready";
#[cfg(target_os = "macos")]
const MACOS_NOTIFIER_ICON_PNG: &[u8] = include_bytes!("../favicon.png");

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

fn notification_title(workspace_name: Option<&str>) -> String {
    let Some(workspace_name) = workspace_name
        .map(sanitize_notification_title_part)
        .filter(|name| !name.is_empty())
    else {
        return "crabcode".to_string();
    };

    format!("crabcode | {workspace_name}")
}

fn sanitize_notification_title_part(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(target_os = "macos")]
pub fn notify_test_event() -> io::Result<()> {
    let (title, subtitle, body) = notification_content(
        crate::sound::SoundEvent::Complete,
        Some("local app icon test"),
        None,
    );
    let macos_title = with_crab_title(&title);
    try_notify_macos_app(&macos_title, &subtitle, &body)
}

#[cfg(not(target_os = "macos"))]
pub fn notify_test_event() -> io::Result<()> {
    notify_event(
        crate::sound::SoundEvent::Complete,
        Some("local app icon test"),
    );
    Ok(())
}

pub fn notify_event(event: crate::sound::SoundEvent, detail: Option<&str>) {
    notify_event_with_options(event, detail, NotificationOptions::default());
}

#[derive(Debug, Clone)]
pub struct NotificationOptions {
    pub workspace_name: Option<String>,

    #[cfg(target_os = "macos")]
    pub macos_backend: crate::config::MacosNotificationBackend,
}

impl Default for NotificationOptions {
    fn default() -> Self {
        Self {
            workspace_name: None,

            #[cfg(target_os = "macos")]
            macos_backend: crate::config::MacosNotificationBackend::CrabcodeNotifier,
        }
    }
}

pub fn notify_event_with_options(
    event: crate::sound::SoundEvent,
    detail: Option<&str>,
    options: NotificationOptions,
) {
    let (title, subtitle, body) =
        notification_content(event, detail, options.workspace_name.as_deref());

    #[cfg(target_os = "macos")]
    {
        let macos_title = with_crab_title(&title);
        if options.macos_backend == crate::config::MacosNotificationBackend::Osascript
            || try_notify_macos_app(&macos_title, &subtitle, &body).is_err()
        {
            let script = build_osascript(&macos_title, &subtitle, &body);
            let _ = Command::new("osascript").arg("-e").arg(script).spawn();
        }
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
        let script = build_windows_toast_script(&title, &subtitle, &body);
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
    workspace_name: Option<&str>,
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
                notification_title(workspace_name),
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
fn try_notify_macos_app(title: &str, subtitle: &str, body: &str) -> io::Result<()> {
    let app = ensure_macos_notifier_app()?;
    let executable = app
        .join("Contents")
        .join("MacOS")
        .join(MACOS_NOTIFIER_EXECUTABLE);
    let status = Command::new(executable)
        .arg(title)
        .arg(subtitle)
        .arg(body)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("macOS notifier failed with {status}"),
        ))
    }
}

#[cfg(target_os = "macos")]
fn ensure_macos_notifier_app() -> io::Result<std::path::PathBuf> {
    let base = crate::persistence::get_cache_dir().join(MACOS_NOTIFIER_CACHE_VERSION);
    let app = base.join(MACOS_NOTIFIER_APP_NAME);
    let marker = base.join(MACOS_NOTIFIER_MARKER);
    let icon = base.join(format!("{MACOS_NOTIFIER_ICON_FILE}.icns"));
    let executable = app
        .join("Contents")
        .join("MacOS")
        .join(MACOS_NOTIFIER_EXECUTABLE);

    if marker.exists() && app.join("Contents").join("Info.plist").exists() && executable.exists() {
        return Ok(app);
    }

    crate::persistence::ensure_cache_dir()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
    std::fs::create_dir_all(&base)?;
    let _ = std::fs::remove_dir_all(&app);

    let source_icon = base.join("favicon.png");
    let source = base.join("notifier.swift");
    let iconset = base.join(format!("{MACOS_NOTIFIER_ICON_FILE}.iconset"));

    std::fs::write(&source_icon, MACOS_NOTIFIER_ICON_PNG)?;
    std::fs::write(&source, macos_notifier_swift_source())?;

    generate_macos_icns(&source_icon, &iconset, &icon)?;
    compile_macos_notifier_app(&source, &app, &icon)?;
    std::fs::write(marker, MACOS_NOTIFIER_BUNDLE_ID)?;

    Ok(app)
}

#[cfg(target_os = "macos")]
fn generate_macos_icns(
    source_icon: &std::path::Path,
    iconset: &std::path::Path,
    icon: &std::path::Path,
) -> io::Result<()> {
    let _ = std::fs::remove_dir_all(iconset);
    std::fs::create_dir_all(iconset)?;

    for (size, name) in macos_icon_specs() {
        run_macos_command(
            Command::new("sips")
                .arg("-z")
                .arg(size.to_string())
                .arg(size.to_string())
                .arg(source_icon)
                .arg("--out")
                .arg(iconset.join(name)),
        )?;
    }

    run_macos_command(
        Command::new("iconutil")
            .arg("-c")
            .arg("icns")
            .arg(iconset)
            .arg("-o")
            .arg(icon),
    )
}

#[cfg(target_os = "macos")]
fn compile_macos_notifier_app(
    source: &std::path::Path,
    app: &std::path::Path,
    icon: &std::path::Path,
) -> io::Result<()> {
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = app.join("Contents").join("Resources");
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&resources)?;

    run_macos_command(
        Command::new("swiftc")
            .arg("-swift-version")
            .arg("5")
            .arg(source)
            .arg("-o")
            .arg(macos.join(MACOS_NOTIFIER_EXECUTABLE))
            .arg("-framework")
            .arg("UserNotifications"),
    )?;

    std::fs::copy(
        icon,
        resources.join(format!("{MACOS_NOTIFIER_ICON_FILE}.icns")),
    )?;

    std::fs::write(contents.join("Info.plist"), macos_notifier_info_plist())?;

    let _ = run_macos_command(
        Command::new("codesign")
            .arg("-f")
            .arg("-s")
            .arg("-")
            .arg(app),
    );
    let _ = run_macos_command(
        Command::new(
            "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
        )
        .arg("-f")
        .arg(app),
    );

    Ok(())
}

#[cfg(target_os = "macos")]
fn run_macos_command(command: &mut Command) -> io::Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("{program} failed with {status}"),
        ))
    }
}

#[cfg(target_os = "macos")]
fn macos_icon_specs() -> [(u32, &'static str); 10] {
    [
        (16, "icon_16x16.png"),
        (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"),
        (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"),
        (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"),
        (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"),
        (1024, "icon_512x512@2x.png"),
    ]
}

#[cfg(target_os = "macos")]
fn macos_notifier_swift_source() -> &'static str {
    r#"import Foundation
import UserNotifications

final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        if #available(macOS 11.0, *) {
            completionHandler([.banner, .list, .sound])
        } else {
            completionHandler([.alert, .sound])
        }
    }
}

let args = CommandLine.arguments
let title = args.count > 1 ? args[1] : "🦀 crabcode"
let subtitle = args.count > 2 ? args[2] : ""
let body = args.count > 3 ? args[3] : "Your assistant response is ready."

let center = UNUserNotificationCenter.current()
let delegate = NotificationDelegate()
center.delegate = delegate
let group = DispatchGroup()
var granted = false
var addFailed = false

group.enter()
center.requestAuthorization(options: [.alert, .sound]) { didGrant, _ in
    granted = didGrant
    group.leave()
}
group.wait()

if !granted {
    exit(1)
}

let content = UNMutableNotificationContent()
content.title = title
content.subtitle = subtitle
content.body = body
content.sound = .default

let trigger = UNTimeIntervalNotificationTrigger(timeInterval: 0.2, repeats: false)
let request = UNNotificationRequest(
    identifier: "crabcode-\(UUID().uuidString)",
    content: content,
    trigger: trigger
)

group.enter()
center.add(request) { error in
    addFailed = error != nil
    group.leave()
}
group.wait()

if addFailed {
    exit(1)
}

RunLoop.current.run(until: Date().addingTimeInterval(2.0))
"#
}

#[cfg(target_os = "macos")]
fn macos_notifier_info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>{MACOS_NOTIFIER_EXECUTABLE}</string>
  <key>CFBundleIdentifier</key><string>{MACOS_NOTIFIER_BUNDLE_ID}</string>
  <key>CFBundleName</key><string>Crabcode Notifier</string>
  <key>CFBundleDisplayName</key><string>Crabcode</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleIconFile</key><string>{MACOS_NOTIFIER_ICON_FILE}</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
</dict>
</plist>
"#
    )
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
    use super::notification_content;
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

    #[test]
    fn complete_notification_title_includes_workspace_name() {
        let (title, subtitle, body) = notification_content(
            crate::sound::SoundEvent::Complete,
            Some("1.2s | 42t/s"),
            Some("  crabcode\nworkspace  "),
        );

        assert_eq!(title, "crabcode | crabcode workspace");
        assert_eq!(subtitle, "Response complete - 1.2s | 42t/s");
        assert_eq!(body, "Your assistant response is ready.");
    }
}
