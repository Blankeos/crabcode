use super::{
    compaction::{context_messages, message_context_tokens},
    types::{Message, MessageRole},
};

/// Counts active context without recounting completed messages on every stream update.
///
/// Session changes, appends, compaction, and changes to the active streaming message
/// refresh the cached base automatically. Replace with `Self::default()` after
/// editing existing completed messages in place.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct StreamingContextTokens {
    base: Option<CachedBase>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CachedBase {
    session_id: Option<String>,
    message_count: usize,
    context_start: usize,
    streaming_idx: Option<usize>,
    tokens: usize,
}

impl StreamingContextTokens {
    /// Substitute live tokens for the last incomplete assistant in active context.
    /// Without one, ignore `streaming_tokens` and count the stored context normally.
    pub fn count(
        &mut self,
        session_id: Option<&str>,
        messages: &[Message],
        streaming_tokens: usize,
    ) -> usize {
        let context = context_messages(messages);
        let context_start = messages.len() - context.len();
        let streaming_idx = context
            .iter()
            .rposition(|message| message.role == MessageRole::Assistant && !message.is_complete);
        let cache_valid = self.base.as_ref().is_some_and(|base| {
            base.session_id.as_deref() == session_id
                && base.message_count == messages.len()
                && base.context_start == context_start
                && base.streaming_idx == streaming_idx
        });
        if !cache_valid {
            let tokens = context
                .iter()
                .enumerate()
                .filter(|(idx, _)| Some(*idx) != streaming_idx)
                .map(|(_, message)| message_context_tokens(message))
                .sum();
            self.base = Some(CachedBase {
                session_id: session_id.map(str::to_owned),
                message_count: messages.len(),
                context_start,
                streaming_idx,
                tokens,
            });
        }

        let tokens = self.base.as_ref().map_or(0, |base| base.tokens);
        if streaming_idx.is_some() {
            tokens.saturating_add(streaming_tokens)
        } else {
            tokens
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{
        compaction::{compaction_marker, total_context_tokens, SUMMARY_PREFIX},
        types::CompactionStats,
    };

    fn completed(tokens: usize) -> Message {
        let mut message = Message::assistant("finished answer");
        message.token_count = Some(tokens);
        message
    }

    fn marker() -> Message {
        compaction_marker(CompactionStats {
            before_tokens: 100_000,
            after_tokens: 100,
            before_messages: 1,
            after_messages: 1,
        })
    }

    #[test]
    fn completed_messages_and_appends() {
        let mut counter = StreamingContextTokens::default();
        let prompt = Message::user("hello there");
        let base = message_context_tokens(&prompt) + 100;
        let mut messages = vec![prompt, completed(100), Message::incomplete("streaming...")];
        assert_eq!(counter.count(Some("session"), &messages, 7), base + 7);
        assert_eq!(counter.count(Some("session"), &messages, 7), base + 7);
        assert_eq!(counter.count(Some("session"), &messages, 19), base + 19);

        messages.insert(2, completed(40));
        assert_eq!(counter.count(Some("session"), &messages, 19), base + 59);
        messages.push(completed(20));
        assert_eq!(counter.count(Some("session"), &messages, 19), base + 79);
    }

    #[test]
    fn switches_sessions_with_the_same_message_layout() {
        let mut counter = StreamingContextTokens::default();
        let first = vec![completed(100), Message::incomplete("")];
        let second = vec![completed(200), Message::incomplete("")];
        assert_eq!(counter.count(Some("first"), &first, 5), 105);
        assert_eq!(counter.count(Some("second"), &second, 5), 205);
        assert_eq!(counter.count(None, &first, 5), 105);
        assert_eq!(counter.count(Some("second"), &second, 5), 205);
    }

    #[test]
    fn no_active_stream_ignores_live_tokens() {
        let mut counter = StreamingContextTokens::default();
        assert_eq!(counter.count(None, &[], 999), 0);
        assert_eq!(counter.count(None, &[completed(100)], 999), 100);
        assert_eq!(counter.count(None, &[completed(100)], 0), 100);
    }

    #[test]
    fn soft_compaction_excludes_archived_partial_and_billed_summary_usage() {
        let mut counter = StreamingContextTokens::default();
        let mut archived = Message::incomplete("old partial answer");
        archived.token_count = Some(100_000);
        let mut summary = Message::user(format!("{SUMMARY_PREFIX}\nsummary"));
        summary.token_count = Some(80_000);
        let mut messages = vec![archived, summary, marker(), Message::user("new prompt")];
        let expected = total_context_tokens(&messages);
        assert!(expected < 1000);
        assert_eq!(counter.count(None, &messages, 777), expected);
        messages.push(Message::incomplete(""));
        assert_eq!(counter.count(None, &messages, 0), expected);
        assert_eq!(counter.count(None, &messages, 17), expected + 17);
        assert_eq!(counter.count(None, &messages, 17), expected + 17);
    }

    #[test]
    fn compaction_refreshes_context_even_with_unchanged_message_count() {
        let mut counter = StreamingContextTokens::default();
        let mut messages = vec![completed(1000), completed(200), completed(300)];
        assert_eq!(counter.count(None, &messages, 0), 1500);
        messages[1] = Message::user(format!("{SUMMARY_PREFIX}\nsummary"));
        messages[2] = marker();
        let expected = message_context_tokens(&messages[1]);
        assert_eq!(counter.count(None, &messages, 0), expected);
    }

    #[test]
    fn only_last_incomplete_assistant_is_replaced_and_completion_refreshes_base() {
        let mut counter = StreamingContextTokens::default();
        let mut earlier = Message::incomplete("earlier partial");
        earlier.token_count = Some(30);
        let mut latest = Message::incomplete("latest partial");
        latest.token_count = Some(50);
        let mut messages = vec![earlier, latest];
        assert_eq!(counter.count(None, &messages, 7), 37);

        messages[1].is_complete = true;
        assert_eq!(counter.count(None, &messages, 7), 57);
        messages[0].is_complete = true;
        assert_eq!(counter.count(None, &messages, 777), 80);
        messages[1].is_complete = false;
        assert_eq!(counter.count(None, &messages, 9), 39);
    }

    #[test]
    fn reset_refreshes_in_place_edits_to_completed_messages() {
        let mut counter = StreamingContextTokens::default();
        let mut messages = vec![completed(100), Message::incomplete("")];
        assert_eq!(counter.count(None, &messages, 7), 107);
        messages[0].token_count = Some(200);
        counter = StreamingContextTokens::default();
        assert_eq!(counter.count(None, &messages, 7), 207);
    }

    #[test]
    fn adding_live_tokens_saturates() {
        let mut counter = StreamingContextTokens::default();
        let messages = vec![completed(1), Message::incomplete("")];
        assert_eq!(counter.count(None, &messages, usize::MAX), usize::MAX);
    }
}
