use super::internal_error;
use agent_client_protocol::Error;
use base64::Engine as _;
use std::path::Path;

pub(super) fn write_prompt_audio(
    session_id: &str,
    audio: &agent_client_protocol::schema::v1::AudioContent,
) -> Result<String, Error> {
    const MAX_AUDIO_BYTES: usize = 20 * 1024 * 1024;
    let media_type = audio.mime_type.trim().to_ascii_lowercase();
    let extension = match media_type.as_str() {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        _ => {
            return Err(Error::invalid_params()
                .data(format!("unsupported audio MIME type: {}", audio.mime_type)));
        }
    };
    let data = base64::engine::general_purpose::STANDARD
        .decode(audio.data.trim())
        .map_err(|error| Error::invalid_params().data(format!("invalid audio data: {error}")))?;
    if data.len() > MAX_AUDIO_BYTES {
        return Err(Error::invalid_params().data("audio exceeds the 20 MiB size limit"));
    }
    let path = crate::persistence::attachments::write(session_id, extension, &data)
        .map_err(|_| internal_error())?;
    Ok(path.to_string_lossy().into_owned())
}

pub(super) struct ManagedAttachmentGuard {
    paths: Vec<String>,
    committed: bool,
}

pub(super) fn write_prompt_image(
    session_id: &str,
    image: &agent_client_protocol::schema::v1::ImageContent,
) -> Result<String, Error> {
    const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

    let (mime_type, encoded_data) = prompt_image_payload(image)?;
    let extension = match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        mime_type => {
            return Err(
                Error::invalid_params().data(format!("unsupported image MIME type: {mime_type}"))
            );
        }
    };
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded_data)
        .map_err(|error| Error::invalid_params().data(format!("invalid image data: {error}")))?;
    if data.len() > MAX_IMAGE_BYTES {
        return Err(Error::invalid_params().data("image exceeds the 20 MiB size limit"));
    }

    let path = crate::persistence::attachments::write(session_id, extension, &data)
        .map_err(|_| internal_error())?;
    Ok(path.to_string_lossy().into_owned())
}

fn prompt_image_payload(
    image: &agent_client_protocol::schema::v1::ImageContent,
) -> Result<(&str, &str), Error> {
    let data = image.data.trim();
    if data.is_empty() {
        return Err(Error::invalid_params().data("image data is empty"));
    }

    let Some(data_uri) = data.strip_prefix("data:") else {
        return Ok((image.mime_type.as_str(), data));
    };
    let (metadata, payload) = data_uri
        .split_once(',')
        .ok_or_else(|| Error::invalid_params().data("invalid image data URI"))?;
    let mut metadata = metadata.split(';');
    let mime_type = metadata
        .next()
        .filter(|mime_type| !mime_type.is_empty())
        .ok_or_else(|| Error::invalid_params().data("image data URI is missing a MIME type"))?;
    if !metadata.any(|value| value.eq_ignore_ascii_case("base64")) {
        return Err(Error::invalid_params().data("image data URI must be base64 encoded"));
    }
    let payload = payload.trim();
    if payload.is_empty() {
        return Err(Error::invalid_params().data("image data is empty"));
    }

    Ok((mime_type, payload))
}

impl ManagedAttachmentGuard {
    pub(super) fn new(paths: Vec<String>) -> Self {
        Self {
            paths,
            committed: false,
        }
    }

    pub(super) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ManagedAttachmentGuard {
    fn drop(&mut self) {
        if !self.committed {
            for path in &self.paths {
                crate::persistence::attachments::remove_file(Path::new(path));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn writes_supported_acp_image_to_managed_session_storage() {
        let session_id = format!("acp-image-{}", cuid2::create_id());
        let image = agent_client_protocol::schema::v1::ImageContent::new("aGk=", "image/png");
        let path = write_prompt_image(&session_id, &image).expect("image file");

        assert!(Path::new(&path).starts_with(crate::persistence::attachments::root_dir()));
        assert_eq!(std::fs::read(&path).expect("image bytes"), b"hi");
        crate::persistence::attachments::cleanup_session(&session_id).unwrap();
    }

    #[test]
    fn writes_supported_acp_audio_to_managed_session_storage() {
        let session_id = format!("acp-audio-{}", cuid2::create_id());
        let audio = agent_client_protocol::schema::v1::AudioContent::new("YXVkaW8=", "audio/wav");
        let path = write_prompt_audio(&session_id, &audio).expect("audio file");

        assert!(path.ends_with(".wav"));
        assert_eq!(std::fs::read(&path).unwrap(), b"audio");
        crate::persistence::attachments::cleanup_session(&session_id).unwrap();
    }

    #[test]
    fn writes_acp_clipboard_image_data_uri_to_managed_storage() {
        let session_id = format!("acp-image-{}", cuid2::create_id());
        let image = agent_client_protocol::schema::v1::ImageContent::new(
            "data:image/png;base64,aGk=",
            "application/octet-stream",
        );
        let path = write_prompt_image(&session_id, &image).expect("image file");

        assert!(path.ends_with(".png"));
        assert_eq!(std::fs::read(&path).expect("image bytes"), b"hi");
        crate::persistence::attachments::cleanup_session(&session_id).unwrap();
    }

    #[test]
    fn rejects_non_base64_acp_image_data_uri() {
        let image = agent_client_protocol::schema::v1::ImageContent::new(
            "data:image/png,not-base64",
            "image/png",
        );

        assert!(write_prompt_image("test", &image).is_err());
    }

    #[test]
    fn rejects_unsupported_acp_image_mime_type() {
        let image = agent_client_protocol::schema::v1::ImageContent::new("aGk=", "image/tiff");

        assert!(write_prompt_image("test", &image).is_err());
    }
}
