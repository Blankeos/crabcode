use crate::persistence::{Message, MessagePart, Session as PersistenceSession};
use crate::session::types::{Message as SessionMessage, MessageRole, Session};

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
            model: msg.model.clone(),
            provider: msg.provider.clone(),
        })
    }
}

pub fn session_to_persistence(name: String, session: &Session) -> (String, Vec<Message>) {
    let messages: Vec<Message> = session.messages.iter().map(|m| m.clone().into()).collect();
    (name, messages)
}

pub fn persistence_to_session(
    _persistence_session: PersistenceSession,
    messages: Vec<Message>,
) -> Result<Session, anyhow::Error> {
    let mut session = Session::new();
    for msg in messages {
        session.add_message(msg.try_into()?);
    }
    Ok(session)
}
