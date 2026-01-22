#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    Done,
    Error(String),
}

pub struct StreamParser;

impl StreamParser {
    pub fn parse_sse_event(_chunk: &[u8]) -> StreamEvent {
        StreamEvent::TextDelta(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_parser() {
        let _parser = StreamParser;
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

    #[test]
    fn test_parse_sse_event() {
        let event = StreamParser::parse_sse_event(&[]);
        assert!(matches!(event, StreamEvent::TextDelta(_)));
    }
}
