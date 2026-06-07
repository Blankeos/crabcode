use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone)]
pub struct LlmSessionConfig {
    pub provider_name: String,
    pub model: String,
    pub api_key: Option<String>,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    pub supports_image_input: bool,
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
