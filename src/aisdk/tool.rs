use crate::message::ImageContent;
use schemars::Schema;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type AsyncToolFn = Arc<
    dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Result<ToolOutput, String>> + Send>>
        + Send
        + Sync,
>;

#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<ImageContent>,
}

#[cfg(test)]
mod tests {
    use super::openai_compatible_input_schema;
    use schemars::Schema;
    use serde_json::json;

    fn schema(value: serde_json::Value) -> Schema {
        serde_json::from_value(value).expect("valid schema")
    }

    #[test]
    fn strips_nested_regex_lookaround_from_openai_tool_schema() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "object",
                    "properties": {
                        "email": {
                            "type": "string",
                            "description": "Contact email.",
                            "pattern": "^(?!\\.)(?!.*\\.\\.)[^@]+@[^@]+$"
                        }
                    }
                },
                "slug": { "type": "string", "pattern": "^[a-z0-9-]+$" }
            }
        }));

        let sanitized = openai_compatible_input_schema(&input);
        assert!(sanitized
            .pointer("/properties/data/properties/email/pattern")
            .is_none());
        assert_eq!(
            sanitized.pointer("/properties/data/properties/email/description"),
            Some(&json!(
                "Contact email. Validation for this value is enforced by the tool."
            ))
        );
        assert_eq!(
            sanitized.pointer("/properties/slug/pattern"),
            Some(&json!("^[a-z0-9-]+$"))
        );
    }

    #[test]
    fn strips_all_lookaround_forms_from_schema_branches() {
        for pattern in ["a(?=b)", "a(?!b)", "(?<=a)b", "(?<!a)b"] {
            let input = schema(json!({
                "type": "array",
                "items": { "type": "string", "pattern": pattern }
            }));
            let sanitized = openai_compatible_input_schema(&input);
            assert!(sanitized.pointer("/items/pattern").is_none(), "{pattern}");
        }
    }

    #[test]
    fn preserves_literal_pattern_properties_and_escaped_tokens() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "literal": {
                    "type": "object",
                    "default": { "pattern": "(?=do-not-touch)" },
                    "enum": [{ "pattern": "(?=do-not-touch)" }]
                },
                "escaped": { "type": "string", "pattern": "^\\\\(\\\\?=value$" },
                "class": { "type": "string", "pattern": "[(?=]+" }
            }
        }));

        assert_eq!(
            openai_compatible_input_schema(&input),
            serde_json::to_value(input).unwrap()
        );
    }
}

/// Return a model-facing tool schema compatible with OpenAI's JSON Schema
/// validator. MCP servers can emit ECMAScript patterns containing lookahead or
/// lookbehind assertions, which OpenAI rejects before the model runs. The MCP
/// server remains the authoritative validator, so unsupported pattern
/// constraints are removed only from the provider-facing copy.
pub(crate) fn openai_compatible_input_schema(schema: &Schema) -> serde_json::Value {
    let mut schema = serde_json::to_value(schema).unwrap_or_default();
    sanitize_openai_schema_node(&mut schema);
    schema
}

fn sanitize_openai_schema_node(node: &mut serde_json::Value) {
    match node {
        serde_json::Value::Object(object) => {
            if object
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .is_some_and(contains_regex_lookaround)
            {
                object.remove("pattern");
                const NOTE: &str = "Validation for this value is enforced by the tool.";
                let description = object
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(|description| format!("{description} {NOTE}"))
                    .unwrap_or_else(|| NOTE.to_string());
                object.insert(
                    "description".to_string(),
                    serde_json::Value::String(description),
                );
            }

            // Visit schema-valued keywords only. Literal values under `const`,
            // `default`, `enum`, `examples`, or extension metadata must remain
            // untouched even when they contain a property named `pattern`.
            for (keyword, value) in object.iter_mut() {
                match keyword.as_str() {
                    "properties" | "patternProperties" | "$defs" | "definitions"
                    | "dependentSchemas" | "dependencies" => {
                        if let Some(schemas) = value.as_object_mut() {
                            for schema in schemas.values_mut() {
                                sanitize_openai_schema_node(schema);
                            }
                        }
                    }
                    "items"
                    | "additionalItems"
                    | "additionalProperties"
                    | "contains"
                    | "propertyNames"
                    | "not"
                    | "if"
                    | "then"
                    | "else"
                    | "unevaluatedProperties"
                    | "unevaluatedItems"
                    | "contentSchema"
                    | "allOf"
                    | "anyOf"
                    | "oneOf"
                    | "prefixItems" => sanitize_openai_schema_node(value),
                    _ => {}
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                sanitize_openai_schema_node(item);
            }
        }
        _ => {}
    }
}

fn contains_regex_lookaround(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut escaped = false;
    let mut in_character_class = false;
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'[' if !in_character_class => in_character_class = true,
            b']' if in_character_class => in_character_class = false,
            b'(' if !in_character_class && bytes.get(index + 1) == Some(&b'?') => {
                let forward = matches!(bytes.get(index + 2), Some(b'=') | Some(b'!'));
                let backward = matches!(
                    (bytes.get(index + 2), bytes.get(index + 3)),
                    (Some(b'<'), Some(b'=' | b'!'))
                );
                if forward || backward {
                    return true;
                }
            }
            _ => {}
        }
        index += 1;
    }
    false
}

impl ToolOutput {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    pub fn with_images(mut self, images: Vec<ImageContent>) -> Self {
        self.images = images;
        self
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.images.is_empty()
    }
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for ToolOutput {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

#[derive(Clone)]
pub struct ToolExecute {
    inner: AsyncToolFn,
}

impl ToolExecute {
    pub fn new<F, Fut, O>(f: F) -> Self
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, String>> + Send + 'static,
        O: Into<ToolOutput> + Send + 'static,
    {
        Self {
            inner: Arc::new(move |v: serde_json::Value| {
                let fut = f(v);
                Box::pin(async move { fut.await.map(Into::into) })
            }),
        }
    }

    pub async fn call(&self, input: serde_json::Value) -> Result<ToolOutput, String> {
        (self.inner)(input).await
    }
}

impl std::fmt::Debug for ToolExecute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecute").finish()
    }
}

/// How a tool is exposed on the wire.
///
/// Client tools become provider `function` tools and run locally via [`Tool::execute`].
/// Provider-executed tools carry a native request fragment (or OpenRouter plugin)
/// and are run by the model provider — same call-site shape as Rig / Vercel AI SDK.
#[derive(Debug, Clone, Default)]
pub enum ToolTransport {
    #[default]
    ClientFunction,
    /// Native tool object, e.g. `{ "type": "web_search" }` or Anthropic hosted tools.
    ProviderNative(serde_json::Value),
    /// OpenRouter `plugins` entry, e.g. `{ "id": "web" }`.
    OpenRouterPlugin(serde_json::Value),
}

#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Schema,
    pub execute: ToolExecute,
    pub transport: ToolTransport,
}

impl std::fmt::Debug for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tool")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("transport", &self.transport)
            .finish()
    }
}

impl Tool {
    pub fn builder() -> ToolBuilder {
        ToolBuilder::default()
    }

    pub fn is_provider_executed(&self) -> bool {
        !matches!(self.transport, ToolTransport::ClientFunction)
    }
}

#[derive(Default)]
pub struct ToolBuilder {
    name: Option<String>,
    description: Option<String>,
    input_schema: Option<Schema>,
    execute: Option<ToolExecute>,
    transport: ToolTransport,
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

    pub fn transport(mut self, transport: ToolTransport) -> Self {
        self.transport = transport;
        self
    }

    pub fn build(self) -> Result<Tool, String> {
        Ok(Tool {
            name: self.name.ok_or("name is required")?,
            description: self.description.ok_or("description is required")?,
            input_schema: self.input_schema.ok_or("input_schema is required")?,
            execute: self.execute.ok_or("execute is required")?,
            transport: self.transport,
        })
    }
}
