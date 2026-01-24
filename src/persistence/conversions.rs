use crate::persistence::{Message, MessagePart, Session as PersistenceSession};
use crate::session::types::{Message as SessionMessage, MessageRole, Session};

impl From<SessionMessage> for Message {
    fn from(msg: SessionMessage) -> Self {
        Message {
            id: cuid2::create_id(),
            session_id: 0,
            role: match msg.role {
                MessageRole::User => "user".to_string(),
                MessageRole::Assistant => "assistant".to_string(),
                MessageRole::System => "system".to_string(),
                MessageRole::Tool => "tool".to_string(),
            },
            parts: vec![MessagePart {
                part_type: "text".to_string(),
                data: serde_json::json!({ "text": msg.content }),
            }],
            timestamp: msg
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            tokens_used: 0,
            model: None,
            provider: None,
        }
    }
}

impl TryFrom<Message> for SessionMessage {
    type Error = anyhow::Error;

    fn try_from(msg: Message) -> Result<Self, Self::Error> {
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
            timestamp: std::time::UNIX_EPOCH + std::time::Duration::from_secs(msg.timestamp as u64),
            is_complete: true,
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
