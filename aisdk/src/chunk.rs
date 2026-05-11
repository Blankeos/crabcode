#[derive(Debug, Clone)]
pub enum ChunkType {
    Start,
    Text(String),
    Reasoning(String),
    ToolCall(String),
    End(String),
    Failed(String),
    Incomplete(String),
    NotSupported(String),
}
