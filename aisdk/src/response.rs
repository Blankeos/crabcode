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
            let mut tool_calls_to_execute: Vec<(String, String, serde_json::Value)> = Vec::new();
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
                        if let Ok(parsed) = parse_tool_calls(&json_str) {
                            for (id, name, args) in parsed {
                                tool_calls_to_execute.push((id, name, args));
                            }
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

fn parse_tool_calls(
    json_str: &str,
) -> std::result::Result<Vec<(String, String, serde_json::Value)>, serde_json::Error> {
    let parsed: serde_json::Value = serde_json::from_str(json_str)?;
    let mut results = Vec::new();

    if let Some(arr) = parsed.as_array() {
        for item in arr {
            if let (Some(id), Some(function)) = (
                item.get("id").and_then(|v| v.as_str()),
                item.get("function"),
            ) {
                let name = function
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = function
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                results.push((id.to_string(), name, args));
            }
        }
    }

    Ok(results)
}
