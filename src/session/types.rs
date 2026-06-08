use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::ops::Range;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Streaming,
    Waiting,
    Failed,
    Interrupted,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Streaming => "streaming",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "streaming" => Self::Streaming,
            "waiting" => Self::Waiting,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            _ => Self::Idle,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Streaming | Self::Waiting)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessagePart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(flatten)]
    pub data: JsonValue,
}

impl MessagePart {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            part_type: "text".to_string(),
            data: serde_json::json!({ "text": text.into() }),
        }
    }

    pub fn reasoning(text: impl Into<String>) -> Self {
        Self {
            part_type: "reasoning".to_string(),
            data: serde_json::json!({ "text": text.into() }),
        }
    }

    pub fn tool_call(id: impl Into<String>, name: impl Into<String>, args: JsonValue) -> Self {
        Self {
            part_type: "tool_call".to_string(),
            data: serde_json::json!({
                "id": id.into(),
                "name": name.into(),
                "status": "running",
                "args": args,
            }),
        }
    }

    pub fn tool_result(data: JsonValue) -> Self {
        Self {
            part_type: "tool_result".to_string(),
            data,
        }
    }

    pub fn text_value(&self) -> Option<&str> {
        self.data.get("text").and_then(|value| value.as_str())
    }

    pub fn tool_id(&self) -> Option<&str> {
        self.data
            .get("id")
            .or_else(|| self.data.get("call_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
    }

    pub fn tool_name(&self) -> Option<&str> {
        self.data
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
    }

    pub fn tool_status(&self) -> Option<&str> {
        self.data.get("status").and_then(|value| value.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompactionStats {
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub before_messages: usize,
    pub after_messages: usize,
}

impl CompactionStats {
    pub fn saved_tokens(self) -> usize {
        self.before_tokens.saturating_sub(self.after_tokens)
    }

    pub fn grew_tokens(self) -> usize {
        self.after_tokens.saturating_sub(self.before_tokens)
    }

    pub fn reduction_percent(self) -> u32 {
        if self.before_tokens == 0 {
            return 0;
        }

        ((self.saved_tokens() as f64 / self.before_tokens as f64) * 100.0).round() as u32
    }

    pub fn growth_percent(self) -> u32 {
        if self.before_tokens == 0 {
            return 0;
        }

        ((self.grew_tokens() as f64 / self.before_tokens as f64) * 100.0).round() as u32
    }

    pub fn change_description(self) -> String {
        if self.after_tokens > self.before_tokens {
            format!("grew {}%", self.growth_percent())
        } else if self.after_tokens < self.before_tokens {
            format!("saved {}%", self.reduction_percent())
        } else {
            "no change".to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub reasoning: Option<String>,
    pub parts: Vec<MessagePart>,
    pub timestamp: SystemTime,
    pub is_complete: bool,
    pub agent_mode: Option<String>,
    pub token_count: Option<usize>,
    pub duration_ms: Option<u64>,
    // Streaming timing primitives (epoch milliseconds)
    // Used to derive TTFT/TPS/full latency.
    pub t0_ms: Option<u64>,
    pub t1_ms: Option<u64>,
    pub tn_ms: Option<u64>,
    pub output_tokens: Option<usize>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub local_image_paths: Vec<String>,
    pub compaction_stats: Option<CompactionStats>,
    pub was_interrupted: bool,
}

impl Message {
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        let content = content.into();
        let parts = if content.is_empty() {
            Vec::new()
        } else {
            vec![MessagePart::text(content.clone())]
        };

        Self {
            role,
            content,
            reasoning: None,
            parts,
            timestamp: SystemTime::now(),
            is_complete: true,
            agent_mode: None,
            token_count: None,
            duration_ms: None,
            t0_ms: None,
            t1_ms: None,
            tn_ms: None,
            output_tokens: None,
            model: None,
            provider: None,
            local_image_paths: Vec::new(),
            compaction_stats: None,
            was_interrupted: false,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(MessageRole::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(MessageRole::Assistant, content)
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(MessageRole::System, content)
    }

    pub fn tool(content: impl Into<String>) -> Self {
        Self::new(MessageRole::Tool, content)
    }

    pub fn incomplete(content: impl Into<String>) -> Self {
        let content = content.into();
        let parts = if content.is_empty() {
            Vec::new()
        } else {
            vec![MessagePart::text(content.clone())]
        };

        Self {
            role: MessageRole::Assistant,
            content,
            reasoning: None,
            parts,
            timestamp: SystemTime::now(),
            is_complete: false,
            agent_mode: None,
            token_count: None,
            duration_ms: None,
            t0_ms: None,
            t1_ms: None,
            tn_ms: None,
            output_tokens: None,
            model: None,
            provider: None,
            local_image_paths: Vec::new(),
            compaction_stats: None,
            was_interrupted: false,
        }
    }

    pub fn append(&mut self, chunk: impl AsRef<str>) {
        let chunk = chunk.as_ref();
        if chunk.is_empty() {
            return;
        }

        let starts_new_text_part = !self
            .parts
            .last()
            .is_some_and(|part| part.part_type == "text");

        if starts_new_text_part && !self.content.trim().is_empty() {
            self.content.push_str("\n\n");
        }
        self.content.push_str(chunk);

        if let Some(part) = self
            .parts
            .last_mut()
            .filter(|part| part.part_type == "text")
        {
            let current = part.data.get("text").and_then(|value| value.as_str());
            let mut text = current.unwrap_or_default().to_string();
            text.push_str(chunk);
            part.data = serde_json::json!({ "text": text });
        } else {
            self.parts.push(MessagePart::text(chunk));
        }
    }

    pub fn append_reasoning(&mut self, chunk: impl AsRef<str>) {
        let chunk = chunk.as_ref();
        if chunk.is_empty() {
            return;
        }

        if let Some(ref mut reasoning) = self.reasoning {
            reasoning.push_str(chunk);
        } else {
            self.reasoning = Some(chunk.to_string());
        }

        if let Some(part) = self
            .parts
            .last_mut()
            .filter(|part| part.part_type == "reasoning")
        {
            let current = part.data.get("text").and_then(|value| value.as_str());
            let mut text = current.unwrap_or_default().to_string();
            text.push_str(chunk);
            part.data = serde_json::json!({ "text": text });
        } else {
            self.parts.push(MessagePart::reasoning(chunk));
        }
    }

    pub fn add_tool_call_part(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        args: JsonValue,
    ) {
        self.parts.push(MessagePart::tool_call(id, name, args));
    }

    pub fn add_or_update_tool_result_part(&mut self, payload: JsonValue) {
        let Some(call_id) = payload
            .get("id")
            .or_else(|| payload.get("call_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.to_string())
        else {
            self.parts.push(MessagePart::tool_result(payload));
            return;
        };

        if let Some(part) = self.parts.iter_mut().find(|part| {
            part.part_type == "tool_result" && part.tool_id() == Some(call_id.as_str())
        }) {
            part.data = payload;
        } else {
            self.parts.push(MessagePart::tool_result(payload));
        }
    }

    pub fn tool_call_part_data(&self, call_id: &str) -> Option<&JsonValue> {
        self.parts.iter().find_map(|part| {
            (part.part_type == "tool_call" && part.tool_id() == Some(call_id)).then_some(&part.data)
        })
    }

    pub fn tool_result_part_data(&self, call_id: &str) -> Option<&JsonValue> {
        self.parts.iter().find_map(|part| {
            (part.part_type == "tool_result" && part.tool_id() == Some(call_id))
                .then_some(&part.data)
        })
    }

    pub fn has_running_tool_parts(&self) -> bool {
        if self.role != MessageRole::Assistant {
            return false;
        }

        let completed_ids = self
            .parts
            .iter()
            .filter(|part| part.part_type == "tool_result")
            .filter_map(MessagePart::tool_id)
            .collect::<std::collections::HashSet<_>>();

        self.parts.iter().any(|part| match part.part_type.as_str() {
            "tool_call" => part.tool_id().is_some_and(|id| !completed_ids.contains(id)),
            "tool_result" => part
                .tool_status()
                .map(|status| matches!(status, "running" | "pending"))
                .unwrap_or(false),
            _ => false,
        })
    }

    pub fn mark_running_tool_parts_failed(&mut self, error: &str) {
        if self.role != MessageRole::Assistant {
            return;
        }

        let running_calls = self
            .parts
            .iter()
            .filter(|part| part.part_type == "tool_call")
            .filter_map(|part| {
                let id = part.tool_id()?.to_string();
                let name = part.tool_name().unwrap_or("tool").to_string();
                let args = part.data.get("args").cloned();
                Some((id, name, args))
            })
            .collect::<Vec<_>>();

        for (id, name, args) in running_calls {
            let mut payload = self
                .tool_result_part_data(&id)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));

            let is_running = payload
                .get("status")
                .and_then(|status| status.as_str())
                .map(|status| matches!(status, "running" | "pending"))
                .unwrap_or(true);

            if !is_running {
                continue;
            }

            payload["id"] = JsonValue::String(id.clone());
            payload["name"] = JsonValue::String(name);
            if payload.get("args").is_none() {
                if let Some(args) = args {
                    payload["args"] = args;
                }
            }
            payload["status"] = JsonValue::String("error".to_string());
            payload["title"] = JsonValue::String("Tool failed".to_string());
            payload["output_preview"] = JsonValue::String(error.to_string());
            self.add_or_update_tool_result_part(payload);
        }
    }

    pub fn mark_complete(&mut self) {
        self.is_complete = true;
    }

    pub fn mark_interrupted(&mut self) {
        self.was_interrupted = true;
    }
}

pub fn logical_message_block_start(messages: &[Message], idx: usize) -> Option<usize> {
    let message = messages.get(idx)?;

    match message.role {
        MessageRole::User => Some(idx),
        MessageRole::Assistant | MessageRole::System | MessageRole::Tool => {
            let segment_start = previous_user_index(messages, idx)
                .map(|user_idx| user_idx.saturating_add(1))
                .unwrap_or(0);

            (segment_start..=idx)
                .find(|&candidate| matches!(messages[candidate].role, MessageRole::Assistant))
        }
    }
}

pub fn logical_message_block_range(messages: &[Message], idx: usize) -> Option<Range<usize>> {
    let start = logical_message_block_start(messages, idx)?;

    match messages.get(start)?.role {
        MessageRole::User => Some(start..start.saturating_add(1)),
        MessageRole::Assistant => {
            let end = messages
                .iter()
                .enumerate()
                .skip(start.saturating_add(1))
                .find_map(|(candidate, message)| {
                    matches!(message.role, MessageRole::User).then_some(candidate)
                })
                .unwrap_or(messages.len());

            Some(start..end)
        }
        MessageRole::System | MessageRole::Tool => None,
    }
}

fn previous_user_index(messages: &[Message], idx: usize) -> Option<usize> {
    (0..idx)
        .rev()
        .find(|&candidate| matches!(messages[candidate].role, MessageRole::User))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub workspace_id: i64,
    pub workspace_path: String,
    pub workspace_name: String,
    pub workspace_sort_order: i64,
    pub status: SessionStatus,
    pub pinned_at: Option<SystemTime>,
    pub archived_at: Option<SystemTime>,
    pub messages: Vec<Message>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let now = SystemTime::now();
        Self {
            id: cuid2::create_id(),
            parent_id: None,
            title: "New Session".to_string(),
            created_at: now,
            updated_at: now,
            workspace_id: 0,
            workspace_path: String::new(),
            workspace_name: "Workspace".to_string(),
            workspace_sort_order: 0,
            status: SessionStatus::Idle,
            pinned_at: None,
            archived_at: None,
            messages: Vec::new(),
        }
    }

    pub fn with_title(title: impl Into<String>) -> Self {
        let now = SystemTime::now();
        Self {
            id: cuid2::create_id(),
            parent_id: None,
            title: title.into(),
            created_at: now,
            updated_at: now,
            workspace_id: 0,
            workspace_path: String::new(),
            workspace_name: "Workspace".to_string(),
            workspace_sort_order: 0,
            status: SessionStatus::Idle,
            pinned_at: None,
            archived_at: None,
            messages: Vec::new(),
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.updated_at = SystemTime::now();
    }

    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.add_message(Message::user(content));
    }

    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.add_message(Message::assistant(content));
    }

    pub fn get_last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    pub fn get_last_assistant_message_mut(&mut self) -> Option<&mut Message> {
        self.messages
            .iter_mut()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
    }

    pub fn append_to_last_assistant(&mut self, chunk: impl AsRef<str>) {
        if self
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::Assistant)
        {
            if let Some(msg) = self.messages.last_mut() {
                msg.append(chunk);
            }
        } else {
            self.add_assistant_message(chunk.as_ref());
        }
    }

    pub fn append_reasoning_to_last_assistant(&mut self, chunk: impl AsRef<str>) {
        if self
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::Assistant)
        {
            if let Some(msg) = self.messages.last_mut() {
                msg.append_reasoning(chunk);
            }
        } else {
            // Create a new assistant message with reasoning
            let mut msg = Message::incomplete("");
            msg.append_reasoning(chunk);
            self.add_message(msg);
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session() {
        let _session = Session::new();
    }

    #[test]
    fn test_message_new() {
        let msg = Message::new(MessageRole::User, "hello");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "hello");
        assert!(msg.is_complete);
    }

    #[test]
    fn test_message_user() {
        let msg = Message::user("test");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "test");
    }

    #[test]
    fn test_message_assistant() {
        let msg = Message::assistant("response");
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "response");
    }

    #[test]
    fn test_message_system() {
        let msg = Message::system("system prompt");
        assert_eq!(msg.role, MessageRole::System);
        assert_eq!(msg.content, "system prompt");
    }

    #[test]
    fn test_message_tool() {
        let msg = Message::tool("tool output");
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.content, "tool output");
    }

    #[test]
    fn test_message_incomplete() {
        let msg = Message::incomplete("partial");
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "partial");
        assert!(!msg.is_complete);
    }

    #[test]
    fn test_message_append() {
        let mut msg = Message::incomplete("hello");
        msg.append(" world");
        assert_eq!(msg.content, "hello world");
        assert!(!msg.is_complete);
    }

    #[test]
    fn test_message_mark_complete() {
        let mut msg = Message::incomplete("test");
        msg.mark_complete();
        assert!(msg.is_complete);
    }

    #[test]
    fn test_session_new() {
        let session = Session::new();
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_session_default() {
        let session = Session::default();
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_session_add_message() {
        let mut session = Session::new();
        session.add_message(Message::user("hello"));
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "hello");
    }

    #[test]
    fn test_session_add_user_message() {
        let mut session = Session::new();
        session.add_user_message("test");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, MessageRole::User);
    }

    #[test]
    fn test_session_add_assistant_message() {
        let mut session = Session::new();
        session.add_assistant_message("response");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, MessageRole::Assistant);
    }

    #[test]
    fn test_session_get_last_message() {
        let mut session = Session::new();
        assert!(session.get_last_message().is_none());

        session.add_user_message("hello");
        assert_eq!(session.get_last_message().unwrap().content, "hello");

        session.add_assistant_message("hi there");
        assert_eq!(session.get_last_message().unwrap().content, "hi there");
    }

    #[test]
    fn test_session_get_last_assistant_message_mut() {
        let mut session = Session::new();
        assert!(session.get_last_assistant_message_mut().is_none());

        session.add_user_message("hello");
        assert!(session.get_last_assistant_message_mut().is_none());

        session.add_assistant_message("response");
        assert_eq!(
            session.get_last_assistant_message_mut().unwrap().content,
            "response"
        );

        session.add_user_message("another");
        assert_eq!(
            session.get_last_assistant_message_mut().unwrap().content,
            "response"
        );
    }

    #[test]
    fn test_session_append_to_last_assistant() {
        let mut session = Session::new();

        session.append_to_last_assistant("hello");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "hello");

        session.append_to_last_assistant(" world");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "hello world");

        session.add_user_message("user");
        session.append_to_last_assistant(" assistant");
        assert_eq!(session.messages.len(), 3);
        assert_eq!(session.messages[2].content, " assistant");
    }

    #[test]
    fn test_session_clear() {
        let mut session = Session::new();
        session.add_user_message("hello");
        session.add_assistant_message("hi");
        assert_eq!(session.messages.len(), 2);

        session.clear();
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_message_role_partial_eq() {
        assert_eq!(MessageRole::User, MessageRole::User);
        assert_eq!(MessageRole::Assistant, MessageRole::Assistant);
        assert_ne!(MessageRole::User, MessageRole::Assistant);
    }

    #[test]
    fn logical_message_block_range_groups_assistant_turn_parts() {
        let messages = vec![
            Message::user("Prompt"),
            Message::assistant(""),
            Message::tool("tool call"),
            Message::assistant("Final answer"),
            Message::user("Next prompt"),
        ];

        assert_eq!(logical_message_block_range(&messages, 0), Some(0..1));
        assert_eq!(logical_message_block_range(&messages, 1), Some(1..4));
        assert_eq!(logical_message_block_range(&messages, 2), Some(1..4));
        assert_eq!(logical_message_block_range(&messages, 3), Some(1..4));
        assert_eq!(logical_message_block_range(&messages, 4), Some(4..5));
    }

    #[test]
    fn logical_message_block_range_ignores_orphan_tool_rows() {
        let messages = vec![Message::tool("orphan"), Message::user("Prompt")];

        assert_eq!(logical_message_block_range(&messages, 0), None);
        assert_eq!(logical_message_block_range(&messages, 1), Some(1..2));
    }

    #[test]
    fn test_message_partial_eq() {
        let msg1 = Message::user("hello");
        let msg2 = Message::user("hello");
        let msg3 = Message::user("world");

        assert_eq!(msg1.role, msg2.role);
        assert_eq!(msg1.content, msg2.content);
        assert_eq!(msg1.role, msg3.role);
        assert_ne!(msg1.content, msg3.content);
    }
}
