use crate::session::types::{CompactionStats, Message, MessageRole};

pub const DEFAULT_TAIL_TURNS: usize = 2;
pub const SUMMARY_PREFIX: &str = "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:";

const SUMMARIZATION_PROMPT: &str = r#"You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Output exactly this Markdown structure and keep the section order unchanged:

## Goal
- [single-sentence task summary]

## Constraints & Preferences
- [user constraints, preferences, specs, or "(none)"]

## Progress
### Done
- [completed work or "(none)"]

### In Progress
- [current work or "(none)"]

### Blocked
- [blockers or "(none)"]

## Key Decisions
- [decision and why, or "(none)"]

## Next Steps
- [ordered next actions or "(none)"]

## Critical Context
- [important technical facts, errors, open questions, or "(none)"]

## Relevant Files
- [file or directory path: why it matters, or "(none)"]

Rules:
- Keep every section, even when empty.
- Use terse bullets, not prose paragraphs.
- Preserve exact file paths, commands, error strings, and identifiers when known.
- Do not mention the summary process or that context was compacted."#;

const TOOL_OUTPUT_MAX_CHARS: usize = 2_000;

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSelection {
    pub messages_to_summarize: Vec<Message>,
    pub tail_messages: Vec<Message>,
}

pub fn select_messages(messages: &[Message], tail_turns: usize) -> Option<CompactionSelection> {
    if messages.is_empty()
        || !messages
            .iter()
            .any(|msg| matches!(msg.role, MessageRole::User))
    {
        return None;
    }

    let user_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| matches!(msg.role, MessageRole::User).then_some(idx))
        .collect();

    let tail_start = if tail_turns > 0 && user_indices.len() > tail_turns {
        user_indices[user_indices.len() - tail_turns]
    } else {
        messages.len()
    };

    let messages_to_summarize = if tail_start == messages.len() {
        messages.to_vec()
    } else {
        messages[..tail_start].to_vec()
    };
    let tail_messages = if tail_start == messages.len() {
        Vec::new()
    } else {
        messages[tail_start..].to_vec()
    };

    Some(CompactionSelection {
        messages_to_summarize,
        tail_messages,
    })
}

pub fn build_prompt(messages: &[Message]) -> String {
    let mut prompt = String::new();
    prompt.push_str("Summarize the following session transcript.\n\n<session-transcript>\n");

    for (idx, message) in messages.iter().enumerate() {
        let content = message_content_for_prompt(message);
        if content.trim().is_empty() {
            continue;
        }

        prompt.push_str(&format!(
            "\n### Message {} ({})\n{}\n",
            idx + 1,
            role_label(message.role.clone()),
            content
        ));
    }

    prompt.push_str("\n</session-transcript>\n\n");
    prompt.push_str(SUMMARIZATION_PROMPT);
    prompt
}

pub fn build_compacted_messages(
    summary: &str,
    tail_messages: Vec<Message>,
    model: Option<String>,
    provider: Option<String>,
    agent_mode: Option<String>,
    stats: Option<CompactionStats>,
) -> Vec<Message> {
    let mut summary_message = Message::user(format!("{}\n{}", SUMMARY_PREFIX, summary.trim()));
    summary_message.model = model;
    summary_message.provider = provider;
    summary_message.agent_mode = agent_mode;
    summary_message.token_count = Some(estimate_tokens(&summary_message.content));
    summary_message.compaction_stats = stats;
    if let Some(first_tail) = tail_messages.first() {
        summary_message.timestamp = first_tail
            .timestamp
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or(first_tail.timestamp);
    }

    let mut messages = vec![summary_message];
    messages.extend(tail_messages);
    messages
}

pub fn total_context_tokens(messages: &[Message]) -> usize {
    messages.iter().map(message_context_tokens).sum()
}

pub fn message_context_tokens(message: &Message) -> usize {
    message
        .token_count
        .unwrap_or_else(|| estimate_tokens(&message.content))
}

pub fn latest_compaction_stats(messages: &[Message]) -> Option<CompactionStats> {
    messages
        .iter()
        .rev()
        .find_map(|message| message.compaction_stats)
}

pub fn is_compaction_summary(message: &Message) -> bool {
    message.compaction_stats.is_some() || message.content.starts_with(SUMMARY_PREFIX)
}

pub fn format_token_count(count: usize) -> String {
    if count < 1000 {
        return count.to_string();
    }
    if count < 1_000_000 {
        let k = count as f64 / 1000.0;
        return format!("{:.1}K", k);
    }
    let m = count as f64 / 1_000_000.0;
    format!("{:.1}M", m)
}

pub fn format_compaction_stats(stats: CompactionStats) -> String {
    format!(
        "{} -> {}, saved {}%",
        format_token_count(stats.before_tokens),
        format_token_count(stats.after_tokens),
        stats.reduction_percent()
    )
}

fn message_content_for_prompt(message: &Message) -> String {
    let mut content = match message.role {
        MessageRole::Tool => tool_content_for_prompt(&message.content),
        _ => message.content.clone(),
    };

    if !message.local_image_paths.is_empty() {
        if !content.trim().is_empty() {
            content.push('\n');
        }
        content.push_str("Attached local images:\n");
        for path in &message.local_image_paths {
            content.push_str("- ");
            content.push_str(path);
            content.push('\n');
        }
    }

    content
}

fn tool_content_for_prompt(content: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return truncate_chars(content, TOOL_OUTPUT_MAX_CHARS);
    };

    let Some(obj) = value.as_object() else {
        return truncate_chars(content, TOOL_OUTPUT_MAX_CHARS);
    };

    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("ok");
    let mut out = format!("Tool `{}` result ({})", name, status);

    if let Some(title) = obj.get("title").and_then(|v| v.as_str()) {
        out.push_str(": ");
        out.push_str(title);
    }

    if let Some(preview) = obj
        .get("output_preview")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        out.push('\n');
        out.push_str(&truncate_chars(preview, TOOL_OUTPUT_MAX_CHARS));
    }

    out
}

fn role_label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}\n[truncated]", truncated)
    } else {
        truncated
    }
}

fn estimate_tokens(content: &str) -> usize {
    content.chars().count().saturating_add(3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_messages_keeps_recent_tail_turns_when_available() {
        let messages = vec![
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            Message::assistant("a2"),
            Message::user("u3"),
            Message::assistant("a3"),
        ];

        let selected = select_messages(&messages, 2).expect("selection");

        assert_eq!(selected.messages_to_summarize.len(), 2);
        assert_eq!(selected.messages_to_summarize[0].content, "u1");
        assert_eq!(selected.tail_messages.len(), 4);
        assert_eq!(selected.tail_messages[0].content, "u2");
    }

    #[test]
    fn select_messages_summarizes_all_when_shorter_than_tail() {
        let messages = vec![Message::user("u1"), Message::assistant("a1")];

        let selected = select_messages(&messages, 2).expect("selection");

        assert_eq!(selected.messages_to_summarize, messages);
        assert!(selected.tail_messages.is_empty());
    }

    #[test]
    fn build_compacted_messages_prefixes_summary() {
        let compacted = build_compacted_messages(
            "summary",
            vec![Message::user("tail")],
            None,
            None,
            None,
            None,
        );

        assert_eq!(compacted.len(), 2);
        assert!(compacted[0].content.starts_with(SUMMARY_PREFIX));
        assert_eq!(compacted[1].content, "tail");
        assert!(compacted[0].timestamp <= compacted[1].timestamp);
    }

    #[test]
    fn compaction_stats_formats_reduction() {
        let stats = CompactionStats {
            before_tokens: 12_000,
            after_tokens: 360,
            before_messages: 10,
            after_messages: 3,
        };

        assert_eq!(stats.saved_tokens(), 11_640);
        assert_eq!(stats.reduction_percent(), 97);
        assert_eq!(format_compaction_stats(stats), "12.0K -> 360, saved 97%");
    }
}
