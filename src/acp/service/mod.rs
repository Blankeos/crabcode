mod attachments;
mod commands;
mod compaction;
mod config;
mod notifications;
mod permissions;
mod prompt;
mod questions;
mod sessions;
mod stream;
mod terminals;
mod usage;

#[cfg(test)]
mod test_support;

use crate::config::configuration::LoadedConfig;
use crate::session::manager::SessionManager;
use agent_client_protocol::Error;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AcpService {
    sessions: Arc<AsyncMutex<AcpSessions>>,
    session_manager: Arc<Mutex<SessionManager>>,
    client_capabilities: Arc<Mutex<agent_client_protocol::schema::v1::ClientCapabilities>>,
}

// Attachments detach immediately on close, but a turn retains ownership until all
// transcript/status finalization finishes. Both are protected by the same lock.
#[derive(Default)]
struct AcpSessions {
    attachments: HashMap<String, AcpSession>,
    active_prompts: HashSet<String>,
}

impl AcpSessions {
    fn ensure_attachable(&self, session_id: &str) -> Result<(), Error> {
        if self.active_prompts.contains(session_id) {
            return Err(Error::invalid_params().data("session already has an active prompt"));
        }
        Ok(())
    }

    fn begin_prompt(&mut self, session_id: &str) -> Result<CancellationToken, Error> {
        self.ensure_attachable(session_id)?;
        let session = self
            .attachments
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let cancellation = CancellationToken::new();
        session.cancellation = Some(cancellation.clone());
        self.active_prompts.insert(session_id.to_string());
        Ok(cancellation)
    }

    // Call only after the stream and wrapper have finished every persisted write.
    fn finish_prompt(&mut self, session_id: &str) {
        if let Some(session) = self.attachments.get_mut(session_id) {
            session.cancellation = None;
        }
        self.active_prompts.remove(session_id);
    }
}

async fn cancellable<T>(
    token: &CancellationToken,
    future: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = token.cancelled() => None,
        result = future => Some(result),
    }
}

#[derive(Clone)]
struct AcpSession {
    cwd: PathBuf,
    config: LoadedConfig,
    tool_permissions: crate::tools::ToolPermissions,
    skills: crate::skill::SkillStore,
    models: Vec<crate::model::types::Model>,
    provider: String,
    model: String,
    agent: String,
    reasoning_selection: crate::model::reasoning::ReasoningEffort,
    reasoning: Option<crate::model::reasoning::ReasoningEffort>,
    context_window: Option<u32>,
    cancellation: Option<CancellationToken>,
}

fn internal_error() -> Error {
    Error::internal_error().data("Crabcode ACP operation failed")
}

fn internal_error_with(error: &str) -> Error {
    Error::internal_error().data(format!("Crabcode ACP operation failed: {error}"))
}

impl AcpService {
    pub fn new(initial_workspace: &Path) -> Result<Self, Error> {
        let session_manager = SessionManager::new()
            .with_history_for_workspace(initial_workspace)
            .map_err(|_| internal_error())?;
        Ok(Self {
            sessions: Arc::new(AsyncMutex::new(AcpSessions::default())),
            session_manager: Arc::new(Mutex::new(session_manager)),
            client_capabilities: Arc::new(Mutex::new(Default::default())),
        })
    }

    pub fn set_client_capabilities(
        &self,
        capabilities: agent_client_protocol::schema::v1::ClientCapabilities,
    ) {
        if let Ok(mut current) = self.client_capabilities.lock() {
            *current = capabilities;
        }
    }

    fn supports_form_elicitation(&self) -> bool {
        self.client_capabilities
            .lock()
            .ok()
            .and_then(|capabilities| capabilities.elicitation.clone())
            .and_then(|elicitation| elicitation.form)
            .is_some()
    }

    fn supports_terminals(&self) -> bool {
        self.client_capabilities
            .lock()
            .ok()
            .is_some_and(|capabilities| capabilities.terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn cancellation_drops_pending_generation_without_waiting() {
        struct OnDrop(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let token = CancellationToken::new();
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = OnDrop(dropped.clone());
        let future = async move {
            let _guard = guard;
            std::future::pending::<String>().await
        };
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        });
        assert!(tokio::time::timeout(
            std::time::Duration::from_secs(1),
            cancellable(&token, future)
        )
        .await
        .unwrap()
        .is_none());
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancellation_wins_over_already_ready_answer() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(cancellable(&token, std::future::ready("answer"))
            .await
            .is_none());
    }
}
