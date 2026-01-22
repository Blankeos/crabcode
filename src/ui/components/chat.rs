use crate::session::types::{Message, MessageRole};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

#[derive(Debug, Clone, Default)]
pub struct Chat {
    pub messages: Vec<Message>,
    pub scroll_offset: usize,
}

impl Chat {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_messages(messages: Vec<Message>) -> Self {
        Self {
            messages,
            scroll_offset: 0,
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.scroll_to_bottom();
    }

    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.add_message(Message::user(content));
    }

    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.add_message(Message::assistant(content));
    }

    pub fn append_to_last_assistant(&mut self, chunk: impl AsRef<str>) {
        if self
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::Assistant)
        {
            if let Some(msg) = self.messages.last_mut() {
                msg.append(chunk);
                self.scroll_to_bottom();
            }
        } else {
            self.add_assistant_message(chunk.as_ref());
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn render(&self, f: &mut ratatui::Frame, area: Rect) {
        let text = self.render_messages(area.height as usize);

        let paragraph = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_offset as u16, 0));

        f.render_widget(paragraph, area);
    }

    fn render_messages(&self, max_height: usize) -> Text<'_> {
        let mut lines = Vec::new();

        for message in &self.messages {
            let role_lines = self.format_message(message, max_height);
            lines.extend(role_lines);
        }

        Text::from(lines)
    }

    fn format_message<'a>(&self, message: &'a Message, _max_height: usize) -> Vec<Line<'a>> {
        let mut lines = Vec::new();

        let (prefix, color) = match message.role {
            MessageRole::User => ("You", Color::Cyan),
            MessageRole::Assistant => ("AI", Color::Green),
            MessageRole::System => ("System", Color::Yellow),
            MessageRole::Tool => ("Tool", Color::Gray),
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("[{}] ", prefix),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(&message.content),
        ]));

        lines.push(Line::from(""));

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_new() {
        let chat = Chat::new();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_chat_default() {
        let chat = Chat::default();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_chat_with_messages() {
        let messages = vec![Message::user("hello"), Message::assistant("hi there")];
        let chat = Chat::with_messages(messages.clone());
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].content, "hello");
        assert_eq!(chat.messages[1].content, "hi there");
    }

    #[test]
    fn test_chat_add_message() {
        let mut chat = Chat::new();
        chat.add_message(Message::user("test"));
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].content, "test");
    }

    #[test]
    fn test_chat_add_user_message() {
        let mut chat = Chat::new();
        chat.add_user_message("hello");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, MessageRole::User);
        assert_eq!(chat.messages[0].content, "hello");
    }

    #[test]
    fn test_chat_add_assistant_message() {
        let mut chat = Chat::new();
        chat.add_assistant_message("response");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, MessageRole::Assistant);
        assert_eq!(chat.messages[0].content, "response");
    }

    #[test]
    fn test_chat_append_to_last_assistant() {
        let mut chat = Chat::new();

        chat.append_to_last_assistant("hello");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].content, "hello");

        chat.append_to_last_assistant(" world");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].content, "hello world");

        chat.add_user_message("user");
        chat.append_to_last_assistant(" assistant");
        assert_eq!(chat.messages.len(), 3);
        assert_eq!(chat.messages[2].content, " assistant");
    }

    #[test]
    fn test_chat_clear() {
        let mut chat = Chat::new();
        chat.add_user_message("hello");
        chat.add_assistant_message("hi");
        assert_eq!(chat.messages.len(), 2);

        chat.clear();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_chat_scroll_down() {
        let mut chat = Chat::new();
        chat.scroll_down(5);
        assert_eq!(chat.scroll_offset, 5);

        chat.scroll_down(3);
        assert_eq!(chat.scroll_offset, 8);
    }

    #[test]
    fn test_chat_scroll_up() {
        let mut chat = Chat::new();
        chat.scroll_offset = 10;
        chat.scroll_up(3);
        assert_eq!(chat.scroll_offset, 7);

        chat.scroll_up(10);
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_chat_scroll_to_bottom() {
        let mut chat = Chat::new();
        chat.scroll_offset = 10;
        chat.scroll_to_bottom();
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_chat_scroll_to_bottom_after_add() {
        let mut chat = Chat::new();
        chat.scroll_down(10);
        chat.add_user_message("test");
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_chat_scroll_to_bottom_after_append() {
        let mut chat = Chat::new();
        chat.add_assistant_message("partial");
        chat.scroll_down(10);
        chat.append_to_last_assistant(" content");
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_format_message_user() {
        let chat = Chat::new();
        let msg = Message::user("hello world");
        let lines = chat.format_message(&msg, 100);

        assert_eq!(lines.len(), 2);
        assert!(lines[0].spans[0].content.contains("[You]"));
        assert!(lines[0].spans[1].content.contains("hello world"));
    }

    #[test]
    fn test_format_message_assistant() {
        let chat = Chat::new();
        let msg = Message::assistant("response");
        let lines = chat.format_message(&msg, 100);

        assert_eq!(lines.len(), 2);
        assert!(lines[0].spans[0].content.contains("[AI]"));
        assert!(lines[0].spans[1].content.contains("response"));
    }

    #[test]
    fn test_format_message_system() {
        let chat = Chat::new();
        let msg = Message::system("system prompt");
        let lines = chat.format_message(&msg, 100);

        assert_eq!(lines.len(), 2);
        assert!(lines[0].spans[0].content.contains("[System]"));
    }

    #[test]
    fn test_format_message_tool() {
        let chat = Chat::new();
        let msg = Message::tool("tool output");
        let lines = chat.format_message(&msg, 100);

        assert_eq!(lines.len(), 2);
        assert!(lines[0].spans[0].content.contains("[Tool]"));
    }

    #[test]
    fn test_chat_multiple_messages() {
        let mut chat = Chat::new();
        chat.add_user_message("hello");
        chat.add_assistant_message("hi");
        chat.add_user_message("how are you?");

        assert_eq!(chat.messages.len(), 3);
        assert_eq!(chat.messages[0].content, "hello");
        assert_eq!(chat.messages[1].content, "hi");
        assert_eq!(chat.messages[2].content, "how are you?");
    }

    #[test]
    fn test_chat_clone() {
        let mut chat1 = Chat::new();
        chat1.add_user_message("test");

        let chat2 = chat1.clone();
        assert_eq!(chat1.messages.len(), chat2.messages.len());
        assert_eq!(chat1.messages[0].content, chat2.messages[0].content);
    }
}
