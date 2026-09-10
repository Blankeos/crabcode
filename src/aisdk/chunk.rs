#[derive(Debug, Clone)]
pub enum ChunkType {
    Start,
    Text(String),
    Reasoning(String),
    ToolCall(String),
    /// Provider-executed tool lifecycle (hosted search, etc.).
    ///
    /// Display / observability only — must never enter the client tool-execute
    /// loop. Payload JSON:
    /// `{ "id", "name", "status": "running"|"completed"|"failed", "arguments"?, "output"? }`.
    ProviderToolCall(String),
    AssistantMessagePhase {
        phase: Option<MessagePhase>,
    },
    /// Opaque Responses reasoning item for the next provider step.
    /// Display text still arrives via [`ChunkType::Reasoning`].
    ReasoningItem(ReasoningReplayItem),
    ResponseCompleted {
        end_turn: Option<bool>,
        reasoning_items: Vec<ReasoningReplayItem>,
        doom_loop_triggers: Vec<String>,
        usage: Option<TokenUsage>,
    },
    Retry(crate::retry::RetryStatus),
    StreamRollback {
        text: String,
        reasoning: String,
    },
    Warning(String),
    Metadata(String),
    /// Provider-billed token usage for the current step.
    ///
    /// `input` is non-cached prompt tokens. Cache hits/writes are separate so
    /// hosts can price them at different rates.
    Usage(TokenUsage),
    End {
        reason: Option<FinishReason>,
    },
    RetryableFailure(crate::retry::RetryError),
    Failed(String),
    Incomplete(String),
    NotSupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReasoningReplayItem {
    pub id: Option<String>,
    pub summary: String,
    pub encrypted_content: Option<String>,
}

impl ReasoningReplayItem {
    pub fn is_empty(&self) -> bool {
        self.id.as_deref().is_none_or(str::is_empty)
            && self.summary.is_empty()
            && self.encrypted_content.as_deref().is_none_or(str::is_empty)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    /// Non-cached input tokens.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenUsage {
    pub fn is_empty(self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_write == 0
    }

    pub fn total(self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
        }
    }
}

impl ChunkType {
    pub fn response_completed(end_turn: Option<bool>) -> Self {
        Self::ResponseCompleted {
            end_turn,
            reasoning_items: Vec::new(),
            doom_loop_triggers: Vec::new(),
            usage: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePhase {
    Commentary,
    FinalAnswer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Refusal,
    EndTurn,
    StopSequence,
    PauseTurn,
    Unknown(String),
}

impl FinishReason {
    pub fn from_openai_compatible(reason: &str) -> Self {
        match reason {
            "stop" => Self::Stop,
            "tool_calls" | "function_call" => Self::ToolCalls,
            "length" => Self::Length,
            "content_filter" => Self::ContentFilter,
            "refusal" => Self::Refusal,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn from_anthropic(reason: &str) -> Self {
        match reason {
            "end_turn" => Self::EndTurn,
            "tool_use" => Self::ToolCalls,
            "max_tokens" => Self::Length,
            "stop_sequence" => Self::StopSequence,
            "pause_turn" => Self::PauseTurn,
            "refusal" => Self::Refusal,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Stop => "stop",
            Self::ToolCalls => "tool_calls",
            Self::Length => "length",
            Self::ContentFilter => "content_filter",
            Self::Refusal => "refusal",
            Self::EndTurn => "end_turn",
            Self::StopSequence => "stop_sequence",
            Self::PauseTurn => "pause_turn",
            Self::Unknown(reason) => reason.as_str(),
        }
    }

    /// True when a phase-less provider gave a stop reason that is strong
    /// enough to accept as a final assistant response without another agent
    /// loop step. Anthropic `end_turn` is intentionally excluded: it marks the
    /// provider message boundary, not a Codex-style final-answer phase.
    pub fn is_final_assistant_stop(&self) -> bool {
        matches!(self, Self::Stop | Self::StopSequence)
    }
}
