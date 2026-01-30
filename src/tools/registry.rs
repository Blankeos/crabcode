use crate::tools::types::{Tool, ToolId};
use crate::tools::ToolHandler;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<ToolId, Arc<dyn ToolHandler>>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(&self, tool: Arc<dyn ToolHandler>) {
        let definition = tool.definition();
        let mut tools = self.tools.write().await;
        tools.insert(definition.id.clone(), tool);
    }

    pub async fn get(&self, id: &str) -> Option<Arc<dyn ToolHandler>> {
        let tools = self.tools.read().await;
        tools.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<Tool> {
        let tools = self.tools.read().await;
        tools
            .values()
            .map(|t| t.definition())
            .collect()
    }

    pub async fn list_schemas(&self) -> Vec<serde_json::Value> {
        let tools = self.tools.read().await;
        tools
            .values()
            .map(|t| t.definition().to_openai_schema())
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
