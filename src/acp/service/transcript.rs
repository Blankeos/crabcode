pub(super) fn record_tool_call(
    assistant: &mut crate::session::types::Message,
    tool_call: &crate::llm::ToolCall,
) {
    let args = serde_json::from_str(&tool_call.function.arguments)
        .unwrap_or_else(|_| serde_json::Value::String(tool_call.function.arguments.clone()));
    let provider_executed = matches!(
        tool_call.function.name.as_str(),
        "x_search" | "web_search" | "file_search"
    );

    if let Some(part) = assistant
        .parts
        .iter_mut()
        .find(|part| part.part_type == "tool_call" && part.tool_id() == Some(tool_call.id.as_str()))
    {
        if let Some(obj) = part.data.as_object_mut() {
            obj.insert(
                "name".to_string(),
                serde_json::Value::String(tool_call.function.name.clone()),
            );
            obj.insert("args".to_string(), args);
            if provider_executed {
                obj.insert(
                    "provider_executed".to_string(),
                    serde_json::Value::Bool(true),
                );
            }
        }
        return;
    }

    assistant.add_tool_call_part(tool_call.id.clone(), tool_call.function.name.clone(), args);
    if provider_executed {
        if let Some(obj) = assistant
            .parts
            .last_mut()
            .and_then(|part| part.data.as_object_mut())
        {
            obj.insert(
                "provider_executed".to_string(),
                serde_json::Value::Bool(true),
            );
        }
    }
}

pub(super) fn record_tool_result(
    assistant: &mut crate::session::types::Message,
    result: &crate::llm::ToolCallResult,
) {
    let mut data = assistant
        .tool_call_part_data(&result.tool_call_id)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    data["id"] = serde_json::Value::String(result.tool_call_id.clone());
    data["name"] = serde_json::Value::String(result.name.clone());
    if matches!(
        result.name.as_str(),
        "x_search" | "web_search" | "file_search"
    ) {
        data["provider_executed"] = serde_json::Value::Bool(true);
    }

    match serde_json::from_str::<serde_json::Value>(&result.content) {
        Ok(serde_json::Value::Object(payload)) => {
            if let Some(data) = data.as_object_mut() {
                for (key, value) in payload {
                    data.insert(key, value);
                }
            }
        }
        _ => {
            data["status"] = serde_json::Value::String("ok".to_string());
            data["output_preview"] = serde_json::Value::String(result.content.clone());
        }
    }
    assistant.add_or_update_tool_result_part(data);
}
