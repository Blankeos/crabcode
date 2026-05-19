use crate::persistence::{Message, MessagePart, Session as PersistenceSession};
use crate::session::types::{CompactionStats, Message as SessionMessage, MessageRole, Session};

impl From<SessionMessage> for Message {
    fn from(msg: SessionMessage) -> Self {
        let mut parts = vec![MessagePart {
            part_type: "text".to_string(),
            data: serde_json::json!({ "text": msg.content }),
        }];

        // Add reasoning as a separate part if present
        if let Some(ref reasoning) = msg.reasoning {
            if !reasoning.is_empty() {
                parts.push(MessagePart {
                    part_type: "reasoning".to_string(),
                    data: serde_json::json!({ "text": reasoning }),
                });
            }
        }

        for path in &msg.local_image_paths {
            parts.push(MessagePart {
                part_type: "local_image".to_string(),
                data: serde_json::json!({ "path": path }),
            });
        }

        if let Some(stats) = msg.compaction_stats {
            if let Ok(data) = serde_json::to_value(stats) {
                parts.push(MessagePart {
                    part_type: "compaction_stats".to_string(),
                    data,
                });
            }
        }

        Message {
            id: cuid2::create_id(),
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
        }
    }
}

impl TryFrom<Message> for SessionMessage {
    type Error = anyhow::Error;

    fn try_from(msg: Message) -> Result<Self, Self::Error> {
        // Extract content from text parts
        let content = msg
            .parts
            .iter()
            .filter_map(|p| {
                if p.part_type == "text" {
                    p.data.get("text").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Extract reasoning from reasoning parts
        let reasoning = msg
            .parts
            .iter()
            .find(|p| p.part_type == "reasoning")
            .and_then(|p| p.data.get("text").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        let local_image_paths = msg
            .parts
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

        let compaction_stats = msg
            .parts
            .iter()
            .find(|p| p.part_type == "compaction_stats")
            .and_then(|p| serde_json::from_value::<CompactionStats>(p.data.clone()).ok());

        let role = match msg.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => return Err(anyhow::anyhow!("Unknown role: {}", msg.role)),
        };

        Ok(SessionMessage {
            role,
            content,
            reasoning,
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
            model: msg.model.clone(),
            provider: msg.provider.clone(),
            local_image_paths,
            compaction_stats,
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
}
