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
        usage: Option<LanguageModelUsage>,
    },
    Retry(crate::retry::RetryStatus),
    StreamRollback {
        text: String,
        reasoning: String,
    },
    Warning(String),
    Metadata(String),
    /// Provider-reported token usage for one model request.
    Usage(LanguageModelUsage),
    End {
        reason: Option<FinishReason>,
    },
    RetryableFailure(crate::retry::RetryError),
    Failed(String),
    Incomplete(String),
    NotSupported(String),
}

/// Normalized provider usage. `input_tokens` includes cached input; cache
/// fields describe subsets used for pricing and observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LanguageModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl LanguageModelUsage {
    pub fn is_empty(self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_write_tokens == 0
    }
}

impl std::ops::AddAssign for LanguageModelUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens = self.input_tokens.saturating_add(rhs.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(rhs.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(rhs.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(rhs.cache_write_tokens);
    }
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
