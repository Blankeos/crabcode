use crate::llm::ChunkSender;
use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;

pub struct QuestionTool {
    sender: Option<ChunkSender>,
}

impl QuestionTool {
    pub fn new() -> Self {
        Self { sender: None }
    }

    pub fn with_sender(mut self, sender: ChunkSender) -> Self {
        self.sender = Some(sender);
        self
    }

    pub fn with_sender_opt(mut self, sender: Option<ChunkSender>) -> Self {
        self.sender = sender;
        self
    }
}

#[async_trait]
impl ToolHandler for QuestionTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "question".to_string(),
            description: "Use this tool when you need to ask the user questions during execution. This allows you to:\n1. Gather user preferences or requirements\n2. Clarify ambiguous instructions\n3. Get decisions on implementation choices as you work\n4. Offer choices to the user about what direction to take.\n\nUsage notes:\n- Questions are answered as arrays of labels\n- You can allow multiple selections or single selection\n- Each question needs a header (short label) and options with labels and descriptions\n- When `custom` is enabled, a \"Type your own answer\" option is added automatically\n- The answers will come back as arrays of selected labels".to_string(),
            parameters: vec![ParameterSchema {
                name: "questions".to_string(),
                description: "JSON string of question objects with: question (text), header (short label), options (array of {label, description}), and optional multiple (bool)".to_string(),
                required: true,
                param_type: ParameterType::String,
            }],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["questions"])
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let questions_raw = get_string_param(&params, "questions").unwrap_or_default();

        let questions: Value = serde_json::from_str(&questions_raw).map_err(|e| {
            ToolError::Validation(format!("Invalid JSON for questions parameter: {}", e))
        })?;

        let sender = self.sender.as_ref().ok_or_else(|| {
            ToolError::Execution("Question tool has no sender configured".to_string())
        })?;

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        sender
            .send(crate::llm::ChunkMessage::QuestionRequest {
                questions: questions.clone(),
                response_tx,
            })
            .map_err(|_| {
                ToolError::Execution("Failed to deliver question request to UI".to_string())
            })?;

        if ctx.is_aborted() {
            return Err(ToolError::Execution("Cancelled".to_string()));
        }

        let response = response_rx.await.unwrap_or_else(|_| {
            serde_json::Value::String("No response from user".to_string())
        });

        let output = serde_json::to_string_pretty(&response)
            .unwrap_or_else(|_| response.to_string());

        Ok(ToolResult::new("Question answered", output).with_metadata(
            "questions",
            questions,
        ))
    }
}
