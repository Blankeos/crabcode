use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy)]
pub enum SoundEvent {
    Error,
    Complete,
    Permission,
    Question,
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
