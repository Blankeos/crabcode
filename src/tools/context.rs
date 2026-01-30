pub struct ToolContext {
    pub session_id: String,
    pub message_id: String,
    pub agent: String,
    pub abort: tokio::sync::watch::Receiver<bool>,
    pub call_id: Option<String>,
    pub extra: Option<serde_json::Value>,
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
            call_id: None,
            extra: None,
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

    pub fn is_aborted(&self) -> bool {
        *self.abort.borrow()
    }
}
