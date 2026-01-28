pub mod client;
pub mod provider;

pub use client::LLMClient;

use tokio::sync::mpsc;

pub enum ChunkMessage {
    Text(String),
    Reasoning(String),
    End,
    Failed(String),
    Cancelled,
    Metrics { token_count: usize, duration_ms: u64 },
}

pub type ChunkSender = mpsc::UnboundedSender<ChunkMessage>;
pub type ChunkReceiver = mpsc::UnboundedReceiver<ChunkMessage>;
