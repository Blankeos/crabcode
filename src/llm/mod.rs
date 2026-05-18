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
    SubagentStarted {
        parent_session_id: String,
        session_id: String,
        title: String,
        subagent_type: String,
        description: String,
        prompt: String,
    },
    SubagentChunk {
        session_id: String,
        chunk: Box<ChunkMessage>,
    },
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
