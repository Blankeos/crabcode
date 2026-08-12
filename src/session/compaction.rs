use crate::session::types::{CompactionStats, Message, MessageRole};

/// Max recent user turns considered for the preserved tail.
/// Actual tail is also capped by [`DEFAULT_PRESERVE_RECENT_TOKENS`].
pub const DEFAULT_TAIL_TURNS: usize = 2;

/// Token budget for recent messages kept verbatim after compaction.
/// OpenCode: max(12k, min(40k, 25% usable)). We use the 12k floor so
/// tool-heavy recent turns cannot swallow the entire context.
pub const DEFAULT_PRESERVE_RECENT_TOKENS: usize = 12_000;

/// Minimum tokens in the head (to summarize) before compaction is worth running.
/// Grok default is 5k; 2k still skips tiny heads that a fat summary would inflate.
pub const MIN_COMPACTABLE_TOKENS: usize = 2_000;
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
    /// Absolute insert index in the full transcript: keep `messages[..summarize_end]`,
    /// then append summary + marker, then keep `messages[summarize_end..]`.
    pub summarize_end: usize,
    pub messages_to_summarize: Vec<Message>,
    pub tail_messages: Vec<Message>,
}

/// First index of the active model context after the latest compaction.
///
/// Soft compaction keeps pre-boundary history for UI/DB and only excludes it from
/// the model request. Active context starts at the latest compaction summary
/// associated with the last marker (canonical layout:
/// `[…history…][summary][…tail…][marker]`).
pub fn context_start_index(messages: &[Message]) -> usize {
    let Some(marker_idx) = messages.iter().rposition(is_compaction_marker) else {
        return 0;
    };

    // Prefer the summary paired with this marker. If none exists (legacy/partial
    // state), keep preceding messages — the marker itself is never model input.
    messages[..=marker_idx]
        .iter()
        .rposition(is_compaction_summary)
        .unwrap_or(0)
}

/// Messages the model should see: from the latest compaction summary onward,
/// with compaction markers removed.
pub fn filter_messages_for_context(messages: &[Message]) -> Vec<Message> {
    let start = context_start_index(messages);
    messages[start..]
        .iter()
        .filter(|message| !is_compaction_marker(message))
        .cloned()
        .collect()
}

/// Indices into `messages` that participate in the active context (no markers).
fn context_work_indices(messages: &[Message]) -> Vec<usize> {
    let start = context_start_index(messages);
    (start..messages.len())
        .filter(|&idx| !is_compaction_marker(&messages[idx]))
        .collect()
}

pub fn select_messages(messages: &[Message], tail_turns: usize) -> Option<CompactionSelection> {
    select_messages_with_budget(
        messages,
        tail_turns,
        DEFAULT_PRESERVE_RECENT_TOKENS,
        0, // unit tests use tiny fixtures; min enforced in for_compaction
    )
}

/// OpenCode/Grok-style head/tail split:
///
/// 1. Consider at most `max_tail_turns` newest user-turns as tail candidates.
/// 2. Walk newest-first; keep whole turns while `sum(tail) <= preserve_tokens`.
/// 3. If nothing can be kept under budget (or the keep would start at the first
///    message), summarize the entire active context (empty tail) — matches
///    OpenCode's "tail fallback" / short-session path.
/// 4. Refuse when the resulting head is below `min_compactable_tokens`.
fn select_messages_with_budget(
    messages: &[Message],
    max_tail_turns: usize,
    preserve_tokens: usize,
    min_compactable_tokens: usize,
) -> Option<CompactionSelection> {
    let work_indices = context_work_indices(messages);
    if work_indices.is_empty() {
        return None;
    }

    let user_work_positions: Vec<usize> = work_indices
        .iter()
        .enumerate()
        .filter_map(|(work_pos, &full_idx)| {
            matches!(messages[full_idx].role, MessageRole::User).then_some(work_pos)
        })
        .collect();

    if user_work_positions.is_empty() {
        return None;
    }

    // Per-turn token costs over work indices.
    let mut turn_tokens: Vec<usize> = Vec::with_capacity(user_work_positions.len());
    for (i, &start_work) in user_work_positions.iter().enumerate() {
        let end_work = user_work_positions
            .get(i + 1)
            .copied()
            .unwrap_or(work_indices.len());
        let tokens = work_indices[start_work..end_work]
            .iter()
            .map(|&full_idx| message_context_tokens(&messages[full_idx]))
            .sum();
        turn_tokens.push(tokens);
    }

    // Candidate recent turns = last max_tail_turns (OpenCode `all.slice(-limit)`).
    let candidate_start = user_work_positions.len().saturating_sub(max_tail_turns);

    let mut tail_start_work: Option<usize> = None;
    let mut kept_tokens = 0usize;

    if max_tail_turns > 0 {
        for i in (candidate_start..user_work_positions.len()).rev() {
            let size = turn_tokens[i];
            if kept_tokens.saturating_add(size) > preserve_tokens {
                break;
            }
            tail_start_work = Some(user_work_positions[i]);
            kept_tokens = kept_tokens.saturating_add(size);
        }
    }

    // OpenCode: if no keep, or keep would start at work index 0, summarize all.
    let tail_start_work = match tail_start_work {
        Some(0) | None => work_indices.len(),
        Some(start) => start,
    };

    let head_indices = &work_indices[..tail_start_work];
    let tail_indices = &work_indices[tail_start_work..];
    if head_indices.is_empty() {
        return None;
    }

    let head_tokens: usize = head_indices
        .iter()
        .map(|&full_idx| message_context_tokens(&messages[full_idx]))
        .sum();
    if head_tokens < min_compactable_tokens {
        crate::emit_log!(
            "[compaction] skip: head {} tokens < min_compactable {}",
            head_tokens,
            min_compactable_tokens
        );
        return None;
    }

    crate::emit_log!(
        "[compaction] select: head_msgs={} head_tok={} tail_msgs={} tail_tok={} preserve_budget={}",
        head_indices.len(),
        head_tokens,
        tail_indices.len(),
        kept_tokens,
        preserve_tokens
    );

    let summarize_end = tail_indices
        .first()
        .copied()
        .unwrap_or_else(|| head_indices.last().copied().unwrap_or(0) + 1)
        .min(messages.len());

    Some(CompactionSelection {
        summarize_end,
        messages_to_summarize: head_indices
            .iter()
            .map(|&idx| messages[idx].clone())
            .collect(),
        tail_messages: tail_indices
            .iter()
            .map(|&idx| messages[idx].clone())
            .collect(),
    })
}

pub fn select_messages_for_compaction(
    messages: &[Message],
    preferred_tail_turns: usize,
) -> Option<CompactionSelection> {
    select_messages_for_compaction_with_min(messages, preferred_tail_turns, MIN_COMPACTABLE_TOKENS)
}

/// Like [`select_messages_for_compaction`], but with an explicit min head size.
/// Manual `/compact` passes `0` so short chats still compact (OpenCode/Grok).
pub fn select_messages_for_compaction_with_min(
    messages: &[Message],
    preferred_tail_turns: usize,
    min_compactable_tokens: usize,
) -> Option<CompactionSelection> {
    for tail_turns in (0..=preferred_tail_turns).rev() {
        let Some(selection) = select_messages_with_budget(
            messages,
            tail_turns,
            DEFAULT_PRESERVE_RECENT_TOKENS,
            min_compactable_tokens,
        ) else {
            continue;
        };
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

fn build_summary_message(
    summary: &str,
    model: Option<String>,
    provider: Option<String>,
    agent_mode: Option<String>,
    timestamp: Option<std::time::SystemTime>,
) -> Message {
    let mut summary_message = Message::user(format!("{}\n{}", SUMMARY_PREFIX, summary.trim()));
    summary_message.model = model;
    summary_message.provider = provider;
    summary_message.agent_mode = agent_mode;
    summary_message.token_count = Some(estimate_tokens(&summary_message.content));
    if let Some(timestamp) = timestamp {
        summary_message.timestamp = timestamp;
    }
    summary_message
}

/// Hard-replace helper used by tests and legacy call sites: summary + tail (+ optional marker).
pub fn build_compacted_messages(
    summary: &str,
    tail_messages: Vec<Message>,
    model: Option<String>,
    provider: Option<String>,
    agent_mode: Option<String>,
    stats: Option<CompactionStats>,
) -> Vec<Message> {
    let timestamp = tail_messages.first().map(|message| {
        message
            .timestamp
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or(message.timestamp)
    });
    let summary_message = build_summary_message(summary, model, provider, agent_mode, timestamp);

    let mut messages = vec![summary_message];
    messages.extend(tail_messages);
    if let Some(stats) = stats {
        append_compaction_marker(&mut messages, stats);
    }
    messages
}

/// OpenCode-style soft compaction: keep pre-boundary history for UI/DB reading,
/// insert summary, keep the retained tail, then append the marker at the end.
pub fn apply_soft_compaction(
    messages: &[Message],
    selection: &CompactionSelection,
    summary: &str,
    model: Option<String>,
    provider: Option<String>,
    agent_mode: Option<String>,
    stats: CompactionStats,
) -> Vec<Message> {
    let summarize_end = selection.summarize_end.min(messages.len());
    let timestamp = selection
        .tail_messages
        .first()
        .map(|message| {
            message
                .timestamp
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap_or(message.timestamp)
        })
        .or_else(|| {
            messages
                .get(summarize_end.saturating_sub(1))
                .map(|message| message.timestamp)
        });

    // Layout: [pre-boundary history…][summary][retained tail…][marker]
    // Marker goes last so it is visible at the bottom of the chat without
    // forcing a mid-history scroll jump (summary itself stays UI-hidden).
    let mut result = Vec::with_capacity(messages.len() + 2);
    result.extend_from_slice(&messages[..summarize_end]);
    result.push(build_summary_message(
        summary, model, provider, agent_mode, timestamp,
    ));
    if summarize_end < messages.len() {
        result.extend_from_slice(&messages[summarize_end..]);
    }
    append_compaction_marker(&mut result, stats);
    result
}

/// Token count for the active model context (post-boundary), not full UI history.
pub fn total_context_tokens(messages: &[Message]) -> usize {
    filter_messages_for_context(messages)
        .iter()
        .map(message_context_tokens)
        .sum()
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
    content = crate::utils::sanitize::strip_legacy_image_descriptions(&content);

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
    fn select_messages_drops_heavy_tail_turn_over_preserve_budget() {
        // Last turn alone exceeds preserve budget → empty tail, whole session summarized.
        let mut heavy = Message::assistant("a-heavy");
        heavy.token_count = Some(DEFAULT_PRESERVE_RECENT_TOKENS + 1);
        let messages = vec![
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            heavy,
        ];

        let selected = select_messages_with_budget(&messages, 2, DEFAULT_PRESERVE_RECENT_TOKENS, 0)
            .expect("selection");

        assert!(selected.tail_messages.is_empty());
        assert_eq!(selected.messages_to_summarize.len(), 4);
    }

    #[test]
    fn select_messages_keeps_only_turns_under_preserve_budget() {
        // Each turn ~5k; preserve 12k → keep last 2 turns (10k), summarize first.
        let mut a1 = Message::assistant("a1");
        a1.token_count = Some(5_000);
        let mut a2 = Message::assistant("a2");
        a2.token_count = Some(5_000);
        let mut a3 = Message::assistant("a3");
        a3.token_count = Some(5_000);
        let messages = vec![
            Message::user("u1"),
            a1,
            Message::user("u2"),
            a2,
            Message::user("u3"),
            a3,
        ];

        let selected = select_messages_with_budget(&messages, 3, 12_000, 0).expect("selection");

        assert_eq!(selected.tail_messages.len(), 4); // u2,a2,u3,a3
        assert_eq!(selected.messages_to_summarize[0].content, "u1");
    }

    #[test]
    fn select_messages_with_budget_skips_tiny_head() {
        // Head is only ~100 tokens of fluff; min_compactable refuses that split.
        let mut heavy = Message::assistant("heavy-tail");
        heavy.token_count = Some(10_000);
        let messages = vec![
            Message::user("hi"),
            Message::assistant("yo"),
            Message::user("continue"),
            heavy,
        ];
        assert!(select_messages_with_budget(
            &messages,
            1,
            DEFAULT_PRESERVE_RECENT_TOKENS,
            MIN_COMPACTABLE_TOKENS,
        )
        .is_none());
    }

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
        // Pad so reduced-tail head clears MIN_COMPACTABLE_TOKENS.
        let mut summary = Message::user(format!("{}\nold summary", SUMMARY_PREFIX));
        summary.token_count = Some(1_500);
        let mut a1 = Message::assistant("a1");
        a1.token_count = Some(1_500);
        let messages = vec![
            summary,
            Message::user("u1"),
            a1,
            Message::user("u2"),
            Message::assistant("a2"),
        ];

        let selected = select_messages_for_compaction(&messages, 2).expect("selection");

        // Preferred tail=2 leaves only prior summary as head (not meaningful)
        // → adaptive reduces to tail=1: summarize summary+u1+a1, keep u2+a2.
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
    fn soft_compaction_keeps_pre_boundary_history() {
        let mut a1 = Message::assistant("a1");
        a1.token_count = Some(3_000); // clear MIN_COMPACTABLE_TOKENS for head
        let messages = vec![
            Message::user("u1"),
            a1,
            Message::user("u2"),
            Message::assistant("a2"),
        ];
        let selection = select_messages_for_compaction(&messages, 1).expect("selection");
        let stats = CompactionStats {
            before_tokens: 1_000,
            after_tokens: 100,
            before_messages: 4,
            after_messages: 3,
        };

        let soft = apply_soft_compaction(
            &messages,
            &selection,
            "handoff summary",
            None,
            None,
            None,
            stats,
        );

        // Pre-boundary history is retained for UI/DB reading.
        assert!(soft.iter().any(|m| m.content == "u1"));
        assert!(soft.iter().any(|m| m.content == "a1"));
        assert!(soft.iter().any(is_compaction_summary));
        assert!(soft.iter().any(is_compaction_marker));
        assert!(soft.iter().any(|m| m.content == "u2"));
        // Marker is last so "Context compacted" is visible at chat bottom.
        assert!(is_compaction_marker(soft.last().expect("non-empty")));

        let context = filter_messages_for_context(&soft);
        assert!(context.iter().any(is_compaction_summary));
        assert!(!context.iter().any(|m| m.content == "u1"));
        assert!(context.iter().any(|m| m.content == "u2"));
        assert_eq!(
            total_context_tokens(&soft),
            context.iter().map(message_context_tokens).sum::<usize>()
        );
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

    #[test]
    fn compaction_prompt_strips_legacy_image_description_blocks() {
        let message = Message::user(
            "[Image #1]\n\n<image_description source=\"vlm-agent\">\nPermission denied\n</image_description>\nkeep this",
        );

        let prompt = build_prompt(&[message]);

        assert!(prompt.contains("[Image #1]"));
        assert!(prompt.contains("keep this"));
        assert!(!prompt.contains("<image_description"));
        assert!(!prompt.contains("Permission denied"));
    }
}
