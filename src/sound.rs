use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;

#[derive(Debug, Clone, Copy)]
pub enum SoundEvent {
    Error,
    Complete,
    Permission,
    Question,
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedSoundsConfig {
    pub error: Option<PathBuf>,
    pub complete: Option<PathBuf>,
    pub permission: Option<PathBuf>,
    pub question: Option<PathBuf>,
}

impl ResolvedSoundsConfig {
    pub fn path_for_event(&self, event: SoundEvent) -> Option<&Path> {
        match event {
            SoundEvent::Error => self.error.as_deref(),
            SoundEvent::Complete => self.complete.as_deref(),
            SoundEvent::Permission => self.permission.as_deref(),
            SoundEvent::Question => self.question.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum BuiltInSound {
    Error,
    Complete,
}

#[derive(Debug, Default)]
struct BuiltInSoundCache {
    error: Option<PathBuf>,
    complete: Option<PathBuf>,
}

const BUILTIN_ERROR_MP3: &[u8] = include_bytes!("../sounds/error.mp3");
const BUILTIN_COMPLETE_MP3: &[u8] = include_bytes!("../sounds/complete.mp3");

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
    };
    if let Some(path) = cached {
        return Some(path.clone());
    }

    let (file_name, bytes) = match sound {
        BuiltInSound::Error => ("error.mp3", BUILTIN_ERROR_MP3),
        BuiltInSound::Complete => ("complete.mp3", BUILTIN_COMPLETE_MP3),
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

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("afplay").arg(path).spawn();
        return;
    }

    #[cfg(target_os = "linux")]
    {
        if Command::new("paplay").arg(path).spawn().is_ok() {
            return;
        }
        let _ = Command::new("aplay").arg(path).spawn();
        return;
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
    }
}
