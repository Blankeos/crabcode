use anyhow::{anyhow, Context, Result};
use std::path::{Component, Path, PathBuf};

pub fn root_dir() -> PathBuf {
    if cfg!(test) || std::env::var_os("CRABCODE_TEST_MODE").is_some() {
        PathBuf::from("/tmp/crabcode_test_data/attachments")
    } else {
        super::get_data_dir().join("attachments")
    }
}

fn validate_session_id(session_id: &str) -> Result<()> {
    let path = Path::new(session_id);
    if session_id.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err(anyhow!("invalid attachment session id"));
    }
    Ok(())
}

pub fn session_dir(session_id: &str) -> Result<PathBuf> {
    validate_session_id(session_id)?;
    Ok(root_dir().join(session_id))
}

pub fn ensure_session_dir(session_id: &str) -> Result<PathBuf> {
    let dir = session_dir(session_id)?;
    super::create_private_dir_all(&dir)?;
    Ok(dir)
}

pub fn write(session_id: &str, extension: &str, data: &[u8]) -> Result<PathBuf> {
    if extension.is_empty() || !extension.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Err(anyhow!("invalid attachment extension"));
    }
    let dir = ensure_session_dir(session_id)?;
    let id = cuid2::create_id();
    let final_path = dir.join(format!("{id}.{extension}"));
    let temporary_path = dir.join(format!(".{id}.tmp"));
    std::fs::write(&temporary_path, data)
        .with_context(|| format!("failed to write attachment {}", temporary_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary_path, std::fs::Permissions::from_mode(0o600))?;
    }
    if let Err(error) = std::fs::rename(&temporary_path, &final_path) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error)
            .with_context(|| format!("failed to finalize attachment {}", final_path.display()));
    }
    Ok(final_path)
}

pub fn is_managed(path: &Path) -> bool {
    path.strip_prefix(root_dir()).is_ok_and(|relative| {
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    })
}

pub fn remove_file(path: &Path) {
    if is_managed(path) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn cleanup_session(session_id: &str) -> Result<()> {
    let dir = session_dir(session_id)?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", dir.display())),
    }
}

pub fn clone_messages(
    messages: &[crate::session::types::Message],
    destination_session_id: &str,
) -> Result<Vec<crate::session::types::Message>> {
    let mut cloned = messages.to_vec();
    let mut created = Vec::new();
    for message in &mut cloned {
        message.id = cuid2::create_id();
        for paths in [
            &mut message.local_image_paths,
            &mut message.local_audio_paths,
        ] {
            for attachment_path in paths {
                let source = PathBuf::from(&*attachment_path);
                if !is_managed(&source) {
                    continue;
                }
                if std::fs::symlink_metadata(&source)?.file_type().is_symlink() {
                    return Err(anyhow!("managed attachment cannot be a symlink"));
                }
                let extension = source
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .ok_or_else(|| anyhow!("managed attachment has no extension"))?;
                let data = std::fs::read(&source)
                    .with_context(|| format!("failed to read attachment {}", source.display()))?;
                match write(destination_session_id, extension, &data) {
                    Ok(path) => {
                        *attachment_path = path.to_string_lossy().into_owned();
                        created.push(path);
                    }
                    Err(error) => {
                        for path in created {
                            remove_file(&path);
                        }
                        return Err(error);
                    }
                }
            }
        }
    }
    Ok(cloned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_attachment_round_trip_and_cleanup() {
        let session = format!("attachment-test-{}", cuid2::create_id());
        let path = write(&session, "png", b"png-data").unwrap();
        assert!(is_managed(&path));
        assert_eq!(std::fs::read(&path).unwrap(), b"png-data");
        cleanup_session(&session).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn fork_clones_managed_files_and_regenerates_message_ids() {
        let source_session = format!("attachment-source-{}", cuid2::create_id());
        let destination_session = format!("attachment-dest-{}", cuid2::create_id());
        let source = write(&source_session, "png", b"image").unwrap();
        let mut message = crate::session::types::Message::user("image");
        let original_id = message.id.clone();
        message.local_image_paths = vec![source.to_string_lossy().into_owned()];

        let cloned = clone_messages(&[message], &destination_session).unwrap();
        assert_ne!(cloned[0].id, original_id);
        assert_ne!(cloned[0].local_image_paths[0], source.to_string_lossy());
        assert_eq!(
            std::fs::read(&cloned[0].local_image_paths[0]).unwrap(),
            b"image"
        );

        cleanup_session(&source_session).unwrap();
        assert!(Path::new(&cloned[0].local_image_paths[0]).exists());
        cleanup_session(&destination_session).unwrap();
    }

    #[test]
    fn traversal_path_is_not_managed() {
        let traversal = root_dir().join("session").join("..").join("outside.png");
        assert!(!is_managed(&traversal));
    }
}
