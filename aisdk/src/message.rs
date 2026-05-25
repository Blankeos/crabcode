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
    #[serde(rename = "tool_call")]
    ToolCall(ToolCallMessage),
    #[serde(rename = "tool_output")]
    ToolOutput(ToolOutputMessage),
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
            images: Vec::new(),
        })
    }

    pub fn user_with_images(content: impl Into<String>, images: Vec<ImageContent>) -> Self {
        Self::User(UserMessage {
            content: content.into(),
            images,
        })
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant(AssistantMessage {
            content: content.into(),
        })
    }

    pub fn tool_call(
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self::ToolCall(ToolCallMessage {
            item_id: None,
            call_id: call_id.into(),
            name: name.into(),
            arguments: arguments.into(),
        })
    }

    pub fn tool_call_with_item_id(
        item_id: impl Into<String>,
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self::ToolCall(ToolCallMessage {
            item_id: Some(item_id.into()),
            call_id: call_id.into(),
            name: name.into(),
            arguments: arguments.into(),
        })
    }

    pub fn tool_output(
        call_id: impl Into<String>,
        name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::tool_output_with_images(call_id, name, output, Vec::new(), is_error)
    }

    pub fn tool_output_with_images(
        call_id: impl Into<String>,
        name: impl Into<String>,
        output: impl Into<String>,
        images: Vec<ImageContent>,
        is_error: bool,
    ) -> Self {
        Self::ToolOutput(ToolOutputMessage {
            call_id: call_id.into(),
            name: name.into(),
            output: output.into(),
            images,
            is_error,
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
    #[serde(default)]
    pub images: Vec<ImageContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    pub data_url: String,
    pub media_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutputMessage {
    pub call_id: String,
    pub name: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,
    #[serde(default)]
    pub is_error: bool,
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
        Self {
            content,
            images: Vec::new(),
        }
    }
}

impl From<&str> for UserMessage {
    fn from(content: &str) -> Self {
        Self {
            content: content.to_string(),
            images: Vec::new(),
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
