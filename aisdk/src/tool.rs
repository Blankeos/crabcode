use schemars::Schema;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type AsyncToolFn =
    Arc<dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync>;

#[derive(Clone)]
pub struct ToolExecute {
    inner: AsyncToolFn,
}

impl ToolExecute {
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, String>> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |v: serde_json::Value| Box::pin(f(v))),
        }
    }

    pub async fn call(&self, input: serde_json::Value) -> Result<String, String> {
        (self.inner)(input).await
    }
}

impl std::fmt::Debug for ToolExecute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecute").finish()
    }
}

#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Schema,
    pub execute: ToolExecute,
}

impl std::fmt::Debug for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tool")
            .field("name", &self.name)
            .field("description", &self.description)
            .finish()
    }
}

impl Tool {
    pub fn builder() -> ToolBuilder {
        ToolBuilder::default()
    }
}

#[derive(Default)]
pub struct ToolBuilder {
    name: Option<String>,
    description: Option<String>,
    input_schema: Option<Schema>,
    execute: Option<ToolExecute>,
}

impl ToolBuilder {
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn input_schema(mut self, schema: Schema) -> Self {
        self.input_schema = Some(schema);
        self
    }

    pub fn execute(mut self, execute: ToolExecute) -> Self {
        self.execute = Some(execute);
        self
    }

    pub fn build(self) -> Result<Tool, String> {
        Ok(Tool {
            name: self.name.ok_or("name is required")?,
            description: self.description.ok_or("description is required")?,
            input_schema: self.input_schema.ok_or("input_schema is required")?,
            execute: self.execute.ok_or("execute is required")?,
        })
    }
}
