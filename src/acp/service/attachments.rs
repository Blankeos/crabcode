use super::internal_error;
use super::AcpSession;
use crate::config::configuration::LoadedConfig;
use agent_client_protocol::schema::v1::ContentBlock;
use agent_client_protocol::schema::v1::EmbeddedResourceResource;
use agent_client_protocol::Error;
use base64::Engine as _;
use std::path::Path;

fn write_prompt_audio(
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

pub(super) fn model_supports_audio(config: &LoadedConfig, provider: &str, model: &str) -> bool {
    crate::model::discovery::Discovery::new_with_custom(Some(
        config.merged_config.custom_providers.clone(),
    ))
    .ok()
    .is_some_and(|discovery| discovery.model_supports_input_modality(provider, model, "audio"))
}

pub(super) struct ManagedAttachmentGuard {
    paths: Vec<String>,
    committed: bool,
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

pub(super) fn prompt_content(
    parts: Vec<ContentBlock>,
    text_override: Option<&str>,
    supports_images: bool,
    supports_audio: bool,
    session_id: &str,
    session: &AcpSession,
) -> Result<(String, Vec<String>, Vec<String>), Error> {
    let mut text = String::new();
    let mut text_override = text_override;
    let mut local_image_paths = Vec::new();
    let mut local_audio_paths = Vec::new();
    for part in parts {
        match part {
            ContentBlock::Text(content) => {
                text.push_str(text_override.take().unwrap_or(&content.text));
            }
            ContentBlock::ResourceLink(link) => {
                text.push_str(&format!("[{}]", link.uri));
            }
            ContentBlock::Resource(resource) => match resource.resource {
                EmbeddedResourceResource::TextResourceContents(resource) => {
                    text.push_str(&format!("[{}]\n{}", resource.uri, resource.text));
                }
                EmbeddedResourceResource::BlobResourceContents(resource) => {
                    text.push_str(&format!("[{}]", resource.uri));
                }
                _ => {}
            },
            ContentBlock::Image(image) => {
                if !supports_images {
                    return Err(Error::invalid_params().data(format!(
                        "model {}/{} does not support image input",
                        session.provider, session.model
                    )));
                }
                match write_prompt_image(session_id, &image) {
                    Ok(path) => local_image_paths.push(path),
                    Err(error) => {
                        for path in &local_image_paths {
                            crate::persistence::attachments::remove_file(Path::new(path));
                        }
                        return Err(error);
                    }
                }
            }
            ContentBlock::Audio(audio) => {
                if !supports_audio {
                    for path in local_image_paths.iter().chain(local_audio_paths.iter()) {
                        crate::persistence::attachments::remove_file(Path::new(path));
                    }
                    return Err(Error::invalid_params().data(format!(
                        "model {}/{} does not support audio input",
                        session.provider, session.model
                    )));
                }
                match write_prompt_audio(session_id, &audio) {
                    Ok(path) => local_audio_paths.push(path),
                    Err(error) => {
                        for path in local_image_paths.iter().chain(local_audio_paths.iter()) {
                            crate::persistence::attachments::remove_file(Path::new(path));
                        }
                        return Err(error);
                    }
                }
            }
            _ => {}
        }
    }
    if text.is_empty() {
        if !local_image_paths.is_empty() {
            text.push_str("[Image attached]");
        } else if !local_audio_paths.is_empty() {
            text.push_str("[Audio attached]");
        }
    }
    Ok((text, local_image_paths, local_audio_paths))
}

fn prompt_text(parts: Vec<ContentBlock>) -> Result<String, Error> {
    let mut text = String::new();
    for part in parts {
        match part {
            ContentBlock::Text(content) => text.push_str(&content.text),
            ContentBlock::ResourceLink(link) => text.push_str(&format!("[{}]", link.uri)),
            ContentBlock::Resource(resource) => match resource.resource {
                EmbeddedResourceResource::TextResourceContents(resource) => {
                    text.push_str(&format!("[{}]\n{}", resource.uri, resource.text));
                }
                EmbeddedResourceResource::BlobResourceContents(resource) => {
                    text.push_str(&format!("[{}]", resource.uri));
                }
                _ => {}
            },
            ContentBlock::Image(_) | ContentBlock::Audio(_) => {
                return Err(
                    Error::invalid_params().data("binary ACP prompt content is not supported here")
                );
            }
            _ => {}
        }
    }
    Ok(text)
}

fn write_prompt_image(
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

#[cfg(test)]
mod tests {
    use super::super::test_support::test_session;
    use super::*;
    use agent_client_protocol::schema::v1::ContentBlock;
    use agent_client_protocol::schema::v1::EmbeddedResourceResource;
    use std::path::Path;

    #[test]
    fn flattens_text_and_embedded_context() {
        let text = prompt_text(vec![
            ContentBlock::from("Inspect this."),
            ContentBlock::Resource(agent_client_protocol::schema::v1::EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(
                    agent_client_protocol::schema::v1::TextResourceContents::new(
                        "fn main() {}",
                        "file:///tmp/main.rs",
                    ),
                ),
            )),
        ])
        .expect("prompt text");

        assert_eq!(text, "Inspect this.[file:///tmp/main.rs]\nfn main() {}");
    }

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
    fn command_text_override_preserves_embedded_context() {
        let resource = agent_client_protocol::schema::v1::EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(
                agent_client_protocol::schema::v1::TextResourceContents::new(
                    "important context",
                    "file:///tmp/context.txt",
                ),
            ),
        );
        let (text, images, audio) = prompt_content(
            vec![
                ContentBlock::Text(agent_client_protocol::schema::v1::TextContent::new(
                    "/review src/lib.rs",
                )),
                ContentBlock::Text(agent_client_protocol::schema::v1::TextContent::new(
                    "\nAdditional instructions",
                )),
                ContentBlock::Resource(resource),
            ],
            Some("expanded command prompt"),
            false,
            false,
            "test-session",
            &test_session(),
        )
        .expect("prompt content");

        assert_eq!(
            text,
            "expanded command prompt\nAdditional instructions[file:///tmp/context.txt]\nimportant context"
        );
        assert!(images.is_empty());
        assert!(audio.is_empty());
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
    fn rejects_audio_for_models_without_audio_modality() {
        let session_id = format!("acp-audio-{}", cuid2::create_id());
        let result = prompt_content(
            vec![ContentBlock::Audio(
                agent_client_protocol::schema::v1::AudioContent::new("YXVkaW8=", "audio/wav"),
            )],
            None,
            false,
            false,
            &session_id,
            &test_session(),
        );

        assert!(result.is_err());
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
    fn prompt_image_failure_rolls_back_prior_managed_files() {
        let session_id = format!("acp-image-{}", cuid2::create_id());
        let result = prompt_content(
            vec![
                ContentBlock::Image(agent_client_protocol::schema::v1::ImageContent::new(
                    "aGk=",
                    "image/png",
                )),
                ContentBlock::Image(agent_client_protocol::schema::v1::ImageContent::new(
                    "not-base64",
                    "image/png",
                )),
            ],
            None,
            true,
            false,
            &session_id,
            &test_session(),
        );

        assert!(result.is_err());
        let directory = crate::persistence::attachments::session_dir(&session_id).unwrap();
        assert!(
            !directory.exists()
                || std::fs::read_dir(&directory)
                    .unwrap()
                    .all(|entry| entry.is_err())
        );
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
