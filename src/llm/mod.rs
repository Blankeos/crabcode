pub mod client;
pub mod provider;
pub mod tool_calls;

pub use client::LLMClient;
pub use tool_calls::{FunctionCall, ToolCall, ToolCallResult};

use tokio::sync::mpsc;

pub enum ChunkMessage {
    Text(String),
    Reasoning(String),
    Warning(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult(ToolCallResult),
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
