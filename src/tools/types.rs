use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type ToolId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSchema {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub param_type: ParameterType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    String,
    Integer,
    Boolean,
    Array(Box<ParameterType>),
    Object(HashMap<String, ParameterType>),
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub id: ToolId,
    pub description: String,
    pub parameters: Vec<ParameterSchema>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    pub title: String,
    pub output: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Execution error: {0}")]
    Execution(String),
    #[error("Permission denied: {0}")]
    Permission(String),
    #[error("Not found: {0}")]
    NotFound(String),
}

impl Tool {
    pub fn to_openai_schema(&self) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for param in &self.parameters {
            properties.insert(param.name.clone(), param.param_type.to_json_schema());
            if param.required {
                required.push(param.name.clone());
            }
        }

        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.id,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required
                }
            }
        })
    }
}

impl ParameterType {
    fn to_json_schema(&self) -> serde_json::Value {
        match self {
            ParameterType::String => serde_json::json!({"type": "string"}),
            ParameterType::Integer => serde_json::json!({"type": "integer"}),
            ParameterType::Boolean => serde_json::json!({"type": "boolean"}),
            ParameterType::Array(inner) => {
                serde_json::json!({
                    "type": "array",
                    "items": inner.to_json_schema()
                })
            }
            ParameterType::Object(props) => {
                let mut properties = serde_json::Map::new();
                for (key, val) in props {
                    properties.insert(key.clone(), val.to_json_schema());
                }
                serde_json::json!({
                    "type": "object",
                    "properties": properties
                })
            }
        }
    }
}

impl ToolResult {
    pub fn new(title: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            output: output.into(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}
