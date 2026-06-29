use crate::tools::types::{Tool, ToolId};
use crate::tools::ToolHandler;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<ToolId, Arc<dyn ToolHandler>>>>,
    order: Arc<RwLock<Vec<ToolId>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            order: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn register(&self, tool: Arc<dyn ToolHandler>) {
        let definition = tool.definition();
        let mut tools = self.tools.write().await;
        if !tools.contains_key(&definition.id) {
            self.order.write().await.push(definition.id.clone());
        }
        tools.insert(definition.id.clone(), tool);
    }

    pub async fn get(&self, id: &str) -> Option<Arc<dyn ToolHandler>> {
        let tools = self.tools.read().await;
        tools.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<Tool> {
        let tools = self.tools.read().await;
        let order = self.order.read().await;
        order
            .iter()
            .filter_map(|id| tools.get(id))
            .map(|tool| tool.definition())
            .collect()
    }

    pub async fn list_schemas(&self) -> Vec<serde_json::Value> {
        let tools = self.tools.read().await;
        let order = self.order.read().await;
        order
            .iter()
            .filter_map(|id| tools.get(id))
            .map(|tool| tool.definition().to_openai_schema())
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{
        fs::WriteTool, ParameterSchema, ParameterType, ToolContext, ToolError, ToolResult,
    };
    use async_trait::async_trait;
    use serde_json::Value;

    struct TestTool(&'static str);

    #[async_trait]
    impl ToolHandler for TestTool {
        fn definition(&self) -> Tool {
            Tool {
                id: self.0.to_string(),
                description: "test tool".to_string(),
                parameters: vec![ParameterSchema {
                    name: "value".to_string(),
                    description: "value".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                }],
                input_schema: None,
            }
        }

        fn validate(&self, _params: &Value) -> Result<(), ToolError> {
            Ok(())
        }

        async fn execute(
            &self,
            _params: Value,
            _ctx: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::new("test", "ok"))
        }
    }

    #[tokio::test]
    async fn lists_tools_in_registration_order() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool("first"))).await;
        registry.register(Arc::new(TestTool("second"))).await;
        registry.register(Arc::new(WriteTool::new())).await;

        let ids: Vec<_> = registry
            .list()
            .await
            .into_iter()
            .map(|tool| tool.id)
            .collect();

        assert_eq!(ids, vec!["first", "second", "write"]);
    }

    #[tokio::test]
    async fn reregister_keeps_original_order() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool("first"))).await;
        registry.register(Arc::new(TestTool("second"))).await;
        registry.register(Arc::new(TestTool("first"))).await;

        let ids: Vec<_> = registry
            .list()
            .await
            .into_iter()
            .map(|tool| tool.id)
            .collect();

        assert_eq!(ids, vec!["first", "second"]);
    }
}
