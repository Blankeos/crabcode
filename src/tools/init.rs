use crate::tools::{
    fs::{GlobTool, GrepTool, ListTool, ReadTool, ViewImageTool, WriteFilesTool, WriteTool},
    ApplyPatchTool, BashTool, EditTool, QuestionTool, SkillTool, TaskTool, ToolPermissions,
    ToolRegistry, UpdatePlanTool, WebfetchTool, WebsearchTool,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub async fn initialize_tool_registry() -> ToolRegistry {
    initialize_tool_registry_with_config(
        None,
        &crate::config::configuration::WebsearchConfig::default(),
    )
    .await
}

pub async fn initialize_tool_registry_with_config(
    provider_name: Option<&str>,
    websearch_config: &crate::config::configuration::WebsearchConfig,
) -> ToolRegistry {
    let registry = ToolRegistry::new();

    registry.register(Arc::new(GlobTool::new())).await;
    registry.register(Arc::new(GrepTool::new())).await;
    registry.register(Arc::new(ListTool::new())).await;
    registry.register(Arc::new(ReadTool::new())).await;
    registry.register(Arc::new(ViewImageTool::new())).await;
    registry.register(Arc::new(ApplyPatchTool::new())).await;
    registry.register(Arc::new(WriteTool::new())).await;
    registry.register(Arc::new(WriteFilesTool::new())).await;
    registry.register(Arc::new(BashTool::new())).await;
    registry.register(Arc::new(EditTool::new())).await;
    registry.register(Arc::new(SkillTool::new())).await;
    registry.register(Arc::new(WebfetchTool::new())).await;
    if WebsearchTool::is_enabled_for_provider(provider_name.unwrap_or_default(), websearch_config) {
        registry
            .register(Arc::new(WebsearchTool::new(websearch_config.clone())))
            .await;
    }
    registry.register(Arc::new(UpdatePlanTool::new())).await;

    registry
}

pub async fn register_dynamic_tools(
    registry: &ToolRegistry,
    sender: Option<crate::llm::ChunkSender>,
    permissions: ToolPermissions,
    agent_registry: crate::agent::definition::AgentRegistry,
    cancel_token: CancellationToken,
) {
    registry
        .register(Arc::new(
            QuestionTool::new().with_sender_opt(sender.clone()),
        ))
        .await;

    registry
        .register(Arc::new(
            TaskTool::new(registry.clone())
                .with_sender_opt(sender)
                .with_runtime_options(permissions, agent_registry, cancel_token),
        ))
        .await;
}

pub async fn initialize_tool_registry_with_dynamic(
    sender: Option<crate::llm::ChunkSender>,
    permissions: ToolPermissions,
    agent_registry: crate::agent::definition::AgentRegistry,
    cancel_token: CancellationToken,
) -> ToolRegistry {
    let registry = initialize_tool_registry().await;
    register_dynamic_tools(&registry, sender, permissions, agent_registry, cancel_token).await;
    registry
}

pub async fn initialize_tool_registry_with_dynamic_config(
    sender: Option<crate::llm::ChunkSender>,
    permissions: ToolPermissions,
    agent_registry: crate::agent::definition::AgentRegistry,
    cancel_token: CancellationToken,
    provider_name: Option<&str>,
    websearch_config: &crate::config::configuration::WebsearchConfig,
) -> ToolRegistry {
    let registry = initialize_tool_registry_with_config(provider_name, websearch_config).await;
    register_dynamic_tools(&registry, sender, permissions, agent_registry, cancel_token).await;
    registry
}

pub async fn scope_tool_registry_for_agent(
    registry: &ToolRegistry,
    permissions: &ToolPermissions,
    agent_mode: &str,
) -> ToolRegistry {
    let scoped = ToolRegistry::new();
    for tool in registry.list().await {
        if permissions.is_tool_visible_for_agent(agent_mode, &tool.id) {
            if let Some(handler) = registry.get(&tool.id).await {
                scoped.register(handler).await;
            }
        }
    }
    scoped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dynamic_registry_contains_runtime_tools() {
        let registry = initialize_tool_registry_with_dynamic(
            None,
            ToolPermissions::new("."),
            crate::agent::definition::AgentRegistry::default(),
            CancellationToken::new(),
        )
        .await;

        assert!(registry.get("question").await.is_some());
        assert!(registry.get("task").await.is_some());
    }

    #[tokio::test]
    async fn scoped_plan_registry_hides_mutating_tools() {
        let permissions = ToolPermissions::new(".");
        let registry = initialize_tool_registry_with_dynamic(
            None,
            permissions.clone(),
            crate::agent::definition::AgentRegistry::default(),
            CancellationToken::new(),
        )
        .await;
        let scoped = scope_tool_registry_for_agent(&registry, &permissions, "plan").await;

        assert!(scoped.get("read").await.is_some());
        assert!(scoped.get("task").await.is_some());
        assert!(scoped.get("bash").await.is_none());
        assert!(scoped.get("apply_patch").await.is_none());
        assert!(scoped.get("write").await.is_none());
        assert!(scoped.get("edit").await.is_none());
    }
}
