//! Opt-in, content-free JSONL telemetry for the primary print-mode turn.
//!
//! SDK metadata is untrusted text: only known event types, numeric fields and
//! validated labels cross this boundary. Never pass raw metadata or conversation
//! content to `record`; its callers supply already content-free event objects.

use crate::aisdk::chunk::TokenUsage;
use crate::llm::ChunkMessage;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

/// Project SDK metadata onto a small, content-free host event vocabulary.
pub(crate) fn metadata_event(message: &str) -> Option<Value> {
    let mut words = message.splitn(2, char::is_whitespace);
    let kind = words.next()?;
    let fields = metadata_fields(words.next().unwrap_or_default());

    if let Some(reason) = kind.strip_prefix("provider_finish_reason=") {
        let reason = match reason {
            "stop" | "tool_calls" | "length" | "content_filter" | "refusal" | "end_turn"
            | "stop_sequence" | "pause_turn" | "unknown" => reason,
            // FinishReason::Unknown can contain arbitrary provider text.
            _ => "unknown",
        };
        return Some(json!({ "event": "provider_end", "reason": reason }));
    }

    match kind {
        "provider_step_start" => {
            let mut event = json!({
                "event": "provider_step_start",
                "step": number(&fields, "step")?,
                "messages": number(&fields, "messages")?,
                "tools": number(&fields, "tools")?,
            });
            // provider_step_log_summary in aisdk/response.rs. Roles and tool
            // names are deliberately omitted; preserve only counts and sizes.
            for key in [
                "message_text_bytes",
                "images",
                "tool_schema_bytes",
                "tool_description_bytes",
            ] {
                if let Some(value) = number(&fields, key) {
                    event[key] = value.into();
                }
            }
            for (group, keys) in [
                (
                    "message_roles",
                    &[
                        ("system", "system_messages"),
                        ("user", "user_messages"),
                        ("assistant", "assistant_messages"),
                    ][..],
                ),
                ("max_message", &[("bytes", "max_message_bytes")][..]),
                (
                    "last_message",
                    &[
                        ("bytes", "last_message_bytes"),
                        ("images", "last_message_images"),
                    ][..],
                ),
            ] {
                if let Some(value) = fields.get(group) {
                    for (source, target) in keys {
                        let value = value.split(',').find_map(|field| {
                            let (key, value) = field.split_once('=')?;
                            (key == *source).then(|| unsigned(value)).flatten()
                        });
                        if let Some(value) = value {
                            event[*target] = value.into();
                        }
                    }
                }
            }
            Some(event)
        }
        "response.completed" => {
            Some(json!({ "event": "provider_end", "reason": "response_completed" }))
        }
        "provider_step_finish" => {
            let has_tool_call = match *fields.get("has_tool_call")? {
                "true" => true,
                "false" => false,
                _ => return None,
            };
            let action = match *fields.get("action")? {
                "continue" => "continue",
                "finish" => "finish",
                _ => return None,
            };
            Some(json!({
                "event": "step_finish",
                "step": number(&fields, "step")?,
                "has_tool_call": has_tool_call,
                "action": action,
            }))
        }
        "tool_results_added" => Some(json!({
            "event": "tool_batch_end", "count": number(&fields, "count")?,
        })),
        "tool_result_error" => {
            let name = *fields.get("tool")?;
            if name.is_empty()
                || !name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':')
                })
            {
                return None;
            }
            // SDK call IDs need not match the host bridge IDs. Do not expose
            // the error output or imply an unreliable ID correlation.
            Some(json!({ "event": "tool_error", "name": name }))
        }
        "empty_response" => {
            let reason = match *fields.get("reason")? {
                "no_visible_content" => "no_visible_content",
                "reasoning_only" => "reasoning_only",
                _ => return None,
            };
            Some(json!({ "event": "empty_response", "reason": reason }))
        }
        "doom_loop_detected" => Some(json!({
            "event": "doom_loop_recovery",
            "step": number(&fields, "step")?,
            "recoveries": number(&fields, "recoveries")?,
        })),
        "provider_step_retry" => Some(json!({
            "event": "provider_retry",
            "step": number(&fields, "step")?,
            "attempt": number(&fields, "attempt")?,
            "delay_ms": number(&fields, "delay_ms")?,
        })),
        _ => None,
    }
}

/// Read structured metadata fields, never scanning into a preview/error tail.
/// Bracketed summaries are single fields, even when their contents have spaces.
fn metadata_fields(mut input: &str) -> HashMap<&str, &str> {
    let mut fields = HashMap::new();
    loop {
        input = input.trim_start();
        let Some(end) = input.find(|c: char| c == '=' || c == '[' || c.is_whitespace()) else {
            break;
        };
        let key = &input[..end];
        let separator = input.as_bytes()[end];
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || !matches!(separator, b'=' | b'[')
            || matches!(key, "preview" | "error" | "raw_error")
        {
            break;
        }
        input = &input[end + 1..];
        let bracketed = separator == b'[' || input.starts_with('[');
        if separator == b'=' && bracketed {
            input = &input[1..];
        }
        let end = if bracketed {
            let Some(end) = input.find(']') else {
                break;
            };
            end
        } else {
            input.find(char::is_whitespace).unwrap_or(input.len())
        };
        let value = &input[..end];
        if value.starts_with(['"', '\'']) {
            break;
        }
        // Keep the first occurrence; later text cannot override a header.
        fields.entry(key).or_insert(value);
        input = &input[end + usize::from(bracketed)..];
        if !input.is_empty() && !input.starts_with(char::is_whitespace) {
            break;
        }
    }
    fields
}

fn unsigned(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn number(fields: &HashMap<&str, &str>, key: &str) -> Option<u64> {
    unsigned(fields.get(key)?)
}

pub(crate) struct PrintTrace {
    file: File,
    started_at: Instant,
    step: Option<u64>,
    first_response_seen: bool,
    provider_completed: bool,
    reasoning_bytes: u64,
    text_bytes: u64,
    first_reasoning_ms: Option<u64>,
    last_reasoning_ms: Option<u64>,
    tool_ids: HashSet<String>,
}

impl PrintTrace {
    pub(crate) fn create(path: Option<&Path>) -> io::Result<Option<Self>> {
        let Some(path) = path else {
            return Ok(None);
        };
        let started_at = Instant::now();
        let mut options = OpenOptions::new();
        // create_new atomically refuses files and symlinks, including a source
        // file accidentally supplied as the trace destination.
        options.write(true).truncate(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut trace = Self {
            file: options.open(path)?,
            started_at,
            step: None,
            first_response_seen: false,
            provider_completed: true,
            reasoning_bytes: 0,
            text_bytes: 0,
            first_reasoning_ms: None,
            last_reasoning_ms: None,
            tool_ids: HashSet::new(),
        };
        trace.record(json!({ "event": "init" }))?;
        Ok(Some(trace))
    }

    /// Record a caller-supplied, content-free object with the common envelope.
    pub(crate) fn record(&mut self, mut event: Value) -> io::Result<()> {
        let fields = event.as_object_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "trace event must be an object")
        })?;
        if !fields
            .get("event")
            .and_then(Value::as_str)
            .is_some_and(|event| !event.is_empty())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "trace event must have an event label",
            ));
        }
        fields.insert("schema_version".into(), json!(1));
        let elapsed_ms = u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        fields.insert("elapsed_ms".into(), elapsed_ms.into());
        let mut line = serde_json::to_vec(&event).map_err(io::Error::other)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()
    }

    pub(crate) fn observe(&mut self, chunk: &ChunkMessage) -> io::Result<()> {
        match chunk {
            ChunkMessage::Diagnostic(event) => {
                match event.get("event").and_then(Value::as_str) {
                    Some("response_activity") => {
                        // Tool argument deltas can precede ToolCalls by a whole
                        // provider step. Record only their first-response time.
                        return if event.get("kind").and_then(Value::as_str) == Some("tool_call") {
                            self.first_response("tool_call")
                        } else {
                            Ok(())
                        };
                    }
                    Some("provider_step_start") => {
                        self.step = event.get("step").and_then(Value::as_u64);
                        self.first_response_seen = false;
                        self.provider_completed = false;
                        self.reasoning_bytes = 0;
                        self.text_bytes = 0;
                        self.first_reasoning_ms = None;
                        self.last_reasoning_ms = None;
                    }
                    Some("provider_end" | "step_finish") => self.finish_response()?,
                    _ => {}
                }
                self.record(event.clone())
            }
            ChunkMessage::Text(text) if !text.is_empty() => {
                if !self.provider_completed {
                    self.text_bytes = self.text_bytes.saturating_add(text.len() as u64);
                }
                self.first_response("text")
            }
            ChunkMessage::Reasoning(reasoning) if !reasoning.is_empty() => {
                if !self.provider_completed {
                    self.reasoning_bytes =
                        self.reasoning_bytes.saturating_add(reasoning.len() as u64);
                    let now = self.started_at.elapsed().as_millis() as u64;
                    self.first_reasoning_ms.get_or_insert(now);
                    self.last_reasoning_ms = Some(now);
                }
                self.first_response("reasoning")
            }
            ChunkMessage::ToolCalls(calls) => {
                for call in calls {
                    if self.tool_ids.contains(&call.id) {
                        continue;
                    }
                    self.record(json!({
                        "event": "tool_start",
                        "id": call.id,
                        "name": call.function.name,
                        "arguments_bytes": call.function.arguments.len(),
                        "signature": tool_signature(&call.function.name, &call.function.arguments)?,
                    }))?;
                    self.tool_ids.insert(call.id.clone());
                }
                Ok(())
            }
            ChunkMessage::ToolResult(result) => self.record(json!({
                "event": "tool_end",
                "id": result.tool_call_id,
                "name": result.name,
                "output_bytes": result.content.len(),
            })),
            ChunkMessage::Usage(usage) => {
                let mut event = json!({ "event": "usage" });
                usage_fields(&mut event, usage);
                self.record(event)
            }
            ChunkMessage::Metrics {
                duration_ms,
                usage,
                cost,
                ..
            } => {
                // This is a reported summary, not another usage delta. Never
                // include token_count: the bridge may have estimated it.
                let mut event = json!({ "event": "metrics", "duration_ms": duration_ms });
                if let Some(usage) = usage {
                    usage_fields(&mut event, usage);
                }
                if let Some(cost) = cost.filter(|cost| cost.is_finite()) {
                    event["cost_usd"] = json!(cost);
                }
                self.record(event)
            }
            ChunkMessage::Retry(status) => self.record(json!({
                "event": "retry", "attempt": status.attempt, "delay_ms": status.delay_ms,
            })),
            ChunkMessage::StreamRollback { text, reasoning } => self.record(json!({
                "event": "rollback",
                "text_bytes": text.len(),
                "reasoning_bytes": reasoning.len(),
            })),
            ChunkMessage::TurnStopReason(reason) => self.record(json!({
                "event": "stop_reason", "reason": format!("{reason:?}"),
            })),
            ChunkMessage::Failed(_) | ChunkMessage::End | ChunkMessage::Cancelled => {
                self.finish_response()?;
                let event = match chunk {
                    ChunkMessage::Failed(_) => "failed",
                    ChunkMessage::End => "end",
                    _ => "cancelled",
                };
                self.record(json!({ "event": event }))
            }
            // In particular, do not recurse into SubagentChunk or record
            // SubagentStarted's title/description/prompt. Their timing and
            // usage must not be mixed into the primary provider step.
            _ => Ok(()),
        }
    }

    fn first_response(&mut self, kind: &str) -> io::Result<()> {
        if self.provider_completed || self.first_response_seen {
            return Ok(());
        }
        let Some(step) = self.step else {
            return Ok(());
        };
        self.record(json!({ "event": "first_response", "kind": kind, "step": step }))?;
        self.first_response_seen = true;
        Ok(())
    }

    fn finish_response(&mut self) -> io::Result<()> {
        if self.provider_completed {
            return Ok(());
        }
        self.record(json!({
            "event": "response_summary",
            "step": self.step,
            "reasoning_bytes": self.reasoning_bytes,
            "text_bytes": self.text_bytes,
            // Observed streaming span only, not billed tokens or hidden thinking time.
            "reasoning_span_ms": self.first_reasoning_ms.zip(self.last_reasoning_ms)
                .map(|(first, last)| last.saturating_sub(first)),
        }))?;
        self.provider_completed = true;
        Ok(())
    }
}

fn usage_fields(event: &mut Value, usage: &TokenUsage) {
    event["input_tokens"] = usage.input.into();
    event["output_tokens"] = usage.output.into();
    event["cache_read_tokens"] = usage.cache_read.into();
    event["cache_write_tokens"] = usage.cache_write.into();
}

fn tool_signature(name: &str, arguments: &str) -> io::Result<String> {
    // Invalid/partial arguments still have a stable hash, without exposing the
    // parse error or falling back to writing their raw content into the trace.
    let arguments =
        serde_json::from_str(arguments).unwrap_or_else(|_| Value::String(arguments.to_owned()));
    let canonical = serde_json::to_vec(&canonical_value(arguments)).map_err(io::Error::other)?;
    let mut hash = Sha256::new();
    hash.update(name.as_bytes());
    hash.update([0]); // Delimit the name from the canonical JSON arguments.
    hash.update(canonical);
    Ok(format!("{:x}", hash.finalize()))
}

fn canonical_value(value: Value) -> Value {
    match value {
        // serde_json's preserve_order feature is enabled. Sort recursively,
        // rather than relying on Map's iteration order to be canonical.
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .map(|(key, value)| (key, canonical_value(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_value).collect()),
        value => value,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn response_summaries_count_observed_bytes_once_per_step_without_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        step(&mut trace, 1);
        trace
            .observe(&ChunkMessage::Reasoning("private café".into()))
            .unwrap();
        trace.observe(&ChunkMessage::Text("answer".into())).unwrap();
        for _ in 0..2 {
            trace
                .observe(&ChunkMessage::Diagnostic(json!({
                    "event": "provider_end", "reason": "stop",
                })))
                .unwrap();
        }
        step(&mut trace, 2);
        trace.observe(&ChunkMessage::Text("ok".into())).unwrap();
        trace.observe(&ChunkMessage::End).unwrap();
        let rows = events(&path);
        let summaries = rows
            .iter()
            .filter(|row| row["event"] == "response_summary")
            .collect::<Vec<_>>();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0]["reasoning_bytes"], "private café".len());
        assert_eq!(summaries[0]["text_bytes"], 6);
        assert_eq!(summaries[1]["reasoning_bytes"], 0);
        assert_eq!(summaries[1]["text_bytes"], 2);
        assert!(!fs::read_to_string(path).unwrap().contains("private"));
    }

    use super::*;
    use crate::llm::{FunctionCall, ToolCall, ToolCallResult, TurnStopReason};
    use std::fs;
    use std::time::Duration;

    fn events(path: &Path) -> Vec<Value> {
        let text = fs::read_to_string(path).unwrap();
        assert!(text.ends_with('\n'));
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn trace(path: &Path) -> PrintTrace {
        PrintTrace::create(Some(path)).unwrap().unwrap()
    }

    fn step(trace: &mut PrintTrace, step: u64) {
        trace
            .observe(&ChunkMessage::Diagnostic(json!({
                "event": "provider_step_start", "step": step, "messages": 2, "tools": 1,
            })))
            .unwrap();
    }

    fn tool(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }

    #[test]
    fn metadata_projects_numeric_request_sizes_only() {
        let event = metadata_event(
            "provider_step_start step=2 messages=4 tools=1 message_roles[system=1,user=2,assistant=1] message_text_bytes=123 images=3 max_message[role=PRIVATE_ROLE,bytes=80] last_message[role=user,bytes=10,images=2] tool_names=[PRIVATE_TOOL,another tool] tool_schema_bytes=40 tool_description_bytes=20 prompt=PRIVATE_PROMPT",
        )
        .unwrap();
        assert_eq!(
            event,
            json!({
                "event": "provider_step_start", "step": 2, "messages": 4, "tools": 1,
                "system_messages": 1, "user_messages": 2, "assistant_messages": 1,
                "message_text_bytes": 123, "images": 3, "max_message_bytes": 80,
                "last_message_bytes": 10, "last_message_images": 2,
                "tool_schema_bytes": 40, "tool_description_bytes": 20,
            })
        );
        assert!(!event.to_string().contains("PRIVATE"));
    }

    #[test]
    fn metadata_omits_previews_errors_and_unknown_labels() {
        for action in ["continue", "finish"] {
            let event = metadata_event(&format!(
                "provider_step_finish step=2 has_tool_call=false end_turn=None provider_finish_reason=unknown last_phase=none assistant_text_chars=10 action={action} preview=\"PRIVATE_RESPONSE step=900 messages=999\""
            ))
            .unwrap();
            assert_eq!(
                event,
                json!({ "event": "step_finish", "step": 2, "has_tool_call": false, "action": action })
            );
        }
        assert_eq!(
            metadata_event("provider_step_retry step=3 attempt=2 delay_ms=40 next_epoch_ms=99 error=PRIVATE_ERROR raw_error=PRIVATE_BODY status=500"),
            Some(json!({ "event": "provider_retry", "step": 3, "attempt": 2, "delay_ms": 40 }))
        );
        assert_eq!(
            metadata_event("provider_finish_reason=PRIVATE_RESPONSE"),
            Some(json!({ "event": "provider_end", "reason": "unknown" }))
        );
        for reason in [
            "stop",
            "tool_calls",
            "length",
            "content_filter",
            "refusal",
            "end_turn",
            "stop_sequence",
            "pause_turn",
            "unknown",
        ] {
            assert_eq!(
                metadata_event(&format!("provider_finish_reason={reason}")),
                Some(json!({ "event": "provider_end", "reason": reason }))
            );
        }
        assert_eq!(
            metadata_event(
                "response.completed end_turn=Some(false) reasoning_items=2 PRIVATE_REASONING"
            ),
            Some(json!({ "event": "provider_end", "reason": "response_completed" }))
        );
    }

    #[test]
    fn metadata_maps_tool_and_recovery_events_without_payloads() {
        for (message, expected) in [
            (
                "tool_results_added count=2 names=PRIVATE_TOOL output_bytes=99 error_results=1 max_output[tool=PRIVATE_TOOL,bytes=90] next_messages=8",
                json!({ "event": "tool_batch_end", "count": 2 }),
            ),
            (
                "tool_result_error tool=functions.bash call_id=PRIVATE_ID output_chars=200 error=PRIVATE_OUTPUT",
                json!({ "event": "tool_error", "name": "functions.bash" }),
            ),
            (
                "empty_response reason=reasoning_only had_reasoning=true attempt=2",
                json!({ "event": "empty_response", "reason": "reasoning_only" }),
            ),
            (
                "empty_response reason=no_visible_content had_reasoning=false attempt=2",
                json!({ "event": "empty_response", "reason": "no_visible_content" }),
            ),
            (
                "doom_loop_detected step=3 recoveries=2 empty_reason=PRIVATE_REASON",
                json!({ "event": "doom_loop_recovery", "step": 3, "recoveries": 2 }),
            ),
        ] {
            assert_eq!(metadata_event(message), Some(expected));
        }
    }

    #[test]
    fn metadata_rejects_malformed_fields_and_never_reads_payload_tails() {
        for message in [
            "",
            "arbitrary PRIVATE_METADATA step=2",
            "reasoning_item id=PRIVATE_ID encrypted_bytes=10",
            "provider_step_start step=-2 messages=1 tools=1",
            "provider_step_start step=+2 messages=1 tools=1",
            "provider_step_start step=18446744073709551616 messages=1 tools=1",
            "provider_step_start step=2 messages=1",
            "provider_step_finish step=2 has_tool_call=false action=PRIVATE_ACTION",
            "provider_step_finish step=2 has_tool_call=PRIVATE_BOOL action=finish",
            "provider_step_finish step=2 has_tool_call=false preview=\"action=finish\"",
            "provider_step_retry step=2 error=PRIVATE_ERROR attempt=9 delay_ms=900",
            "empty_response reason=PRIVATE_REASON",
            "tool_result_error tool=\"PRIVATE_TOOL CONTENT\" call_id=123",
            "tool_results_added count=PRIVATE_COUNT",
        ] {
            assert!(metadata_event(message).is_none(), "accepted {message}");
        }
        assert_eq!(
            metadata_event(
                "provider_step_start step=1 messages=2 tools=0 preview=\"message_text_bytes=1234\""
            ),
            Some(json!({ "event": "provider_step_start", "step": 1, "messages": 2, "tools": 0 }))
        );
    }

    #[test]
    fn create_is_opt_in_and_flushes_init_without_a_model() {
        assert!(PrintTrace::create(None).unwrap().is_none());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let _trace = trace(&path);
        let recorded = events(&path);
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["event"], "init");
        assert_eq!(recorded[0]["schema_version"], 1);
        assert!(recorded[0]["elapsed_ms"].as_u64().is_some());
        assert!(recorded[0].get("model").is_none());
        assert!(recorded[0].get("provider").is_none());
    }

    #[test]
    fn record_adds_and_overwrites_envelope_and_flushes_each_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        trace.started_at = Instant::now() - Duration::from_millis(50);
        trace
            .record(json!({
                "event": "model", "provider": "test", "model": "test-model",
                "schema_version": 999, "elapsed_ms": "not-a-clock",
            }))
            .unwrap();
        let recorded = events(&path);
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[1]["event"], "model");
        assert_eq!(recorded[1]["schema_version"], 1);
        assert!(recorded[1]["elapsed_ms"].as_u64().unwrap() >= 50);
        for invalid in [Value::Null, json!([]), json!({}), json!({ "event": 1 })] {
            assert_eq!(
                trace.record(invalid).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(events(&path).len(), 2);
    }

    #[test]
    fn create_refuses_existing_trace_and_source_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.rs");
        fs::write(&source, "PRIVATE_SOURCE").unwrap();
        assert_eq!(
            PrintTrace::create(Some(&source)).err().unwrap().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "PRIVATE_SOURCE");
        let path = dir.path().join("trace.jsonl");
        let _trace = trace(&path);
        let before = fs::read(&path).unwrap();
        assert_eq!(
            PrintTrace::create(Some(&path)).err().unwrap().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn create_is_private_and_refuses_existing_and_dangling_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let _trace = trace(&path);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for (target, link) in [
            (path.clone(), dir.path().join("trace-link")),
            (dir.path().join("missing"), dir.path().join("dangling-link")),
        ] {
            symlink(&target, &link).unwrap();
            assert_eq!(
                PrintTrace::create(Some(&link)).err().unwrap().kind(),
                io::ErrorKind::AlreadyExists
            );
        }
        assert!(!dir.path().join("missing").exists());
    }

    #[test]
    fn activity_records_only_one_first_response_per_provider_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        let activity = ChunkMessage::Diagnostic(json!({
            "event": "response_activity", "kind": "tool_call",
        }));
        trace.observe(&activity).unwrap();
        trace
            .observe(&ChunkMessage::Text("before step".into()))
            .unwrap();
        step(&mut trace, 1);
        trace.observe(&ChunkMessage::Text(String::new())).unwrap();
        trace.observe(&activity).unwrap();
        trace.observe(&activity).unwrap();
        trace
            .observe(&ChunkMessage::Reasoning("PRIVATE_REASONING".into()))
            .unwrap();
        trace
            .observe(&ChunkMessage::Text("PRIVATE_RESPONSE".into()))
            .unwrap();
        step(&mut trace, 2);
        trace
            .observe(&ChunkMessage::Reasoning("PRIVATE_REASONING".into()))
            .unwrap();
        trace.observe(&activity).unwrap();
        step(&mut trace, 3);
        trace
            .observe(&ChunkMessage::Text("PRIVATE_RESPONSE".into()))
            .unwrap();
        let recorded = events(&path);
        let first: Vec<_> = recorded
            .iter()
            .filter(|event| event["event"] == "first_response")
            .collect();
        assert_eq!(first.len(), 3);
        for (event, (step, kind)) in
            first
                .iter()
                .zip([(1, "tool_call"), (2, "reasoning"), (3, "text")])
        {
            assert_eq!(event["step"], step);
            assert_eq!(event["kind"], kind);
        }
        assert!(!recorded
            .iter()
            .any(|event| event["event"] == "response_activity"));
        assert!(!fs::read_to_string(&path).unwrap().contains("PRIVATE"));
    }

    #[test]
    fn completed_steps_do_not_time_tool_output_as_a_response() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        for (index, completion) in [
            ChunkMessage::Diagnostic(json!({ "event": "provider_end", "reason": "tool_calls" })),
            ChunkMessage::Diagnostic(
                json!({ "event": "step_finish", "step": 2, "action": "finish" }),
            ),
            ChunkMessage::Failed("PRIVATE_ERROR".into()),
            ChunkMessage::End,
            ChunkMessage::Cancelled,
        ]
        .into_iter()
        .enumerate()
        {
            step(&mut trace, index as u64 + 1);
            trace.observe(&completion).unwrap();
            trace
                .observe(&ChunkMessage::Text("PRIVATE_TOOL_OUTPUT".into()))
                .unwrap();
            trace
                .observe(&ChunkMessage::Reasoning("PRIVATE_REASONING".into()))
                .unwrap();
            trace
                .observe(&ChunkMessage::Diagnostic(
                    json!({ "event": "response_activity", "kind": "tool_call" }),
                ))
                .unwrap();
        }
        step(&mut trace, 6);
        trace
            .observe(&ChunkMessage::Text("new step".into()))
            .unwrap();
        let recorded = events(&path);
        let first: Vec<_> = recorded
            .iter()
            .filter(|event| event["event"] == "first_response")
            .collect();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0]["step"], 6);
    }

    #[test]
    fn tools_deduplicate_ids_and_hash_canonical_arguments_without_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        let args = r#"{"z":"PRIVATE_ARGUMENT","a":{"d":2,"c":1},"list":[{"y":2,"x":1}]}"#;
        let ordered = r#"{ "list": [{"x":1,"y":2}], "a": {"c":1,"d":2}, "z":"PRIVATE_ARGUMENT" }"#;
        let call = tool("call-1", "read", args);
        trace
            .observe(&ChunkMessage::ToolCalls(vec![
                call.clone(),
                call.clone(),
                tool("call-2", "read", ordered),
            ]))
            .unwrap();
        trace.observe(&ChunkMessage::ToolCalls(vec![call])).unwrap();
        step(&mut trace, 2);
        trace
            .observe(&ChunkMessage::ToolCalls(vec![tool("call-1", "read", args)]))
            .unwrap();
        let output = "PRIVATE_TOOL_RESULT 🦀";
        trace
            .observe(&ChunkMessage::ToolResult(ToolCallResult {
                tool_call_id: "call-1".into(),
                role: "tool".into(),
                name: "read".into(),
                content: output.into(),
            }))
            .unwrap();
        let recorded = events(&path);
        let starts: Vec<_> = recorded
            .iter()
            .filter(|event| event["event"] == "tool_start")
            .collect();
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[0]["arguments_bytes"], args.len());
        assert_eq!(starts[0]["signature"], starts[1]["signature"]);
        assert_eq!(starts[0]["signature"].as_str().unwrap().len(), 64);
        let end = recorded
            .iter()
            .find(|event| event["event"] == "tool_end")
            .unwrap();
        assert_eq!(end["output_bytes"], output.len());
        assert_eq!(end["id"], "call-1");
        assert!(!fs::read_to_string(&path).unwrap().contains("PRIVATE"));
        assert_ne!(
            tool_signature("read", args).unwrap(),
            tool_signature("write", args).unwrap()
        );
        assert_ne!(
            tool_signature("read", "[1,2]").unwrap(),
            tool_signature("read", "[2,1]").unwrap()
        );
        let invalid = tool_signature("read", "PRIVATE_INVALID_JSON").unwrap();
        assert_eq!(invalid.len(), 64);
        assert!(!invalid.contains("PRIVATE"));
    }

    #[test]
    fn metrics_are_a_reported_summary_not_an_extra_usage_delta() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        let usage = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_write: 40,
        };
        trace.observe(&ChunkMessage::Usage(usage)).unwrap();
        trace.observe(&ChunkMessage::Usage(usage)).unwrap();
        trace
            .observe(&ChunkMessage::Metrics {
                token_count: 999_999,
                duration_ms: 80,
                usage: Some(usage.saturating_add(usage)),
                cost: Some(0.25),
            })
            .unwrap();
        let recorded = events(&path);
        let deltas: Vec<_> = recorded
            .iter()
            .filter(|event| event["event"] == "usage")
            .collect();
        assert_eq!(deltas.len(), 2);
        assert_eq!(
            deltas
                .iter()
                .map(|event| event["input_tokens"].as_u64().unwrap())
                .sum::<u64>(),
            20
        );
        let metrics = recorded
            .iter()
            .find(|event| event["event"] == "metrics")
            .unwrap();
        assert_eq!(metrics["input_tokens"], 20);
        assert_eq!(metrics["output_tokens"], 40);
        assert_eq!(metrics["cache_read_tokens"], 60);
        assert_eq!(metrics["cache_write_tokens"], 80);
        assert_eq!(metrics["duration_ms"], 80);
        assert_eq!(metrics["cost_usd"], 0.25);
        assert!(recorded
            .iter()
            .all(|event| event.get("token_count").is_none()));
    }

    #[test]
    fn metrics_without_reported_usage_omit_estimates_and_nonfinite_cost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        for cost in [None, Some(f64::NAN), Some(f64::INFINITY)] {
            trace
                .observe(&ChunkMessage::Metrics {
                    token_count: 900,
                    duration_ms: 3,
                    usage: None,
                    cost,
                })
                .unwrap();
        }
        for event in events(&path).iter().skip(1) {
            assert_eq!(event.as_object().unwrap().len(), 4);
            assert_eq!(event["event"], "metrics");
            assert!(event.get("input_tokens").is_none());
            assert!(event.get("token_count").is_none());
            assert!(event.get("cost_usd").is_none());
        }
    }

    #[test]
    fn lifecycle_and_rollback_do_not_include_error_or_rolled_back_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        for chunk in [
            ChunkMessage::Retry(crate::aisdk::retry::RetryStatus {
                attempt: 2,
                message: "PRIVATE_RETRY_ERROR".into(),
                delay_ms: 10,
                next_epoch_ms: 999,
            }),
            ChunkMessage::StreamRollback {
                text: "PRIVATE_TEXT 🦀".into(),
                reasoning: "PRIVATE_REASONING".into(),
            },
            ChunkMessage::TurnStopReason(TurnStopReason::MaxTokens),
            ChunkMessage::Failed("PRIVATE_ERROR".into()),
            ChunkMessage::End,
            ChunkMessage::Cancelled,
            ChunkMessage::Warning("PRIVATE_WARNING".into()),
        ] {
            trace.observe(&chunk).unwrap();
        }
        let recorded = events(&path);
        let labels: Vec<_> = recorded
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            [
                "init",
                "retry",
                "rollback",
                "stop_reason",
                "failed",
                "end",
                "cancelled"
            ]
        );
        assert_eq!(recorded[1]["attempt"], 2);
        assert_eq!(recorded[1]["delay_ms"], 10);
        assert_eq!(recorded[2]["text_bytes"], "PRIVATE_TEXT 🦀".len());
        assert_eq!(recorded[2]["reasoning_bytes"], "PRIVATE_REASONING".len());
        assert_eq!(recorded[3]["reason"], "MaxTokens");
        assert!(!fs::read_to_string(&path).unwrap().contains("PRIVATE"));
    }

    #[test]
    fn subagent_events_never_change_primary_timing_or_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let mut trace = trace(&path);
        step(&mut trace, 1);
        trace
            .observe(&ChunkMessage::SubagentStarted {
                parent_session_id: "parent".into(),
                session_id: "child".into(),
                title: "PRIVATE_TITLE".into(),
                subagent_type: "general".into(),
                model: None,
                provider: None,
                description: "PRIVATE_DESCRIPTION".into(),
                prompt: "PRIVATE_PROMPT".into(),
            })
            .unwrap();
        for chunk in [
            ChunkMessage::Diagnostic(json!({ "event": "provider_step_start", "step": 99 })),
            ChunkMessage::Text("PRIVATE_SUBAGENT_TEXT".into()),
            ChunkMessage::Usage(TokenUsage {
                input: 999,
                ..TokenUsage::default()
            }),
            ChunkMessage::ToolCalls(vec![tool("child-tool", "read", "PRIVATE_ARGUMENT")]),
            ChunkMessage::End,
        ] {
            trace
                .observe(&ChunkMessage::SubagentChunk {
                    session_id: "child".into(),
                    chunk: Box::new(chunk),
                })
                .unwrap();
        }
        trace
            .observe(&ChunkMessage::Text("PRIVATE_PRIMARY_TEXT".into()))
            .unwrap();
        let recorded = events(&path);
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[2]["event"], "first_response");
        assert_eq!(recorded[2]["step"], 1);
        assert!(!fs::read_to_string(&path).unwrap().contains("PRIVATE"));
    }
}
