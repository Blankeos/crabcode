#[derive(Debug, Clone)]
pub enum ChunkType {
    Start,
    Text(String),
    Reasoning(String),
    ToolCall(String),
    AssistantMessagePhase { phase: Option<MessagePhase> },
    ResponseCompleted { end_turn: Option<bool> },
    Metadata(String),
    End(String),
    Failed(String),
    Incomplete(String),
    NotSupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePhase {
    Commentary,
    FinalAnswer,
}
