use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEvent {
    Error,
    Complete,
    SubagentComplete,
    Permission,
    Question,
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedSoundsConfig {
    pub error: Option<PathBuf>,
    pub complete: Option<PathBuf>,
    pub subagent_complete: Option<PathBuf>,
    pub permission: Option<PathBuf>,
    pub question: Option<PathBuf>,
}

impl ResolvedSoundsConfig {
    pub fn path_for_event(&self, event: SoundEvent) -> Option<&Path> {
        match event {
            SoundEvent::Error => self.error.as_deref(),
            SoundEvent::Complete => self.complete.as_deref(),
            SoundEvent::SubagentComplete => self.subagent_complete.as_deref(),
            SoundEvent::Permission => self.permission.as_deref(),
            SoundEvent::Question => self.question.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum BuiltInSound {
    Error,
    Complete,
    SubagentComplete,
}

#[derive(Debug, Default)]
struct BuiltInSoundCache {
    error: Option<PathBuf>,
    complete: Option<PathBuf>,
    subagent_complete: Option<PathBuf>,
}

const BUILTIN_ERROR_MP3: &[u8] = include_bytes!("../sounds/error.mp3");
const BUILTIN_COMPLETE_MP3: &[u8] = include_bytes!("../sounds/complete.mp3");
const BUILTIN_SUBAGENT_COMPLETE_MP3: &[u8] = include_bytes!("../sounds/subagent_complete.mp3");

pub fn resolve_effective_sounds(
    config: &crate::config::NotificationsConfig,
) -> (ResolvedSoundsConfig, Vec<String>) {
    let mut warnings = Vec::new();
    let mut built_in_cache = BuiltInSoundCache::default();

    let resolved = ResolvedSoundsConfig {
        error: resolve_event_path(
            "notifications.error",
            &config.error,
            Some(BuiltInSound::Error),
            &mut built_in_cache,
            &mut warnings,
        ),
        complete: resolve_event_path(
            "notifications.complete",
            &config.complete,
            Some(BuiltInSound::Complete),
            &mut built_in_cache,
            &mut warnings,
        ),
        subagent_complete: resolve_event_path(
            "notifications.subagentComplete",
            &config.subagent_complete,
            Some(BuiltInSound::SubagentComplete),
            &mut built_in_cache,
            &mut warnings,
        ),
        permission: resolve_event_path(
            "notifications.permission",
            &config.permission,
            None,
            &mut built_in_cache,
            &mut warnings,
        ),
        question: resolve_event_path(
            "notifications.question",
            &config.question,
            None,
            &mut built_in_cache,
            &mut warnings,
        ),
    };

    if config.any_desktop_enabled() && !crate::notify::is_supported() {
        warnings.push(
            "Desktop notifications are enabled, but no supported notification backend is available on this OS"
                .to_string(),
        );
    }

    (resolved, warnings)
}

fn resolve_event_path(
    key: &str,
    effect: &crate::config::NotificationEventConfig,
    fallback: Option<BuiltInSound>,
    built_in_cache: &mut BuiltInSoundCache,
    warnings: &mut Vec<String>,
) -> Option<PathBuf> {
    if !effect.sound_enabled {
        return None;
    }

    if let Some(path) = effect.sound_file.as_ref() {
        if path.is_file() {
            return Some(path.clone());
        }

        warnings.push(format!(
            "{}: configured sound file was not found at {}; event stays silent",
            key,
            path.display()
        ));
        return None;
    }

    if let Some(sound) = fallback {
        return materialize_built_in_sound(sound, built_in_cache, warnings);
    }

    warnings.push(format!(
        "{}: enabled but no file configured; event stays silent",
        key
    ));
    None
}

fn materialize_built_in_sound(
    sound: BuiltInSound,
    built_in_cache: &mut BuiltInSoundCache,
    warnings: &mut Vec<String>,
) -> Option<PathBuf> {
    let cached = match sound {
        BuiltInSound::Error => built_in_cache.error.as_ref(),
        BuiltInSound::Complete => built_in_cache.complete.as_ref(),
        BuiltInSound::SubagentComplete => built_in_cache.subagent_complete.as_ref(),
    };
    if let Some(path) = cached {
        return Some(path.clone());
    }

    let (file_name, bytes) = match sound {
        BuiltInSound::Error => ("error.mp3", BUILTIN_ERROR_MP3),
        BuiltInSound::Complete => ("complete.mp3", BUILTIN_COMPLETE_MP3),
        BuiltInSound::SubagentComplete => ("subagent_complete.mp3", BUILTIN_SUBAGENT_COMPLETE_MP3),
    };

    let sounds_dir = crate::persistence::get_data_dir().join("sounds");
    if let Err(err) = fs::create_dir_all(&sounds_dir) {
        warnings.push(format!(
            "Failed to prepare built-in sounds directory {}: {}",
            sounds_dir.display(),
            err
        ));
        return None;
    }

    let out_path = sounds_dir.join(file_name);
    if let Err(err) = ensure_file_contents(&out_path, bytes) {
        warnings.push(format!(
            "Failed to materialize built-in sound {}: {}",
            out_path.display(),
            err
        ));
        return None;
    }

    match sound {
        BuiltInSound::Error => {
            built_in_cache.error = Some(out_path.clone());
        }
        BuiltInSound::Complete => {
            built_in_cache.complete = Some(out_path.clone());
        }
        BuiltInSound::SubagentComplete => {
            built_in_cache.subagent_complete = Some(out_path.clone());
        }
    }

    Some(out_path)
}

fn ensure_file_contents(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let should_write = match fs::read(path) {
        Ok(existing) => existing != bytes,
        Err(_) => true,
    };

    if should_write {
        fs::write(path, bytes)?;
    }

    Ok(())
}

pub fn play_file(path: &Path) {
    if !path.is_file() {
        return;
    }

    if !sound_playback_available() {
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let _ = spawn_player("afplay", path);
        return;
    }

    #[cfg(target_os = "linux")]
    {
        if spawn_player("paplay", path).is_ok() {
            return;
        }
        let _ = spawn_player("aplay", path);
        return;
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
    }
}

/// Whether this process should attempt local audio playback.
///
/// - `CRABCODE_SOUND=0|false|no|off` forces off
/// - `CRABCODE_SOUND=1|true|yes|on` forces on (still requires a player binary)
/// - otherwise probes once for a usable player / audio backend
fn sound_playback_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(detect_sound_playback_available)
}

fn detect_sound_playback_available() -> bool {
    match env_sound_override() {
        Some(false) => return false,
        Some(true) | None => {}
    }

    #[cfg(target_os = "macos")]
    {
        return player_on_path("afplay");
    }

    #[cfg(target_os = "linux")]
    {
        // Prefer Pulse/PipeWire (`paplay`); fall back to ALSA (`aplay`) only when a
        // sound device is present so headless servers do not fork `aplay` every event.
        if player_on_path("paplay") && pulse_or_pipewire_available() {
            return true;
        }
        if player_on_path("aplay") && alsa_device_available() {
            return true;
        }
        // Forced on via env: allow spawn attempts even without a detected device.
        if env_sound_override() == Some(true) {
            return player_on_path("paplay") || player_on_path("aplay");
        }
        return false;
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

fn env_sound_override() -> Option<bool> {
    let value = std::env::var("CRABCODE_SOUND").ok()?;
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "0" | "false" | "no" | "off" => Some(false),
        "1" | "true" | "yes" | "on" => Some(true),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn spawn_player(player: &str, path: &Path) -> std::io::Result<std::process::Child> {
    Command::new(player)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn player_on_path(player: &str) -> bool {
    // Scan PATH instead of spawning the player (afplay treats unknown flags as files).
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(player);
        candidate.is_file()
    })
}

#[cfg(target_os = "linux")]
fn pulse_or_pipewire_available() -> bool {
    if std::env::var_os("PULSE_SERVER").is_some() {
        return true;
    }
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return false;
    };
    let runtime_dir = PathBuf::from(runtime_dir);
    runtime_dir.join("pulse/native").exists()
        || runtime_dir.join("pipewire-0").exists()
        || runtime_dir.join("pipewire-0-manager").exists()
}

#[cfg(target_os = "linux")]
fn alsa_device_available() -> bool {
    // Common device nodes / proc entries when a sound card is present.
    if Path::new("/dev/snd/controlC0").exists() || Path::new("/dev/dsp").exists() {
        return true;
    }
    match fs::read_to_string("/proc/asound/cards") {
        Ok(contents) => contents.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.contains("no soundcards")
        }),
        Err(_) => false,
    }
}
