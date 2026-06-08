use crate::session::types::{CompactionStats, Message, MessageRole};

pub const DEFAULT_TAIL_TURNS: usize = 2;
pub const SUMMARY_PREFIX: &str = "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:";
pub const COMPACTION_MARKER_CONTENT: &str = "[crabcode:context-compacted]";

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

pub fn select_messages_for_compaction(
    messages: &[Message],
    preferred_tail_turns: usize,
) -> Option<CompactionSelection> {
    for tail_turns in (0..=preferred_tail_turns).rev() {
        let selection = select_messages(messages, tail_turns)?;
        if selection
            .messages_to_summarize
            .iter()
            .any(is_meaningful_for_compaction)
        {
            return Some(selection);
        }
    }

    None
}

pub fn build_prompt(messages: &[Message]) -> String {
    let mut prompt = String::new();
    prompt.push_str("Summarize the following session transcript.\n\n<session-transcript>\n");

    for (idx, message) in messages.iter().enumerate() {
        if is_compaction_marker(message) {
            continue;
        }

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
    if let Some(first_tail) = tail_messages.first() {
        summary_message.timestamp = first_tail
            .timestamp
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or(first_tail.timestamp);
    }

    let mut messages = vec![summary_message];
    messages.extend(tail_messages);
    if let Some(stats) = stats {
        append_compaction_marker(&mut messages, stats);
    }
    messages
}

pub fn total_context_tokens(messages: &[Message]) -> usize {
    messages.iter().map(message_context_tokens).sum()
}

pub fn message_context_tokens(message: &Message) -> usize {
    if is_compaction_marker(message) {
        return 0;
    }

    let part_tokens = message_parts_context_tokens(message);
    if part_tokens > 0 {
        return message
            .token_count
            .map(|token_count| token_count.max(part_tokens))
            .unwrap_or(part_tokens);
    }

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
    message.content.starts_with(SUMMARY_PREFIX)
}

pub fn is_compaction_marker(message: &Message) -> bool {
    message.content == COMPACTION_MARKER_CONTENT && message.compaction_stats.is_some()
}

pub fn is_compaction_display_item(message: &Message) -> bool {
    is_compaction_summary(message) || is_compaction_marker(message)
}

fn is_meaningful_for_compaction(message: &Message) -> bool {
    if is_compaction_display_item(message) {
        return false;
    }

    !message.content.trim().is_empty()
        || message.parts.iter().any(|part| {
            matches!(
                part.part_type.as_str(),
                "text" | "tool_call" | "tool_result"
            )
        })
}

pub fn compaction_marker(stats: CompactionStats) -> Message {
    let mut marker = Message::system(COMPACTION_MARKER_CONTENT);
    marker.compaction_stats = Some(stats);
    marker.token_count = Some(0);
    marker
}

pub fn append_compaction_marker(messages: &mut Vec<Message>, stats: CompactionStats) {
    let mut marker = compaction_marker(stats);
    let now = std::time::SystemTime::now();
    marker.timestamp = messages
        .last()
        .map(|message| {
            if now < message.timestamp {
                message.timestamp
            } else {
                now
            }
        })
        .unwrap_or(now);
    messages.push(marker);
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
        "{} -> {}, {}",
        format_token_count(stats.before_tokens),
        format_token_count(stats.after_tokens),
        stats.change_description()
    )
}

fn message_parts_context_tokens(message: &Message) -> usize {
    if message.parts.is_empty() {
        return 0;
    }

    match message.role {
        MessageRole::Assistant => assistant_parts_context_tokens(message),
        MessageRole::Tool => estimate_tokens(&tool_content_for_prompt(&message.content)),
        _ => estimate_tokens(&message.content),
    }
}

fn assistant_parts_context_tokens(message: &Message) -> usize {
    let tool_call_ids = message
        .parts
        .iter()
        .filter(|part| part.part_type == "tool_call")
        .filter_map(|part| part.tool_id().map(|id| id.to_string()))
        .collect::<std::collections::HashSet<_>>();

    message
        .parts
        .iter()
        .map(|part| match part.part_type.as_str() {
            "text" => part.text_value().map(estimate_tokens).unwrap_or(0),
            "tool_call" => tool_call_context_tokens(part),
            "tool_result" => {
                let mut tokens = tool_result_context_tokens(part);
                if part
                    .tool_id()
                    .map(|id| !tool_call_ids.contains(id))
                    .unwrap_or(true)
                {
                    tokens += tool_call_context_tokens(part);
                }
                tokens
            }
            _ => 0,
        })
        .sum()
}

fn tool_call_context_tokens(part: &crate::session::types::MessagePart) -> usize {
    let Some(args) = part.data.get("args") else {
        return 0;
    };

    estimate_tokens(&serde_json::to_string(args).unwrap_or_else(|_| args.to_string()))
}

fn tool_result_context_tokens(part: &crate::session::types::MessagePart) -> usize {
    part.data
        .get("output_preview")
        .and_then(|value| value.as_str())
        .map(estimate_tokens)
        .unwrap_or(0)
}

fn message_content_for_prompt(message: &Message) -> String {
    let mut content = match message.role {
        MessageRole::Tool => tool_content_for_prompt(&message.content),
        MessageRole::Assistant if !message.parts.is_empty() => {
            assistant_parts_content_for_prompt(message)
        }
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

fn assistant_parts_content_for_prompt(message: &Message) -> String {
    let result_ids = message
        .parts
        .iter()
        .filter(|part| part.part_type == "tool_result")
        .filter_map(|part| part.tool_id().map(|id| id.to_string()))
        .collect::<std::collections::HashSet<_>>();

    let mut sections = Vec::new();
    for part in &message.parts {
        match part.part_type.as_str() {
            "text" => {
                if let Some(text) = part.text_value().filter(|text| !text.trim().is_empty()) {
                    sections.push(text.to_string());
                }
            }
            "reasoning" => {}
            "tool_call" => {
                let Some(id) = part.tool_id() else {
                    continue;
                };
                if result_ids.contains(id) {
                    continue;
                }
                if let Ok(content) = serde_json::to_string(&part.data) {
                    sections.push(tool_content_for_prompt(&content));
                }
            }
            "tool_result" => {
                if let Ok(content) = serde_json::to_string(&part.data) {
                    sections.push(tool_content_for_prompt(&content));
                }
            }
            _ => {}
        }
    }

    if sections.is_empty() {
        message.content.clone()
    } else {
        sections.join("\n\n")
    }
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

    if let Some(args) = obj.get("args") {
        out.push_str("\n\nTool call arguments:\n```json\n");
        let args = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
        out.push_str(&truncate_chars(&args, TOOL_OUTPUT_MAX_CHARS));
        out.push_str("\n```");
    }

    if let Some(preview) = obj
        .get("output_preview")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        out.push_str("\n\nTool output:\n");
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
    fn adaptive_selection_reduces_tail_when_prefix_is_only_prior_summary() {
        let summary = Message::user(format!("{}\nold summary", SUMMARY_PREFIX));
        let messages = vec![
            summary,
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            Message::assistant("a2"),
        ];

        let selected = select_messages_for_compaction(&messages, 2).expect("selection");

        assert_eq!(selected.messages_to_summarize.len(), 3);
        assert_eq!(selected.messages_to_summarize[1].content, "u1");
        assert_eq!(selected.tail_messages.len(), 2);
        assert_eq!(selected.tail_messages[0].content, "u2");
    }

    #[test]
    fn adaptive_selection_ignores_display_only_history() {
        let summary = Message::user(format!("{}\nold summary", SUMMARY_PREFIX));
        let marker = compaction_marker(CompactionStats {
            before_tokens: 100,
            after_tokens: 10,
            before_messages: 3,
            after_messages: 1,
        });

        assert!(select_messages_for_compaction(&[summary, marker], 2).is_none());
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
    fn compaction_marker_is_appended_after_retained_tail() {
        let stats = CompactionStats {
            before_tokens: 12_000,
            after_tokens: 360,
            before_messages: 8,
            after_messages: 2,
        };

        let compacted = build_compacted_messages(
            "summary",
            vec![Message::user("tail")],
            None,
            None,
            None,
            Some(stats),
        );

        assert_eq!(compacted.len(), 3);
        assert!(is_compaction_summary(&compacted[0]));
        assert_eq!(compacted[1].content, "tail");
        assert!(is_compaction_marker(&compacted[2]));
        assert_eq!(compacted[2].compaction_stats, Some(stats));
        assert_eq!(message_context_tokens(&compacted[2]), 0);
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

    #[test]
    fn compaction_stats_formats_growth() {
        let stats = CompactionStats {
            before_tokens: 2_472,
            after_tokens: 3_060,
            before_messages: 6,
            after_messages: 5,
        };

        assert_eq!(stats.grew_tokens(), 588);
        assert_eq!(stats.growth_percent(), 24);
        assert_eq!(format_compaction_stats(stats), "2.5K -> 3.1K, grew 24%");
    }

    #[test]
    fn assistant_tool_parts_count_as_context_tokens() {
        let mut message = Message::assistant("small text");
        message.token_count = Some(1);
        message.add_tool_call_part(
            "call_1",
            "read",
            serde_json::json!({ "file_path": "src/lib.rs" }),
        );
        message.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_1",
            "name": "read",
            "status": "ok",
            "args": { "file_path": "src/lib.rs" },
            "output_preview": "x".repeat(400),
        }));

        assert!(message_context_tokens(&message) >= 100);
    }

    #[test]
    fn compaction_prompt_preserves_tool_call_arguments() {
        let tool = Message::tool(
            serde_json::json!({
                "name": "edit",
                "status": "ok",
                "args": {
                    "file_path": "src/lib.rs",
                    "old_string": "before",
                    "new_string": "after"
                },
                "output_preview": "Replaced at line 4"
            })
            .to_string(),
        );

        let prompt = build_prompt(&[tool]);

        assert!(prompt.contains("Tool call arguments:"));
        assert!(prompt.contains("\"old_string\": \"before\""));
        assert!(prompt.contains("\"new_string\": \"after\""));
        assert!(prompt.contains("Tool output:\nReplaced at line 4"));
    }
}
