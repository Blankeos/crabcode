use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub reasoning: Option<String>,
    pub timestamp: SystemTime,
    pub is_complete: bool,
    pub agent_mode: Option<String>,
    pub token_count: Option<usize>,
    pub duration_ms: Option<u64>,
    pub model: Option<String>,
    pub provider: Option<String>,
}

impl Message {
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            reasoning: None,
            timestamp: SystemTime::now(),
            is_complete: true,
            agent_mode: None,
            token_count: None,
            duration_ms: None,
            model: None,
            provider: None,
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
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            reasoning: None,
            timestamp: SystemTime::now(),
            is_complete: false,
            agent_mode: None,
            token_count: None,
            duration_ms: None,
            model: None,
            provider: None,
        }
    }

    pub fn append(&mut self, chunk: impl AsRef<str>) {
        self.content.push_str(chunk.as_ref());
    }

    pub fn append_reasoning(&mut self, chunk: impl AsRef<str>) {
        if let Some(ref mut reasoning) = self.reasoning {
            reasoning.push_str(chunk.as_ref());
        } else {
            self.reasoning = Some(chunk.as_ref().to_string());
        }
    }

    pub fn mark_complete(&mut self) {
        self.is_complete = true;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
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
            title: "New Session".to_string(),
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
        }
    }

    pub fn with_title(title: impl Into<String>) -> Self {
        let now = SystemTime::now();
        Self {
            id: cuid2::create_id(),
            title: title.into(),
            created_at: now,
            updated_at: now,
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
