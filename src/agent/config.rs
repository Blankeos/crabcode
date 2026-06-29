use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenAIRequestOptions {
    pub response_path: Option<String>,
    pub additional_headers: std::collections::HashMap<String, String>,
    pub force_store_false: bool,
    pub default_instructions: Option<String>,
    pub disallow_system_messages: bool,
    pub force_tool_strict_false: bool,
}

#[derive(Debug, Clone)]
pub struct LlmSessionConfig {
    pub provider_name: String,
    pub model: String,
    pub api_key: Option<String>,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    pub supports_image_input: bool,
    pub openai_options: OpenAIRequestOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAI,
    OpenAICompatible,
    Anthropic,
}

static LLM_SESSION: OnceLock<RwLock<Option<LlmSessionConfig>>> = OnceLock::new();

pub fn set_llm_session(config: LlmSessionConfig) {
    let session = LLM_SESSION.get_or_init(|| RwLock::new(None));
    if let Ok(mut guard) = session.write() {
        *guard = Some(config);
    }
}

pub fn get_llm_session() -> Option<LlmSessionConfig> {
    LLM_SESSION
        .get()
        .and_then(|session| session.read().ok())
        .and_then(|guard| guard.clone())
}
