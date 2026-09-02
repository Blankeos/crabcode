use crate::persistence::{
    Message, MessagePart as PersistenceMessagePart, Session as PersistenceSession,
};
use crate::session::types::{
    CompactionStats, Message as SessionMessage, MessagePart as SessionMessagePart, MessageRole,
    Session,
};

impl From<SessionMessage> for Message {
    fn from(msg: SessionMessage) -> Self {
        // Move the owned parts instead of cloning them: this conversion runs
        // for the whole transcript on every streaming snapshot.
        let mut parts: Vec<PersistenceMessagePart> = if msg.parts.is_empty() {
            let mut parts = Vec::new();
            if !msg.content.is_empty() {
                parts.push(PersistenceMessagePart {
                    part_type: "text".to_string(),
                    data: serde_json::json!({ "text": msg.content }),
                });
            }
            for path in &msg.local_audio_paths {
                parts.push(PersistenceMessagePart {
                    part_type: "local_audio".to_string(),
                    data: serde_json::json!({ "path": path }),
                });
            }
            parts
        } else {
            msg.parts
                .into_iter()
                .map(|part| PersistenceMessagePart {
                    part_type: part.part_type,
                    data: part.data,
                })
                .collect()
        };

        if let Some(ref reasoning) = msg.reasoning {
            if !reasoning.is_empty() && !parts.iter().any(|part| part.part_type == "reasoning") {
                parts.push(PersistenceMessagePart {
                    part_type: "reasoning".to_string(),
                    data: serde_json::json!({ "text": reasoning }),
                });
            }
        }

        for path in &msg.local_image_paths {
            parts.push(PersistenceMessagePart {
                part_type: "local_image".to_string(),
                data: serde_json::json!({ "path": path }),
            });
        }
        for path in &msg.local_audio_paths {
            parts.push(PersistenceMessagePart {
                part_type: "local_audio".to_string(),
                data: serde_json::json!({ "path": path }),
            });
        }

        if let Some(stats) = msg.compaction_stats {
            if let Ok(data) = serde_json::to_value(stats) {
                parts.push(PersistenceMessagePart {
                    part_type: "compaction_stats".to_string(),
                    data,
                });
            }
        }

        if msg.was_interrupted
            && !parts.iter().any(|part| {
                part.part_type == "status"
                    && part
                        .data
                        .get("state")
                        .and_then(|value| value.as_str())
                        .is_some_and(|state| state == "interrupted")
            })
        {
            parts.push(PersistenceMessagePart {
                part_type: "status".to_string(),
                data: serde_json::json!({ "state": "interrupted" }),
            });
        }

        Message {
            id: msg.id,
            session_id: 0,
            role: match msg.role {
                MessageRole::User => "user".to_string(),
                MessageRole::Assistant => "assistant".to_string(),
                MessageRole::System => "system".to_string(),
                MessageRole::Tool => "tool".to_string(),
            },
            parts,
            timestamp: msg
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            tokens_used: msg.token_count.map(|c| c as i32).unwrap_or(0),
            model: msg.model.clone(),
            provider: msg.provider.clone(),
            agent_mode: msg.agent_mode.clone(),
            duration_ms: msg.duration_ms.map(|d| d as i64).unwrap_or(0),
            t0_ms: msg.t0_ms.map(|v| v as i64),
            t1_ms: msg.t1_ms.map(|v| v as i64),
            tn_ms: msg.tn_ms.map(|v| v as i64),
            output_tokens: msg.output_tokens.map(|v| v as i64),
            input_tokens: msg.input_tokens.map(|v| v as i64),
            cache_read_tokens: msg.cache_read_tokens.map(|v| v as i64),
            cache_write_tokens: msg.cache_write_tokens.map(|v| v as i64),
            cost: msg.cost,
            usage_authoritative: msg.usage_authoritative,
        }
    }
}

impl TryFrom<Message> for SessionMessage {
    type Error = anyhow::Error;

    fn try_from(msg: Message) -> Result<Self, Self::Error> {
        let session_parts: Vec<SessionMessagePart> = msg
            .parts
            .iter()
            .map(|part| SessionMessagePart {
                part_type: part.part_type.clone(),
                data: part.data.clone(),
            })
            .collect();
        let local_audio_paths = session_parts
            .iter()
            .filter_map(|part| {
                (part.part_type == "local_audio")
                    .then(|| part.data.get("path").and_then(|value| value.as_str()))
                    .flatten()
            })
            .map(str::to_string)
            .collect();

        let content = session_parts
            .iter()
            .filter_map(|p| {
                if p.part_type == "text" {
                    p.data.get("text").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let reasoning = session_parts
            .iter()
            .filter_map(|p| {
                if p.part_type == "reasoning" {
                    p.data.get("text").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");
        let reasoning = (!reasoning.is_empty()).then_some(reasoning);

        let local_image_paths = session_parts
            .iter()
            .filter_map(|p| {
                if p.part_type == "local_image" {
                    p.data.get("path").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .map(|path| path.to_string())
            .collect();

        let compaction_stats = session_parts
            .iter()
            .find(|p| p.part_type == "compaction_stats")
            .and_then(|p| serde_json::from_value::<CompactionStats>(p.data.clone()).ok());

        let was_interrupted = session_parts.iter().any(|p| {
            p.part_type == "status"
                && p.data
                    .get("state")
                    .and_then(|value| value.as_str())
                    .is_some_and(|state| state == "interrupted")
        });

        let role = match msg.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => return Err(anyhow::anyhow!("Unknown role: {}", msg.role)),
        };

        Ok(SessionMessage {
            id: msg.id,
            role,
            content,
            reasoning,
            parts: session_parts,
            timestamp: std::time::UNIX_EPOCH + std::time::Duration::from_secs(msg.timestamp as u64),
            is_complete: true,
            agent_mode: msg.agent_mode.clone(),
            token_count: if msg.tokens_used > 0 {
                Some(msg.tokens_used as usize)
            } else {
                None
            },
            duration_ms: if msg.duration_ms > 0 {
                Some(msg.duration_ms as u64)
            } else {
                None
            },
            reasoning_started_at: None,
            t0_ms: msg
                .t0_ms
                .and_then(|v| if v > 0 { Some(v as u64) } else { None }),
            t1_ms: msg
                .t1_ms
                .and_then(|v| if v > 0 { Some(v as u64) } else { None }),
            tn_ms: msg
                .tn_ms
                .and_then(|v| if v > 0 { Some(v as u64) } else { None }),
            output_tokens: msg
                .output_tokens
                .and_then(|v| if v > 0 { Some(v as usize) } else { None }),
            input_tokens: msg
                .input_tokens
                .and_then(|v| if v > 0 { Some(v as usize) } else { None }),
            cache_read_tokens: msg.cache_read_tokens.and_then(|v| {
                if v > 0 {
                    Some(v as usize)
                } else {
                    None
                }
            }),
            cache_write_tokens: msg.cache_write_tokens.and_then(|v| {
                if v > 0 {
                    Some(v as usize)
                } else {
                    None
                }
            }),
            cost: msg.cost,
            usage_authoritative: msg.usage_authoritative,
            tokens_per_sec: None,
            model: msg.model.clone(),
            provider: msg.provider.clone(),
            local_image_paths,
            local_audio_paths,
            compaction_stats,
            was_interrupted,
        })
    }
}

pub fn session_to_persistence(name: String, session: &Session) -> (String, Vec<Message>) {
    let messages: Vec<Message> = session.messages.iter().map(|m| m.clone().into()).collect();
    (name, messages)
}

pub fn persistence_to_session(
    persistence_session: PersistenceSession,
    messages: Vec<Message>,
) -> Result<Session, anyhow::Error> {
    let mut session = Session::new();
    session.parent_id = persistence_session.parent_session_identifier;
    for msg in messages {
        session.add_message(msg.try_into()?);
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_id_round_trips_through_persistence() {
        let session_message = SessionMessage::assistant("hello");
        let id = session_message.id.clone();

        let persistence_message: Message = session_message.into();
        assert_eq!(persistence_message.id, id);

        let restored = SessionMessage::try_from(persistence_message).unwrap();
        assert_eq!(restored.id, id);
    }

    #[test]
    fn authoritative_usage_round_trips_through_persistence() {
        let mut session_message = SessionMessage::assistant("hello");
        session_message.apply_usage(
            crate::aisdk::chunk::LanguageModelUsage {
                input_tokens: 100,
                output_tokens: 25,
                cache_read_tokens: 60,
                cache_write_tokens: 10,
            },
            Some(0.0125),
        );

        let restored = SessionMessage::try_from(Message::from(session_message)).unwrap();
        assert_eq!(restored.input_tokens, Some(100));
        assert_eq!(restored.output_tokens, Some(25));
        assert_eq!(restored.cache_read_tokens, Some(60));
        assert_eq!(restored.cache_write_tokens, Some(10));
        assert_eq!(restored.cost, Some(0.0125));
        assert!(restored.usage_authoritative);
    }

    #[test]
    fn audio_paths_round_trip_through_persistence() {
        let mut session_message = SessionMessage::user("listen");
        session_message.local_audio_paths = vec!["/tmp/audio.wav".to_string()];

        let restored = SessionMessage::try_from(Message::from(session_message)).unwrap();
        assert_eq!(restored.local_audio_paths, vec!["/tmp/audio.wav"]);
    }

    #[test]
    fn compaction_stats_round_trip_through_message_parts() {
        let stats = CompactionStats {
            before_tokens: 12_000,
            after_tokens: 360,
            before_messages: 8,
            after_messages: 2,
        };
        let mut session_message = SessionMessage::user("summary");
        session_message.compaction_stats = Some(stats);

        let persistence_message: Message = session_message.into();
        assert!(persistence_message
            .parts
            .iter()
            .any(|part| part.part_type == "compaction_stats"));

        let restored = SessionMessage::try_from(persistence_message).unwrap();
        assert_eq!(restored.compaction_stats, Some(stats));
    }

    #[test]
    fn interrupted_status_round_trips_through_message_parts() {
        let mut session_message = SessionMessage::assistant("partial");
        session_message.mark_interrupted();

        let persistence_message: Message = session_message.into();
        assert!(persistence_message.parts.iter().any(|part| {
            part.part_type == "status"
                && part.data.get("state").and_then(|value| value.as_str()) == Some("interrupted")
        }));

        let restored = SessionMessage::try_from(persistence_message).unwrap();
        assert!(restored.was_interrupted);
    }

    #[test]
    fn assistant_ordered_parts_round_trip_without_reordering() {
        let mut session_message = SessionMessage::incomplete("");
        session_message.append_reasoning("thinking");
        session_message.append("I will inspect.");
        session_message.add_tool_call_part(
            "call_read",
            "read",
            serde_json::json!({ "path": "src/lib.rs" }),
        );
        session_message.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_read",
            "name": "read",
            "status": "ok",
            "args": { "path": "src/lib.rs" },
            "output_preview": "contents",
        }));
        session_message.append("Done.");

        let persistence_message: Message = session_message.into();
        let restored = SessionMessage::try_from(persistence_message).unwrap();

        assert_eq!(
            restored
                .parts
                .iter()
                .map(|part| part.part_type.as_str())
                .collect::<Vec<_>>(),
            vec!["reasoning", "text", "tool_call", "tool_result", "text"]
        );
        assert_eq!(restored.reasoning.as_deref(), Some("thinking"));
        assert_eq!(restored.content, "I will inspect.\n\nDone.");
    }
}
