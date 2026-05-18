use crate::tools::{
    fs::{GlobTool, GrepTool, ListTool, ReadTool, WriteTool},
    BashTool, EditTool, QuestionTool, SkillTool, TaskTool, TodowriteTool, ToolRegistry,
    WebfetchTool,
};
use std::sync::Arc;

pub async fn initialize_tool_registry() -> ToolRegistry {
    let registry = ToolRegistry::new();

    registry.register(Arc::new(GlobTool::new())).await;
    registry.register(Arc::new(GrepTool::new())).await;
    registry.register(Arc::new(ListTool::new())).await;
    registry.register(Arc::new(ReadTool::new())).await;
    registry.register(Arc::new(WriteTool::new())).await;
    registry.register(Arc::new(BashTool::new())).await;
    registry.register(Arc::new(EditTool::new())).await;
    registry.register(Arc::new(SkillTool::new())).await;
    registry.register(Arc::new(WebfetchTool::new())).await;
    registry.register(Arc::new(TodowriteTool::new())).await;

    registry
}

pub async fn register_dynamic_tools(
    registry: &ToolRegistry,
    sender: Option<crate::llm::ChunkSender>,
) {
    registry
        .register(Arc::new(QuestionTool::new().with_sender_opt(sender.clone())))
        .await;

    registry
        .register(Arc::new(TaskTool::new(registry.clone()).with_sender_opt(sender)))
        .await;
}
