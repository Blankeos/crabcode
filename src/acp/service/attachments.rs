use super::attachment_storage::write_prompt_audio;
use super::attachment_storage::write_prompt_image;
use super::AcpSession;
use crate::config::configuration::LoadedConfig;
use agent_client_protocol::schema::v1::ContentBlock;
use agent_client_protocol::schema::v1::EmbeddedResourceResource;
use agent_client_protocol::Error;
use std::path::Path;

pub(super) fn model_supports_audio(config: &LoadedConfig, provider: &str, model: &str) -> bool {
    crate::model::discovery::Discovery::new_with_custom(Some(
        config.merged_config.custom_providers.clone(),
    ))
    .ok()
    .is_some_and(|discovery| discovery.model_supports_input_modality(provider, model, "audio"))
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

#[cfg(test)]
mod tests {
    use super::super::test_support::test_session;
    use super::*;
    use agent_client_protocol::schema::v1::ContentBlock;
    use agent_client_protocol::schema::v1::EmbeddedResourceResource;

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
}
