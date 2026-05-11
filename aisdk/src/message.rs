use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "system")]
    System(SystemMessage),
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System(SystemMessage {
            content: content.into(),
        })
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User(UserMessage {
            content: content.into(),
        })
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant(AssistantMessage {
            content: content.into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: String,
}

impl From<String> for SystemMessage {
    fn from(content: String) -> Self {
        Self { content }
    }
}

impl From<&str> for SystemMessage {
    fn from(content: &str) -> Self {
        Self {
            content: content.to_string(),
        }
    }
}

impl From<String> for UserMessage {
    fn from(content: String) -> Self {
        Self { content }
    }
}

impl From<&str> for UserMessage {
    fn from(content: &str) -> Self {
        Self {
            content: content.to_string(),
        }
    }
}

impl From<String> for AssistantMessage {
    fn from(content: String) -> Self {
        Self { content }
    }
}

impl From<&str> for AssistantMessage {
    fn from(content: &str) -> Self {
        Self {
            content: content.to_string(),
        }
    }
}
