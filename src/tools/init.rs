use crate::tools::{
    fs::{GlobTool, GrepTool, ListTool, ReadTool, WriteTool},
    BashTool, EditTool, SkillTool, ToolRegistry,
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

    registry
}
