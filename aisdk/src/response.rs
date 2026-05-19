use crate::chunk::ChunkType;
use crate::error::Result;
use crate::message::Message;
use crate::provider::Provider;
use crate::stop::{StopReason, StopWhenFn};
use crate::tool::Tool;
use futures::StreamExt;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct StreamTextResponse {
    pub stream: LanguageModelStream,
    stop_reason: Arc<tokio::sync::Mutex<Option<StopReason>>>,
    messages: Arc<tokio::sync::Mutex<Vec<Message>>>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
}

pub struct LanguageModelStream {
    rx: mpsc::UnboundedReceiver<ChunkType>,
}

impl futures::Stream for LanguageModelStream {
    type Item = ChunkType;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

impl StreamTextResponse {
    fn create() -> (Self, mpsc::UnboundedSender<ChunkType>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let stop_reason = Arc::new(tokio::sync::Mutex::new(None));
        let messages = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        (
            Self {
                stream: LanguageModelStream { rx },
                stop_reason: stop_reason.clone(),
                messages: messages.clone(),
                _handles: Vec::new(),
            },
            tx,
        )
    }

    pub async fn stop_reason(&self) -> Option<StopReason> {
        self.stop_reason.lock().await.clone()
    }

    pub async fn messages(&self) -> Vec<Message> {
        self.messages.lock().await.clone()
    }

    fn add_handle(&mut self, handle: tokio::task::JoinHandle<()>) {
        self._handles.push(handle);
    }
}

pub async fn stream_with_tools<P: Provider>(
    provider: P,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    max_steps: Option<usize>,
    stop_when: Option<StopWhenFn>,
    headers: HashMap<String, String>,
) -> Result<StreamTextResponse> {
    let (mut response, tx) = StreamTextResponse::create();
    let _ = tx.send(ChunkType::Start);

    let tx_loop = tx.clone();
    let stop_reason_arc = response.stop_reason.clone();
    let messages_arc = response.messages.clone();
    let provider_clone = provider.clone();

    let handle = tokio::spawn(async move {
        let mut current_messages = messages;
        let mut step_idx: usize = 0;
        let max_steps = max_steps.unwrap_or(usize::MAX);

        loop {
            step_idx += 1;

            if step_idx > max_steps {
                let _ = tx_loop.send(ChunkType::Incomplete("Max steps reached".to_string()));
                *stop_reason_arc.lock().await = Some(StopReason::Hook);
                break;
            }

            if let Some(ref hook) = stop_when {
                if hook(step_idx) {
                    *stop_reason_arc.lock().await = Some(StopReason::Hook);
                    break;
                }
            }

            let stream_result = provider_clone
                .stream_text(&current_messages, &tools, &headers)
                .await;

            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx_loop.send(ChunkType::Failed(e.to_string()));
                    *stop_reason_arc.lock().await = Some(StopReason::Error(e.to_string()));
                    break;
                }
            };

            let mut has_tool_call = false;
            let mut tool_call_accumulator = ToolCallAccumulator::default();
            let mut accumulated_text = String::new();

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(ChunkType::Text(text)) => {
                        accumulated_text.push_str(&text);
                        let _ = tx_loop.send(ChunkType::Text(text));
                    }
                    Ok(ChunkType::Reasoning(reasoning)) => {
                        let _ = tx_loop.send(ChunkType::Reasoning(reasoning));
                    }
                    Ok(ChunkType::ToolCall(json_str)) => {
                        has_tool_call = true;
                        let _ = tx_loop.send(ChunkType::ToolCall(json_str.clone()));
                        if let Err(err) = tool_call_accumulator.ingest(&json_str) {
                            let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                            *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                            return;
                        }
                    }
                    Ok(ChunkType::End(_content)) => {
                        // Processed internally — NOT forwarded to tx_loop.
                        // Forwarding End would cause relay_stream_to_sender
                        // to return Ended prematurely, dropping the channel
                        // before tool execution / subsequent steps.
                    }
                    Ok(ChunkType::Incomplete(msg)) => {
                        let _ = tx_loop.send(ChunkType::Incomplete(msg));
                    }
                    Ok(ChunkType::Failed(err)) => {
                        let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                        *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                        return;
                    }
                    Ok(ChunkType::Start) => {
                        let _ = tx_loop.send(ChunkType::Start);
                    }
                    Ok(ChunkType::NotSupported(msg)) => {
                        let _ = tx_loop.send(ChunkType::NotSupported(msg));
                    }
                    Err(e) => {
                        let err = e.to_string();
                        let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                        *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                        return;
                    }
                }
            }

            // Build assistant message from accumulated text deltas
            let assistant_text = accumulated_text.trim().to_string();
            if !assistant_text.is_empty() {
                let assistant_msg = Message::assistant(&assistant_text);
                current_messages.push(assistant_msg.clone());
                messages_arc.lock().await.push(assistant_msg);
            }

            if !has_tool_call {
                *stop_reason_arc.lock().await = Some(StopReason::Finish);
                break;
            }

            let tool_calls_to_execute = match tool_call_accumulator.finish() {
                Ok(tool_calls) if !tool_calls.is_empty() => tool_calls,
                Ok(_) => {
                    let err = "Tool call stream did not contain executable tool calls".to_string();
                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                    return;
                }
                Err(err) => {
                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                    return;
                }
            };

            for (_call_id, tool_name, args) in &tool_calls_to_execute {
                let tool = tools.iter().find(|t| &t.name == tool_name);
                match tool {
                    Some(t) => match t.execute.call(args.clone()).await {
                        Ok(result) => {
                            let observation = format!("Tool `{}` result:\n{}", tool_name, result);
                            current_messages.push(Message::user(observation.clone()));
                            messages_arc.lock().await.push(Message::user(observation));
                        }
                        Err(e) => {
                            let _ = tx_loop.send(ChunkType::Failed(format!(
                                "Tool '{}' error: {}",
                                tool_name, e
                            )));
                        }
                    },
                    None => {
                        let _ = tx_loop
                            .send(ChunkType::Failed(format!("Tool not found: {}", tool_name)));
                    }
                }
            }
        }
        let _ = std::fs::write("aisdk_debug.log", "spawned task done, dropping tx\n");
    });

    response.add_handle(handle);
    Ok(response)
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    calls: Vec<PendingToolCall>,
}

#[derive(Debug)]
struct PendingToolCall {
    key: String,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    saw_arguments: bool,
}

impl ToolCallAccumulator {
    fn ingest(&mut self, json_str: &str) -> std::result::Result<(), String> {
        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| format!("Invalid tool call delta: {}", e))?;

        let items = parsed
            .as_array()
            .ok_or_else(|| "Unsupported tool call delta shape".to_string())?;

        for (array_index, item) in items.iter().enumerate() {
            self.ingest_openai_delta(item, array_index)?;
        }

        Ok(())
    }

    fn finish(self) -> std::result::Result<Vec<(String, String, serde_json::Value)>, String> {
        let mut results = Vec::new();

        for call in self.calls {
            let name = call
                .name
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("Tool call '{}' missing function name", call.key))?;

            let id = call.id.unwrap_or(call.key);
            let args = if !call.saw_arguments || call.arguments.trim().is_empty() {
                serde_json::Value::Object(Default::default())
            } else {
                serde_json::from_str(&call.arguments).map_err(|e| {
                    format!(
                        "Tool call '{}' arguments are incomplete or invalid JSON: {}",
                        id, e
                    )
                })?
            };

            results.push((id, name, args));
        }

        Ok(results)
    }

    fn ingest_openai_delta(
        &mut self,
        item: &serde_json::Value,
        array_index: usize,
    ) -> std::result::Result<(), String> {
        let key = tool_call_key(item, array_index);
        let pending = self.pending_for_key(key, item);

        if pending.id.is_none() {
            pending.id = item
                .get("id")
                .and_then(|value| value.as_str())
                .filter(|id| !id.is_empty())
                .map(ToString::to_string);
        }

        if let Some(function) = item.get("function") {
            if pending.name.is_none() {
                pending.name = function
                    .get("name")
                    .and_then(|value| value.as_str())
                    .filter(|name| !name.is_empty())
                    .map(ToString::to_string);
            }

            if let Some(arguments) = function.get("arguments") {
                pending.saw_arguments = true;
                match arguments {
                    serde_json::Value::String(delta) => pending.arguments.push_str(delta),
                    serde_json::Value::Null => {}
                    value => pending.arguments.push_str(&value.to_string()),
                }
            }
        }

        Ok(())
    }

    fn pending_for_key(&mut self, key: String, item: &serde_json::Value) -> &mut PendingToolCall {
        if let Some(index) = self.calls.iter().position(|call| call.key == key) {
            return &mut self.calls[index];
        }

        if let Some(id) = item
            .get("id")
            .and_then(|value| value.as_str())
            .filter(|id| !id.is_empty())
        {
            if let Some(index) = self
                .calls
                .iter()
                .position(|call| call.id.as_deref() == Some(id))
            {
                return &mut self.calls[index];
            }
        }

        self.calls.push(PendingToolCall {
            key,
            id: None,
            name: None,
            arguments: String::new(),
            saw_arguments: false,
        });
        self.calls.last_mut().expect("pending tool call exists")
    }
}

fn tool_call_key(item: &serde_json::Value, array_index: usize) -> String {
    if let Some(index) = item.get("index").and_then(|value| value.as_u64()) {
        return format!("index:{}", index);
    }

    if let Some(id) = item
        .get("id")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
    {
        return format!("id:{}", id);
    }

    format!("position:{}", array_index)
}

#[cfg(test)]
mod tests {
    use super::ToolCallAccumulator;

    #[test]
    fn accumulates_streamed_openai_tool_call_arguments() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]"#,
            )
            .unwrap();
        accumulator
            .ingest(r#"[{"index":0,"function":{"arguments":":\"ls -la\"}"}}]"#)
            .unwrap();

        let calls = accumulator.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "call_1");
        assert_eq!(calls[0].1, "bash");
        assert_eq!(calls[0].2["command"], "ls -la");
    }

    #[test]
    fn rejects_incomplete_tool_call_arguments() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]"#,
            )
            .unwrap();

        let error = accumulator.finish().unwrap_err();

        assert!(error.contains("arguments are incomplete or invalid JSON"));
    }

    #[test]
    fn supports_multiple_tool_calls_by_index() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"file_path\""}},{"index":1,"id":"call_2","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]"#,
            )
            .unwrap();
        accumulator
            .ingest(
                r#"[{"index":0,"function":{"arguments":":\"Cargo.toml\"}"}},{"index":1,"function":{"arguments":":\"cargo test\"}"}}]"#,
            )
            .unwrap();

        let calls = accumulator.finish().unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, "read");
        assert_eq!(calls[0].2["file_path"], "Cargo.toml");
        assert_eq!(calls[1].1, "bash");
        assert_eq!(calls[1].2["command"], "cargo test");
    }

    #[test]
    fn empty_arguments_become_empty_object() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"list","arguments":""}}]"#,
            )
            .unwrap();

        let calls = accumulator.finish().unwrap();

        assert_eq!(calls[0].2, serde_json::json!({}));
    }
}
