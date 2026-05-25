use crate::tools::{ToolContext, ToolRegistry};
use aisdk::core::tools::{ToolExecute, ToolOutput};
use aisdk::core::Tool;
use schemars::Schema;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::llm::ChunkSender;

const TOOL_UI_PREVIEW_LIMIT: usize = 4_000;
const TOOL_MODEL_OUTPUT_LIMIT: usize = 60_000;

static TOOL_CALL_SEQ: AtomicUsize = AtomicUsize::new(0);

pub async fn convert_to_aisdk_tools(
    registry: &ToolRegistry,
    sender: Option<ChunkSender>,
    agent_mode: String,
    permissions: crate::tools::ToolPermissions,
    session_id: Option<String>,
    message_id: Option<String>,
) -> Vec<Tool> {
    let mut aisdk_tools = Vec::new();
    let tools = registry.list().await;

    for tool_def in tools {
        if !permissions.is_tool_allowed_for_agent(&agent_mode, &tool_def.id) {
            crate::emit_log!(
                "[AISDK_TOOLS] Skipping '{}': not allowed in {} mode",
                tool_def.id,
                agent_mode
            );
            continue;
        }

        let tool_id = tool_def.id.clone();
        let registry = registry.clone();
        let sender = sender.clone();
        let agent_mode = agent_mode.clone();
        let permissions = permissions.clone();
        let session_id = session_id.clone();
        let message_id = message_id.clone();

        let execute = ToolExecute::new(move |input: Value| {
            let tool_id = tool_id.clone();
            let tool_id_for_exec = tool_id.clone();
            let tool_id_for_ui = tool_id.clone();

            let registry = registry.clone();
            let sender = sender.clone();
            let agent_mode = agent_mode.clone();
            let permissions = permissions.clone();
            let session_id = session_id.clone();
            let message_id = message_id.clone();

            async move {
                let call_seq = TOOL_CALL_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
                let call_id = format!("call_{call_seq}");
                let started_at = Instant::now();
                let session_id_label = session_id.as_deref().unwrap_or("session");
                let message_id_label = message_id.as_deref().unwrap_or("message");
                let sender_present = sender.is_some();

                if let Some(ref sender) = sender {
                    let args = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                    if sender
                        .send(crate::llm::ChunkMessage::ToolCalls(vec![
                            crate::llm::ToolCall {
                                id: call_id.clone(),
                                call_type: "function".to_string(),
                                function: crate::llm::FunctionCall {
                                    name: tool_id.clone(),
                                    arguments: args,
                                },
                            },
                        ]))
                        .is_err()
                    {
                        crate::emit_log!(
                            "[AISDK_TOOL] ui_send_failed phase=tool_call tool={} call_id={} session_id={} message_id={} agent_mode={}",
                            tool_id, call_id, session_id_label, message_id_label, agent_mode
                        );
                    }
                }

                crate::emit_log!(
                    "[AISDK_TOOL] call tool={} call_id={} session_id={} message_id={} agent_mode={} sender_present={} args={}",
                    tool_id_for_exec,
                    call_id,
                    session_id_label,
                    message_id_label,
                    agent_mode,
                    sender_present,
                    input
                );

                let handler = registry
                    .get(&tool_id_for_exec)
                    .await
                    .ok_or_else(|| format!("Tool '{}' not found", tool_id_for_exec));
                let handler = match handler {
                    Ok(handler) => handler,
                    Err(err) => {
                        send_tool_error_result(sender.as_ref(), &call_id, &tool_id_for_ui, &err);
                        crate::emit_log!(
                            "[AISDK_TOOL] error tool={} call_id={} session_id={} message_id={} agent_mode={} duration_ms={} error={}",
                            tool_id_for_exec,
                            call_id,
                            session_id_label,
                            message_id_label,
                            agent_mode,
                            started_at.elapsed().as_millis(),
                            err
                        );
                        return Err(err);
                    }
                };

                if let Err(e) = handler.validate(&input) {
                    let err = format!("Validation error: {}", e);
                    send_tool_error_result(sender.as_ref(), &call_id, &tool_id_for_ui, &err);
                    crate::emit_log!(
                        "[AISDK_TOOL] error tool={} call_id={} session_id={} message_id={} agent_mode={} duration_ms={} error={}",
                        tool_id_for_exec,
                        call_id,
                        session_id_label,
                        message_id_label,
                        agent_mode,
                        started_at.elapsed().as_millis(),
                        err
                    );
                    return Err(err);
                }

                if let Err(e) = permissions
                    .preflight(&agent_mode, &tool_id_for_exec, &input, sender.as_ref())
                    .await
                {
                    let err = format!("{}", e);
                    send_tool_error_result(sender.as_ref(), &call_id, &tool_id_for_ui, &err);
                    crate::emit_log!(
                        "[AISDK_TOOL] error tool={} call_id={} session_id={} message_id={} agent_mode={} duration_ms={} error={}",
                        tool_id_for_exec,
                        call_id,
                        session_id_label,
                        message_id_label,
                        agent_mode,
                        started_at.elapsed().as_millis(),
                        err
                    );
                    return Err(err);
                }

                let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
                let ctx = ToolContext::new(
                    session_id.clone().unwrap_or_else(|| "session".to_string()),
                    message_id.clone().unwrap_or_else(|| "message".to_string()),
                    agent_mode.clone(),
                    abort_rx,
                )
                .with_call_id(call_id.clone());

                let tool_result = handler
                    .execute(input, &ctx)
                    .await
                    .map_err(|e| format!("Execution error: {}", e));
                let tool_result = match tool_result {
                    Ok(tool_result) => tool_result,
                    Err(err) => {
                        send_tool_error_result(sender.as_ref(), &call_id, &tool_id_for_ui, &err);
                        crate::emit_log!(
                            "[AISDK_TOOL] error tool={} call_id={} session_id={} message_id={} agent_mode={} duration_ms={} error={}",
                            tool_id_for_exec,
                            call_id,
                            session_id_label,
                            message_id_label,
                            agent_mode,
                            started_at.elapsed().as_millis(),
                            err
                        );
                        return Err(err);
                    }
                };

                crate::emit_log!(
                    "[AISDK_TOOL] result tool={} call_id={} session_id={} message_id={} agent_mode={} duration_ms={} output_bytes={}",
                    tool_id_for_exec,
                    call_id,
                    session_id_label,
                    message_id_label,
                    agent_mode,
                    started_at.elapsed().as_millis(),
                    tool_result.output.len()
                );

                let model_images = tool_result
                    .images
                    .iter()
                    .map(|image| aisdk::message::ImageContent {
                        data_url: image.data_url.clone(),
                        media_type: image.media_type.clone(),
                    })
                    .collect::<Vec<_>>();
                let model_output = ToolOutput::new(truncate_tool_output(
                    &tool_result.output,
                    TOOL_MODEL_OUTPUT_LIMIT,
                ))
                .with_images(model_images);

                if let Some(ref sender) = sender {
                    let preview = truncate_tool_output(&tool_result.output, TOOL_UI_PREVIEW_LIMIT);

                    let line_count = tool_result.output.lines().count();
                    let meta = serde_json::Value::Object(
                        tool_result
                            .metadata
                            .into_iter()
                            .collect::<serde_json::Map<String, serde_json::Value>>(),
                    );

                    let payload = serde_json::json!({
                        "status": "ok",
                        "title": tool_result.title,
                        "output_preview": preview,
                        "line_count": line_count,
                        "metadata": meta,
                    })
                    .to_string();

                    if sender
                        .send(crate::llm::ChunkMessage::ToolResult(
                            crate::llm::ToolCallResult {
                                tool_call_id: call_id.clone(),
                                role: "tool".to_string(),
                                name: tool_id_for_ui.clone(),
                                content: payload,
                            },
                        ))
                        .is_err()
                    {
                        crate::emit_log!(
                            "[AISDK_TOOL] ui_send_failed phase=tool_result tool={} call_id={} session_id={} message_id={} agent_mode={}",
                            tool_id_for_ui, call_id, session_id_label, message_id_label, agent_mode
                        );
                    }
                }

                Ok(model_output)
            }
        });

        // Build the tool schema from parameters
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for param in &tool_def.parameters {
            let schema = param_to_json_schema(&param.param_type);
            properties.insert(param.name.clone(), schema);
            if param.required {
                required.push(param.name.clone());
            }
        }

        let input_schema_json = serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required
        });

        let schema: Schema = match serde_json::from_value(input_schema_json) {
            Ok(s) => s,
            Err(e) => {
                crate::emit_log!(
                    "Error creating schema for tool {}: {} (falling back to any schema)",
                    tool_def.id,
                    e
                );
                Schema::from(true)
            }
        };

        let aisdk_tool = match Tool::builder()
            .name(&tool_def.id)
            .description(&tool_def.description)
            .input_schema(schema)
            .execute(execute)
            .build()
        {
            Ok(t) => t,
            Err(e) => {
                crate::emit_log!("Error building tool {}: {}", tool_def.id, e);
                continue;
            }
        };

        aisdk_tools.push(aisdk_tool);
    }

    aisdk_tools
}

fn truncate_tool_output(output: &str, limit: usize) -> String {
    if output.len() <= limit {
        return output.to_string();
    }

    let boundary = output.floor_char_boundary(limit);
    let mut truncated = output[..boundary].to_string();
    truncated.push_str(&format!(
        "\n\n... (tool output truncated to {} bytes; narrow the request for more)",
        limit
    ));
    truncated
}

fn send_tool_error_result(
    sender: Option<&ChunkSender>,
    call_id: &str,
    tool_name: &str,
    error: &str,
) {
    let Some(sender) = sender else {
        return;
    };

    let preview = truncate_tool_output(error, TOOL_UI_PREVIEW_LIMIT);
    let payload = serde_json::json!({
        "status": "error",
        "title": "Tool failed",
        "output_preview": preview,
        "line_count": error.lines().count().max(1),
        "metadata": {
            "error": error,
        },
    })
    .to_string();

    let _ = sender.send(crate::llm::ChunkMessage::ToolResult(
        crate::llm::ToolCallResult {
            tool_call_id: call_id.to_string(),
            role: "tool".to_string(),
            name: tool_name.to_string(),
            content: payload,
        },
    ));
}

fn param_to_json_schema(param_type: &crate::tools::ParameterType) -> serde_json::Value {
    use crate::tools::ParameterType;

    match param_type {
        ParameterType::String => serde_json::json!({"type": "string"}),
        ParameterType::Integer => serde_json::json!({"type": "integer"}),
        ParameterType::Boolean => serde_json::json!({"type": "boolean"}),
        ParameterType::Array(inner) => {
            serde_json::json!({
                "type": "array",
                "items": param_to_json_schema(inner)
            })
        }
        ParameterType::Object(props) => {
            let mut properties = serde_json::Map::new();
            for (key, val) in props {
                properties.insert(key.clone(), param_to_json_schema(val));
            }
            serde_json::json!({
                "type": "object",
                "properties": properties
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{send_tool_error_result, truncate_tool_output};

    #[test]
    fn truncate_tool_output_bounds_large_results() {
        let output = "a".repeat(70_000);

        let truncated = truncate_tool_output(&output, 60_000);

        assert!(truncated.len() < output.len());
        assert!(truncated.contains("tool output truncated to 60000 bytes"));
    }

    #[test]
    fn truncate_tool_output_preserves_small_results() {
        let output = "small result";

        assert_eq!(truncate_tool_output(output, 60_000), output);
    }

    #[test]
    fn send_tool_error_result_emits_error_payload() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        send_tool_error_result(
            Some(&tx),
            "call_1",
            "edit",
            "Execution error: Could not find text to replace",
        );

        let message = rx.try_recv().expect("expected tool result");
        let crate::llm::ChunkMessage::ToolResult(result) = message else {
            panic!("expected tool result message");
        };

        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.name, "edit");

        let payload: serde_json::Value =
            serde_json::from_str(&result.content).expect("payload should be json");
        assert_eq!(payload["status"], "error");
        assert_eq!(payload["title"], "Tool failed");
        assert_eq!(
            payload["output_preview"],
            "Execution error: Could not find text to replace"
        );
    }
}
