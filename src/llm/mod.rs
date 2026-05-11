pub mod client;
pub mod provider;
pub mod tool_calls;

pub use tool_calls::{FunctionCall, ToolCall, ToolCallResult};

use tokio::sync::mpsc;

pub enum ChunkMessage {
    Text(String),
    Reasoning(String),
    Warning(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult(ToolCallResult),
    PermissionRequest(crate::tools::PermissionPrompt),
    QuestionRequest {
        questions: serde_json::Value,
        response_tx: tokio::sync::oneshot::Sender<serde_json::Value>,
    },
    End,
    Failed(String),
    Cancelled,
    Metrics {
        token_count: usize,
        duration_ms: u64,
    },
}

pub type ChunkSender = mpsc::UnboundedSender<ChunkMessage>;
pub type ChunkReceiver = mpsc::UnboundedReceiver<ChunkMessage>;
