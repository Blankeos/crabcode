use crate::prompt::SystemPromptComposer;
use crate::session::types::{Message, MessageRole};
use crate::tools::{
    initialize_tool_registry, ToolContext, ToolError, ToolHandler, ToolRegistry, ToolResult,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::watch;

pub struct Agent {
    pub id: String,
    pub name: String,
    pub system_prompt: String,
    pub tool_registry: ToolRegistry,
}

pub struct AgentManager {
    agent: Agent,
    session_id: String,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    ToolCallStarted { tool_id: String, call_id: String },
    ToolCallCompleted { tool_id: String, call_id: String, result: ToolResult },
    ToolCallFailed { tool_id: String, call_id: String, error: String },
    Message(String),
}

impl AgentManager {
    pub async fn new(
        model_id: &str,
        working_directory: impl Into<String>,
        is_git_repo: bool,
        platform: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let tool_registry = initialize_tool_registry().await;
        
        let composer = SystemPromptComposer::new(
            model_id,
            working_directory,
            is_git_repo,
            platform,
        ).with_tool_registry(tool_registry.clone());

        let system_prompt = composer.compose().await;

        let agent = Agent {
            id: cuid2::create_id(),
            name: "default".to_string(),
            system_prompt,
            tool_registry,
        };

        Ok(Self {
            agent,
            session_id: cuid2::create_id(),
        })
    }

    pub fn get_system_prompt(&self) -> &str {
        &self.agent.system_prompt
    }

    pub fn get_tool_registry(&self) -> &ToolRegistry {
        &self.agent.tool_registry
    }

    pub async fn execute_tool(
        &self,
        tool_id: &str,
        params: serde_json::Value,
        call_id: String,
        abort_rx: watch::Receiver<bool>,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .agent
            .tool_registry
            .get(tool_id)
            .await
            .ok_or_else(|| ToolError::NotFound(format!("Tool not found: {}", tool_id)))?;

        tool.validate(&params)?;

        let ctx = ToolContext::new(
            self.session_id.clone(),
            call_id.clone(),
            self.agent.name.clone(),
            abort_rx,
        )
        .with_call_id(call_id);

        tool.execute(params, &ctx).await
    }

    pub fn create_system_message(&self,
    ) -> Message {
        Message::system(self.agent.system_prompt.clone())
    }

    pub async fn process_tool_calls(
        &self,
        tool_calls: Vec<ToolCall>,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
    ) -> Vec<ToolCallResult> {
        let mut results = Vec::new();
        let (abort_tx, abort_rx) = watch::channel(false);

        for call in tool_calls {
            let _ = event_tx.send(AgentEvent::ToolCallStarted {
                tool_id: call.tool_id.clone(),
                call_id: call.call_id.clone(),
            });

            match self
                .execute_tool(&call.tool_id,
                    call.params.clone(),
                    call.call_id.clone(),
                    abort_rx.clone(),
                )
                .await
            {
                Ok(result) => {
                    let _ = event_tx.send(AgentEvent::ToolCallCompleted {
                        tool_id: call.tool_id.clone(),
                        call_id: call.call_id.clone(),
                        result: result.clone(),
                    });
                    results.push(ToolCallResult {
                        call_id: call.call_id,
                        tool_id: call.tool_id,
                        success: true,
                        output: result.output,
                        metadata: result.metadata,
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::ToolCallFailed {
                        tool_id: call.tool_id.clone(),
                        call_id: call.call_id.clone(),
                        error: e.to_string(),
                    });
                    results.push(ToolCallResult {
                        call_id: call.call_id,
                        tool_id: call.tool_id,
                        success: false,
                        output: e.to_string(),
                        metadata: std::collections::HashMap::new(),
                    });
                }
            }
        }

        drop(abort_tx);
        results
    }
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub call_id: String,
    pub tool_id: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub call_id: String,
    pub tool_id: String,
    pub success: bool,
    pub output: String,
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_manager_creation() {
        let manager = AgentManager::new(
            "gpt-4",
            "/tmp",
            false,
            "darwin",
        ).await;

        assert!(manager.is_ok());
    }

    #[test]
    fn test_tool_call_struct() {
        let call = ToolCall {
            call_id: "test-123".to_string(),
            tool_id: "read".to_string(),
            params: serde_json::json!({"file_path": "/tmp/test.txt"}),
        };

        assert_eq!(call.tool_id, "read");
    }
}
