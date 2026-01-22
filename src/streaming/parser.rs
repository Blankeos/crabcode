#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    Done,
    Error(String),
}

pub struct StreamParser {
    buffer: String,
}

impl StreamParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn parse_chunk(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        let text = String::from_utf8_lossy(chunk);
        self.buffer.push_str(&text);
        self.parse_events()
    }

    fn parse_events(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        let mut current_data = String::new();

        for line in self.buffer.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    events.push(StreamEvent::Done);
                    continue;
                }
                if !current_data.is_empty() {
                    current_data.push('\n');
                }
                current_data.push_str(data);
            } else if line.is_empty() && !current_data.is_empty() {
                if let Ok(parsed) = self.parse_data(&current_data) {
                    events.push(parsed);
                }
                current_data.clear();
            }
        }

        if !current_data.is_empty() {
            if let Ok(parsed) = self.parse_data(&current_data) {
                events.push(parsed);
            }
        }

        events
    }

    fn parse_data(&self, data: &str) -> Result<StreamEvent, serde_json::Error> {
        #[derive(serde::Deserialize, Default)]
        struct SseData {
            #[serde(default)]
            choices: Vec<Choice>,
        }

        #[derive(serde::Deserialize, Default)]
        struct Choice {
            #[serde(default)]
            delta: Delta,
            #[serde(default)]
            finish_reason: Option<String>,
        }

        #[derive(serde::Deserialize, Default)]
        struct Delta {
            #[serde(default)]
            content: String,
        }

        let sse_data: SseData = serde_json::from_str(data)?;
        let text = sse_data
            .choices
            .first()
            .map(|c| {
                if c.finish_reason.is_some() {
                    "".to_string()
                } else {
                    c.delta.content.clone()
                }
            })
            .unwrap_or_default();

        if sse_data
            .choices
            .first()
            .is_some_and(|c| c.finish_reason.is_some())
        {
            Ok(StreamEvent::Done)
        } else if !text.is_empty() {
            Ok(StreamEvent::TextDelta(text))
        } else {
            Ok(StreamEvent::TextDelta(String::new()))
        }
    }
}

impl Default for StreamParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_parser_new() {
        let parser = StreamParser::new();
        assert!(parser.buffer.is_empty());
    }

    #[test]
    fn test_stream_parser_default() {
        let parser = StreamParser::default();
        assert!(parser.buffer.is_empty());
    }

    #[test]
    fn test_parse_text_delta() {
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";
        let mut parser = StreamParser::new();
        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], StreamEvent::TextDelta("Hello".to_string()));
    }

    #[test]
    fn test_parse_done() {
        let chunk = b"data: [DONE]\n\n";
        let mut parser = StreamParser::new();
        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], StreamEvent::Done);
    }

    #[test]
    fn test_parse_multiple_events() {
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" World\"}}]}\n\n";
        let mut parser = StreamParser::new();
        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], StreamEvent::TextDelta("Hello".to_string()));
        assert_eq!(events[1], StreamEvent::TextDelta(" World".to_string()));
    }

    #[test]
    fn test_parse_finish_reason() {
        let chunk = b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
        let mut parser = StreamParser::new();
        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], StreamEvent::Done);
    }

    #[test]
    fn test_parse_empty_content() {
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n";
        let mut parser = StreamParser::new();
        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], StreamEvent::TextDelta("".to_string()));
    }

    #[test]
    fn test_parse_invalid_json() {
        let chunk = b"data: invalid\n\n";
        let mut parser = StreamParser::new();
        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn test_stream_event_text_delta() {
        let event = StreamEvent::TextDelta("hello".to_string());
        assert_eq!(event, StreamEvent::TextDelta("hello".to_string()));
    }

    #[test]
    fn test_stream_event_done() {
        let event = StreamEvent::Done;
        assert_eq!(event, StreamEvent::Done);
    }

    #[test]
    fn test_stream_event_error() {
        let event = StreamEvent::Error("test error".to_string());
        assert_eq!(event, StreamEvent::Error("test error".to_string()));
    }
}
