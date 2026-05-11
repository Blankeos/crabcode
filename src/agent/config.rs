use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct LlmSessionConfig {
    pub provider_name: String,
    pub model: String,
    pub api_key: Option<String>,
    pub provider_kind: ProviderKind,
    pub base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAI,
    OpenAICompatible,
    Anthropic,
}

static LLM_SESSION: OnceLock<LlmSessionConfig> = OnceLock::new();

pub fn set_llm_session(config: LlmSessionConfig) {
    let _ = LLM_SESSION.set(config);
}

pub fn get_llm_session() -> Option<&'static LlmSessionConfig> {
    LLM_SESSION.get()
}
