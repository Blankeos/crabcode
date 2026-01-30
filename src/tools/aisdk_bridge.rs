use crate::tools::{ToolContext, ToolRegistry};
use aisdk::core::{tools::ToolExecute, Tool};
use schemars::Schema;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::llm::ChunkSender;

static TOOL_CALL_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Convert our ToolRegistry to AISDK Tools
pub async fn convert_to_aisdk_tools(registry: &ToolRegistry, sender: Option<ChunkSender>) -> Vec<Tool> {
    let mut aisdk_tools = Vec::new();
    let tools = registry.list().await;
    
    for tool_def in tools {
        let tool_id = tool_def.id.clone();
        let tool_description = tool_def.description.clone();
        let registry = registry.clone();
        let sender = sender.clone();
        
        // Create the execute function
        let execute = ToolExecute::new(Box::new(move |input: Value| {
            let tool_id = tool_id.clone();
            let tool_id_for_exec = tool_id.clone();
            let tool_id_for_ui = tool_id.clone();

            let tool_description = tool_description.clone();
            let tool_description_for_ui = tool_description.clone();
            let registry = registry.clone();
            let sender = sender.clone();

            let call_seq = TOOL_CALL_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
            let call_id = format!("call_{call_seq}");

            if let Some(ref sender) = sender {
                // Surface tool call start to the UI
                let args = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                let _ = sender.send(crate::llm::ChunkMessage::ToolCalls(vec![crate::llm::ToolCall {
                    id: call_id.clone(),
                    call_type: "function".to_string(),
                    function: crate::llm::FunctionCall {
                        name: tool_id.clone(),
                        arguments: args,
                    },
                }]));
            }

            let sender_for_block = sender.clone();
            let call_id_for_block = call_id.clone();
            let tool_id_for_ui_block = tool_id_for_ui.clone();

            // aisdk tool execution is synchronous (Fn(Value) -> Result<String, String>),
            // but our tools are async. Bridge by blocking in-place on the current runtime.
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                    let _ = crate::logging::log(&format!(
                        "[AISDK_TOOL] call {} args={} ",
                        tool_id_for_exec,
                        input
                    ));

                    let handler = registry
                        .get(&tool_id_for_exec)
                        .await
                        .ok_or_else(|| format!("Tool '{}' not found", tool_id_for_exec))?;

                    if let Err(e) = handler.validate(&input) {
                        return Err(format!("Validation error: {}", e));
                    }

                    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
                    let ctx = ToolContext::new("session", "message", "aisdk", abort_rx);

                    let tool_result = handler
                        .execute(input, &ctx)
                        .await
                        .map_err(|e| format!("Execution error: {}", e))?;

                    let _ = crate::logging::log(&format!(
                        "[AISDK_TOOL] result {} bytes={}",
                        tool_id_for_exec,
                        tool_result.output.len()
                    ));

                    if let Some(ref sender) = sender_for_block {
                        let preview_limit: usize = 4000;
                        let mut preview = tool_result.output.clone();
                        if preview.len() > preview_limit {
                            preview.truncate(preview_limit);
                            preview.push_str("... (truncated)");
                        }

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

                        let _ = sender.send(crate::llm::ChunkMessage::ToolResult(
                            crate::llm::ToolCallResult {
                                tool_call_id: call_id_for_block.clone(),
                                role: "tool".to_string(),
                                name: tool_id_for_ui_block.clone(),
                                content: payload,
                            },
                        ));
                    }

                    Ok(tool_result.output)
                })
            });

            if let (Err(err), Some(ref sender)) = (&result, sender.as_ref()) {
                // Error path: emit structured error payload.
                let payload = serde_json::json!({
                    "status": "error",
                    "title": tool_description_for_ui,
                    "output_preview": format!("{}", err),
                })
                .to_string();
                let _ = sender.send(crate::llm::ChunkMessage::ToolResult(
                    crate::llm::ToolCallResult {
                        tool_call_id: call_id.clone(),
                        role: "tool".to_string(),
                        name: tool_id_for_ui.clone(),
                        content: payload,
                    },
                ));
            }

            result
        }));
        
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
                let _ = crate::logging::log(&format!(
                    "Error creating schema for tool {}: {} (falling back to any schema)",
                    tool_def.id, e
                ));
                Schema::from(true)
            }
        };
        
        let aisdk_tool = match Tool::builder()
            .name(&tool_def.id)
            .description(&tool_def.description)
            .input_schema(schema)
            .execute(execute)
            .build() {
            Ok(t) => t,
            Err(e) => {
                let _ = crate::logging::log(&format!("Error building tool {}: {}", tool_def.id, e));
                continue;
            }
        };
        
        aisdk_tools.push(aisdk_tool);
    }
    
    aisdk_tools
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
