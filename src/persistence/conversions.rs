use crate::persistence::{
    Message, MessagePart as PersistenceMessagePart, Session as PersistenceSession,
};
use crate::session::types::{
    CompactionStats, Message as SessionMessage, MessagePart as SessionMessagePart, MessageRole,
    Session,
};

impl From<SessionMessage> for Message {
    fn from(msg: SessionMessage) -> Self {
        // Move the owned parts instead of cloning them: this conversion runs
        // for the whole transcript on every streaming snapshot.
        let usage = msg.recorded_usage();
        let is_compaction_summary = crate::session::compaction::is_compaction_summary(&msg);
        let mut parts: Vec<PersistenceMessagePart> = if msg.parts.is_empty() {
            let mut parts = Vec::new();
            if !msg.content.is_empty() {
                parts.push(PersistenceMessagePart {
                    part_type: "text".to_string(),
                    data: serde_json::json!({ "text": msg.content }),
                });
            }
            parts
        } else {
            msg.parts
                .into_iter()
                .map(|part| PersistenceMessagePart {
                    part_type: part.part_type,
                    data: part.data,
                })
                .collect()
        };

        if let Some(ref reasoning) = msg.reasoning {
            if !reasoning.is_empty() && !parts.iter().any(|part| part.part_type == "reasoning") {
                parts.push(PersistenceMessagePart {
                    part_type: "reasoning".to_string(),
                    data: serde_json::json!({ "text": reasoning }),
                });
            }
        }

        for path in &msg.local_image_paths {
            parts.push(PersistenceMessagePart {
                part_type: "local_image".to_string(),
                data: serde_json::json!({ "path": path }),
            });
        }

        if let Some(stats) = msg.compaction_stats {
            if let Ok(data) = serde_json::to_value(stats) {
                parts.push(PersistenceMessagePart {
                    part_type: "compaction_stats".to_string(),
                    data,
                });
            }
        }

        if msg.was_interrupted
            && !parts.iter().any(|part| {
                part.part_type == "status"
                    && part
                        .data
                        .get("state")
                        .and_then(|value| value.as_str())
                        .is_some_and(|state| state == "interrupted")
            })
        {
            parts.push(PersistenceMessagePart {
                part_type: "status".to_string(),
                data: serde_json::json!({ "state": "interrupted" }),
            });
        }

        // Compaction summaries store billed prompt/completion on a usage part
        // for stats/cost. `tokens_used` is the context estimate (summary text),
        // not billed buckets — otherwise reload inflates the model window.
        let tokens_used = if is_compaction_summary {
            msg.token_count
                .map(|count| count.min(i32::MAX as usize) as i32)
                .unwrap_or(0)
        } else {
            usage
                .map(|usage| usage.tokens().min(i32::MAX as u64) as i32)
                .or(msg.token_count.map(|c| c as i32))
                .unwrap_or(0)
        };

        Message {
            id: cuid2::create_id(),
            session_id: 0,
            role: match msg.role {
                MessageRole::User => "user".to_string(),
                MessageRole::Assistant => "assistant".to_string(),
                MessageRole::System => "system".to_string(),
                MessageRole::Tool => "tool".to_string(),
            },
            parts,
            timestamp: msg
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            tokens_used,
            model: msg.model.clone(),
            provider: msg.provider.clone(),
            agent_mode: msg.agent_mode.clone(),
            duration_ms: msg.duration_ms.map(|d| d as i64).unwrap_or(0),
            t0_ms: msg.t0_ms.map(|v| v as i64),
            t1_ms: msg.t1_ms.map(|v| v as i64),
            tn_ms: msg.tn_ms.map(|v| v as i64),
            output_tokens: msg.output_tokens.map(|v| v as i64),
            tokens_per_sec: msg.tokens_per_sec,
        }
    }
}

impl TryFrom<Message> for SessionMessage {
    type Error = anyhow::Error;

    fn try_from(msg: Message) -> Result<Self, Self::Error> {
        let session_parts: Vec<SessionMessagePart> = msg
            .parts
            .iter()
            .map(|part| SessionMessagePart {
                part_type: part.part_type.clone(),
                data: part.data.clone(),
            })
            .collect();

        let content = session_parts
            .iter()
            .filter_map(|p| {
                if p.part_type == "text" {
                    p.data.get("text").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let reasoning = session_parts
            .iter()
            .filter_map(|p| {
                if p.part_type == "reasoning" {
                    p.data.get("text").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");
        let reasoning = (!reasoning.is_empty()).then_some(reasoning);

        let local_image_paths = session_parts
            .iter()
            .filter_map(|p| {
                if p.part_type == "local_image" {
                    p.data.get("path").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .map(|path| path.to_string())
            .collect();

        let compaction_stats = session_parts
            .iter()
            .find(|p| p.part_type == "compaction_stats")
            .and_then(|p| serde_json::from_value::<CompactionStats>(p.data.clone()).ok());

        let was_interrupted = session_parts.iter().any(|p| {
            p.part_type == "status"
                && p.data
                    .get("state")
                    .and_then(|value| value.as_str())
                    .is_some_and(|state| state == "interrupted")
        });

        let role = match msg.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => return Err(anyhow::anyhow!("Unknown role: {}", msg.role)),
        };

        // Billed output buckets are exact; prefer them over the persisted
        // text estimate when backfilling rows stored before output_tokens
        // existed. Never derive output tokens from `tokens_used` (total).
        let billed_output: Option<usize> = {
            let mut total = 0u64;
            let mut found = false;
            for part in &session_parts {
                if part.part_type == "usage" {
                    if let Some(output) = part.data.get("output").and_then(|v| v.as_u64()) {
                        total = total.saturating_add(output);
                        found = true;
                    }
                }
            }
            if found && total > 0 && total <= usize::MAX as u64 {
                Some(total as usize)
            } else {
                None
            }
        };
        let persisted_output_tokens: Option<usize> =
            msg.output_tokens
                .and_then(|v| if v > 0 { Some(v as usize) } else { None });
        let output_tokens = persisted_output_tokens.or(billed_output);

        Ok(SessionMessage {
            role,
            content,
            reasoning,
            parts: session_parts,
            timestamp: std::time::UNIX_EPOCH + std::time::Duration::from_secs(msg.timestamp as u64),
            is_complete: true,
            agent_mode: msg.agent_mode.clone(),
            token_count: if msg.tokens_used > 0 {
                Some(msg.tokens_used as usize)
            } else {
                None
            },
            duration_ms: if msg.duration_ms > 0 {
                Some(msg.duration_ms as u64)
            } else {
                None
            },
            reasoning_started_at: None,
            t0_ms: msg
                .t0_ms
                .and_then(|v| if v > 0 { Some(v as u64) } else { None }),
            t1_ms: msg
                .t1_ms
                .and_then(|v| if v > 0 { Some(v as u64) } else { None }),
            tn_ms: msg
                .tn_ms
                .and_then(|v| if v > 0 { Some(v as u64) } else { None }),
            output_tokens,
            tokens_per_sec: msg.tokens_per_sec.filter(|v| v.is_finite() && *v > 0.0),
            model: msg.model.clone(),
            provider: msg.provider.clone(),
            local_image_paths,
            compaction_stats,
            was_interrupted,
        })
    }
}

pub fn session_to_persistence(name: String, session: &Session) -> (String, Vec<Message>) {
    let messages: Vec<Message> = session.messages.iter().map(|m| m.clone().into()).collect();
    (name, messages)
}

pub fn persistence_to_session(
    persistence_session: PersistenceSession,
    messages: Vec<Message>,
) -> Result<Session, anyhow::Error> {
    let mut session = Session::new();
    session.parent_id = persistence_session.parent_session_identifier;
    for msg in messages {
        session.add_message(msg.try_into()?);
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_stats_round_trip_through_message_parts() {
        let stats = CompactionStats {
            before_tokens: 12_000,
            after_tokens: 360,
            before_messages: 8,
            after_messages: 2,
        };
        let mut session_message = SessionMessage::user("summary");
        session_message.compaction_stats = Some(stats);

        let persistence_message: Message = session_message.into();
        assert!(persistence_message
            .parts
            .iter()
            .any(|part| part.part_type == "compaction_stats"));

        let restored = SessionMessage::try_from(persistence_message).unwrap();
        assert_eq!(restored.compaction_stats, Some(stats));
    }

    #[test]
    fn interrupted_status_round_trips_through_message_parts() {
        let mut session_message = SessionMessage::assistant("partial");
        session_message.mark_interrupted();

        let persistence_message: Message = session_message.into();
        assert!(persistence_message.parts.iter().any(|part| {
            part.part_type == "status"
                && part.data.get("state").and_then(|value| value.as_str()) == Some("interrupted")
        }));

        let restored = SessionMessage::try_from(persistence_message).unwrap();
        assert!(restored.was_interrupted);
    }

    #[test]
    fn assistant_ordered_parts_round_trip_without_reordering() {
        let mut session_message = SessionMessage::incomplete("");
        session_message.append_reasoning("thinking");
        session_message.append("I will inspect.");
        session_message.add_tool_call_part(
            "call_read",
            "read",
            serde_json::json!({ "path": "src/lib.rs" }),
        );
        session_message.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_read",
            "name": "read",
            "status": "ok",
            "args": { "path": "src/lib.rs" },
            "output_preview": "contents",
        }));
        session_message.append("Done.");

        let persistence_message: Message = session_message.into();
        let restored = SessionMessage::try_from(persistence_message).unwrap();

        assert_eq!(
            restored
                .parts
                .iter()
                .map(|part| part.part_type.as_str())
                .collect::<Vec<_>>(),
            vec!["reasoning", "text", "tool_call", "tool_result", "text"]
        );
        assert_eq!(restored.reasoning.as_deref(), Some("thinking"));
        assert_eq!(restored.content, "I will inspect.\n\nDone.");
    }

    #[test]
    fn usage_parts_set_tokens_used_from_billed_buckets() {
        let mut session_message = SessionMessage::assistant("done");
        session_message.token_count = Some(40);
        session_message
            .parts
            .push(SessionMessagePart::usage(1_000, 200, 500, 50, 0.0123));

        let persistence_message: Message = session_message.into();
        assert_eq!(persistence_message.tokens_used, 1_750);
        assert!(persistence_message
            .parts
            .iter()
            .any(|part| part.part_type == "usage"));
    }

    #[test]
    fn compaction_summary_keeps_context_token_count_not_billed_usage() {
        let mut summary = SessionMessage::user(format!(
            "{}\n{}",
            crate::session::compaction::SUMMARY_PREFIX,
            "handoff summary"
        ));
        summary.token_count = Some(40);
        summary
            .parts
            .push(SessionMessagePart::usage(80_000, 400, 12_000, 1_000, 0.42));

        let persistence_message: Message = summary.into();
        assert_eq!(persistence_message.tokens_used, 40);
        assert!(persistence_message
            .parts
            .iter()
            .any(|part| part.part_type == "usage"));

        let restored = SessionMessage::try_from(persistence_message).unwrap();
        assert_eq!(restored.token_count, Some(40));
        let usage = restored.recorded_usage().unwrap();
        assert_eq!(usage.input, 80_000);
        assert_eq!(usage.output, 400);
    }

    #[test]
    fn precomputed_tps_round_trips_through_persistence() {
        let mut session_message = SessionMessage::assistant("done");
        session_message.output_tokens = Some(390);
        session_message.tokens_per_sec = Some(145.0);

        let persistence_message: Message = session_message.into();
        assert_eq!(persistence_message.tokens_per_sec, Some(145.0));

        let restored = SessionMessage::try_from(persistence_message).unwrap();
        assert_eq!(restored.tokens_per_sec, Some(145.0));
        assert_eq!(restored.output_tokens, Some(390));
    }

    #[test]
    fn billed_output_backfills_output_tokens_not_total() {
        // Legacy row stored before output_tokens existed: tokens_used is the
        // billed total (in+out+cache), usage part carries exact buckets.
        let mut legacy = Message {
            id: "legacy".to_string(),
            session_id: 1,
            role: "assistant".to_string(),
            parts: vec![PersistenceMessagePart {
                part_type: "text".to_string(),
                data: serde_json::json!({ "text": "done" }),
            }],
            timestamp: 0,
            tokens_used: 8000,
            model: None,
            provider: None,
            agent_mode: None,
            duration_ms: 2600,
            t0_ms: Some(1000),
            t1_ms: Some(10_000),
            tn_ms: Some(12_600),
            output_tokens: None,
            tokens_per_sec: None,
        };
        legacy.parts.push(PersistenceMessagePart {
            part_type: "usage".to_string(),
            data: serde_json::json!({
                "input": 7000, "output": 390,
                "cache_read": 500, "cache_write": 110, "cost": 0.01,
            }),
        });

        let restored = SessionMessage::try_from(legacy).unwrap();
        // Output bucket (390), never the billed total (8000).
        assert_eq!(restored.output_tokens, Some(390));
    }
}
