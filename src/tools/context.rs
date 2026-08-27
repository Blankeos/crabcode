use crate::tools::process_registry::ProcessRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ToolContext {
    pub session_id: String,
    pub message_id: String,
    pub agent: String,
    pub abort: tokio::sync::watch::Receiver<bool>,
    pub cancel_token: tokio_util::sync::CancellationToken,
    pub call_id: Option<String>,
    pub extra: Option<serde_json::Value>,
    workdir: PathBuf,
    pub process_registry: Option<Arc<ProcessRegistry>>,
}

impl ToolContext {
    pub fn new(
        session_id: impl Into<String>,
        message_id: impl Into<String>,
        agent: impl Into<String>,
        abort: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            message_id: message_id.into(),
            agent: agent.into(),
            abort,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            call_id: None,
            extra: None,
            workdir: crate::utils::cwd::current_dir_or_dot(),
            process_registry: None,
        }
    }

    pub fn from_cancel_token(
        session_id: impl Into<String>,
        message_id: impl Into<String>,
        agent: impl Into<String>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Self {
        let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        Self {
            session_id: session_id.into(),
            message_id: message_id.into(),
            agent: agent.into(),
            abort: abort_rx,
            cancel_token,
            call_id: None,
            extra: None,
            workdir: crate::utils::cwd::current_dir_or_dot(),
            process_registry: None,
        }
    }

    pub fn with_call_id(mut self, call_id: impl Into<String>) -> Self {
        self.call_id = Some(call_id.into());
        self
    }

    pub fn with_extra(mut self, extra: serde_json::Value) -> Self {
        self.extra = Some(extra);
        self
    }

    pub fn with_workdir(mut self, workdir: impl Into<PathBuf>) -> Self {
        self.workdir = workdir.into();
        self
    }

    pub fn with_process_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.process_registry = Some(registry);
        self
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn is_aborted(&self) -> bool {
        self.cancel_token.is_cancelled() || *self.abort.borrow()
    }
}
