use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenAIRequestOptions {
    pub response_path: Option<String>,
    pub additional_headers: std::collections::HashMap<String, String>,
    pub force_store_false: bool,
    pub default_instructions: Option<String>,
    pub disallow_system_messages: bool,
    pub force_tool_strict_false: bool,
    /// Use the ChatGPT Codex Responses Lite request contract.
    pub use_responses_lite: bool,
    /// Sticky prompt-cache routing key (Responses / chat-completions).
    /// Typically the crabcode session id.
    pub prompt_cache_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmSessionRegistration {
    pub session_id: String,
    pub generation: u64,
}

#[derive(Debug, Clone)]
struct ScopedLlmSessionEntry {
    config: LlmSessionConfig,
    generation: u64,
}

/// Unconditional removal for test cleanup and legacy callers.
pub fn remove_llm_session_for(session_id: &str) {
    if let Some(sessions) = LLM_SESSIONS.get() {
        if let Ok(mut guard) = sessions.write() {
            guard.remove(session_id);
        }
    }
}

/// Removes the scoped config only when `registration` still owns the map entry.
pub fn remove_llm_session_if_owned(registration: &LlmSessionRegistration) {
    if let Some(sessions) = LLM_SESSIONS.get() {
        if let Ok(mut guard) = sessions.write() {
            let should_remove = guard
                .get(&registration.session_id)
                .is_some_and(|entry| entry.generation == registration.generation);
            if should_remove {
                guard.remove(&registration.session_id);
            }
        }
    }
}

pub fn set_llm_session_for(
    session_id: impl Into<String>,
    config: LlmSessionConfig,
) -> LlmSessionRegistration {
    let session_id = session_id.into();
    let generation = LLM_SESSION_GENERATION.fetch_add(1, Ordering::Relaxed);
    let sessions = LLM_SESSIONS.get_or_init(|| RwLock::new(HashMap::new()));
    if let Ok(mut guard) = sessions.write() {
        guard.insert(
            session_id.clone(),
            ScopedLlmSessionEntry { config, generation },
        );
    }
    LlmSessionRegistration {
        session_id,
        generation,
    }
}

pub fn get_llm_session_for(session_id: &str) -> Option<LlmSessionConfig> {
    LLM_SESSIONS
        .get()
        .and_then(|sessions| sessions.read().ok())
        .and_then(|guard| guard.get(session_id).map(|entry| entry.config.clone()))
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
    /// Sticky prompt-cache key for this session (OpenAI/xAI/compatible).
    pub prompt_cache_key: Option<String>,
    /// Vercel AI Gateway: `providerOptions.gateway.caching = "auto"`.
    pub gateway_caching_auto: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAI,
    OpenAICompatible,
    Anthropic,
}

static LLM_SESSION: OnceLock<RwLock<Option<LlmSessionConfig>>> = OnceLock::new();
static LLM_SESSIONS: OnceLock<RwLock<HashMap<String, ScopedLlmSessionEntry>>> = OnceLock::new();
static LLM_SESSION_GENERATION: AtomicU64 = AtomicU64::new(1);

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

#[cfg(test)]
pub(crate) fn test_scoped_llm_session_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config(model: &str) -> LlmSessionConfig {
        LlmSessionConfig {
            provider_name: "test-provider".to_string(),
            model: model.to_string(),
            api_key: None,
            provider_kind: ProviderKind::OpenAICompatible,
            base_url: "https://example.test".to_string(),
            reasoning_effort: None,
            supports_image_input: false,
            openai_options: OpenAIRequestOptions::default(),
            prompt_cache_key: None,
            gateway_caching_auto: false,
        }
    }

    fn cleanup_session(session_id: &str) {
        remove_llm_session_for(session_id);
    }

    #[test]
    fn session_scoped_llm_config_is_isolated_per_session_id() {
        let _lock = test_scoped_llm_session_lock();
        let _reg_a = set_llm_session_for("parent-a", sample_config("model-a"));
        let _reg_b = set_llm_session_for("parent-b", sample_config("model-b"));

        let a = get_llm_session_for("parent-a").expect("session a");
        let b = get_llm_session_for("parent-b").expect("session b");
        assert_eq!(a.model, "model-a");
        assert_eq!(b.model, "model-b");

        remove_llm_session_for("parent-a");
        assert!(get_llm_session_for("parent-a").is_none());
        assert_eq!(
            get_llm_session_for("parent-b").expect("session b").model,
            "model-b"
        );

        cleanup_session("parent-a");
        cleanup_session("parent-b");
    }

    #[test]
    fn stale_scoped_cleanup_does_not_remove_replacement_registration() {
        let _lock = test_scoped_llm_session_lock();
        let session_id = "race-session";
        let first = set_llm_session_for(session_id, sample_config("first-model"));
        let _second = set_llm_session_for(session_id, sample_config("second-model"));

        remove_llm_session_if_owned(&first);

        assert_eq!(
            get_llm_session_for(session_id)
                .expect("replacement config")
                .model,
            "second-model"
        );

        cleanup_session(session_id);
    }
}
