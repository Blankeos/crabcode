use crate::llm::ChunkSender;
use crate::tools::{
    validate_required, ParameterSchema, ParameterType, Tool, ToolContext, ToolError, ToolHandler,
    ToolResult,
};
use async_trait::async_trait;
use serde_json::{Map, Value};

fn question_from_plain_text(params: &Value, question: &str) -> Value {
    let mut item = Map::new();
    item.insert("question".to_string(), Value::String(question.to_string()));

    let header = params
        .get("header")
        .and_then(|v| v.as_str())
        .unwrap_or("Question");
    item.insert("header".to_string(), Value::String(header.to_string()));

    for key in [
        "options",
        "custom",
        "multiple",
        "allow_multiple",
        "allowMultiple",
        "multi",
        "multiselect",
        "multi_select",
        "multipleChoice",
        "multiple_choice",
        "type",
        "kind",
        "mode",
        "selection",
        "selection_type",
        "allow_random_order",
    ] {
        if let Some(value) = params.get(key) {
            item.insert(key.to_string(), value.clone());
        }
    }

    Value::Array(vec![Value::Object(item)])
}

fn parse_questions_string(params: &Value, raw: &str) -> Result<Value, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::Validation(
            "questions parameter cannot be empty".to_string(),
        ));
    }

    if !trimmed.starts_with('{') && !trimmed.starts_with('[') && !trimmed.starts_with('"') {
        return Ok(question_from_plain_text(params, trimmed));
    }

    serde_json::from_str::<Value>(trimmed)
        .map_err(|e| ToolError::Validation(format!("Invalid JSON for questions parameter: {}", e)))
}

fn parse_questions_param(params: &Value) -> Result<Value, ToolError> {
    let raw = params.get("questions").ok_or_else(|| {
        ToolError::Validation("Missing required parameter: questions".to_string())
    })?;

    let parsed = match raw {
        Value::String(s) => parse_questions_string(params, s)?,
        Value::Array(_) | Value::Object(_) => raw.clone(),
        _ => {
            return Err(ToolError::Validation(
                "questions parameter must be an array, object, or JSON string".to_string(),
            ));
        }
    };

    match parsed {
        Value::Array(_) => Ok(parsed),
        Value::Object(_) => Ok(Value::Array(vec![parsed])),
        Value::String(s) if !s.trim().is_empty() => Ok(question_from_plain_text(params, &s)),
        _ => Err(ToolError::Validation(
            "questions JSON must decode to an array or object".to_string(),
        )),
    }
}

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
            description: "Use this tool when you need to ask the user questions during execution. This allows you to:\n1. Gather user preferences or requirements\n2. Clarify ambiguous instructions\n3. Get decisions on implementation choices as you work\n4. Offer choices to the user about what direction to take.\n\nUsage notes:\n- Questions are answered as arrays of labels\n- You can allow multiple selections or single selection\n- For select-all-that-apply questions, set `multiple: true`\n- Each question needs a header (short label) and options with labels and descriptions\n- A \"Type your own answer\" option is always available for option questions\n- The answers will come back as arrays of selected labels or custom answers".to_string(),
            parameters: vec![ParameterSchema {
                name: "questions".to_string(),
                description: "Array of question objects with: question (text), header (short label), options (array of {label, description}), and optional multiple (bool)".to_string(),
                required: true,
                param_type: ParameterType::Array(Box::new(ParameterType::Object(Default::default()))),
            }],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["questions"])?;
        parse_questions_param(params).map(|_| ())
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let questions = parse_questions_param(&params)?;

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

        let response = response_rx
            .await
            .unwrap_or_else(|_| serde_json::Value::String("No response from user".to_string()));

        let output =
            serde_json::to_string_pretty(&response).unwrap_or_else(|_| response.to_string());

        Ok(ToolResult::new("Question answered", output)
            .with_metadata("questions", questions)
            .with_metadata("answers", response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_questions_accepts_structured_array() {
        let params = json!({
            "questions": [{
                "question": "Pick an option",
                "header": "Choice",
                "options": [{ "label": "A", "description": "First" }]
            }]
        });

        let questions = parse_questions_param(&params).unwrap();

        assert!(questions.is_array());
        assert_eq!(questions[0]["question"], "Pick an option");
    }

    #[test]
    fn parse_questions_accepts_json_string() {
        let params = json!({
            "questions": r#"[{"question":"Pick","header":"Choice","options":[]}]"#
        });

        let questions = parse_questions_param(&params).unwrap();

        assert!(questions.is_array());
        assert_eq!(questions[0]["header"], "Choice");
    }

    #[test]
    fn parse_questions_accepts_plain_text_with_top_level_options() {
        let params = json!({
            "questions": "What should the table contain?",
            "options": [
                { "label": "Stats", "description": "Show project stats" },
                { "label": "Files", "description": "Show file list" }
            ],
            "custom": true
        });

        let questions = parse_questions_param(&params).unwrap();

        assert!(questions.is_array());
        assert_eq!(questions[0]["question"], "What should the table contain?");
        assert_eq!(questions[0]["header"], "Question");
        assert_eq!(questions[0]["options"][0]["label"], "Stats");
        assert_eq!(questions[0]["custom"], true);
    }

    #[test]
    fn parse_questions_accepts_json_encoded_plain_text() {
        let params = json!({ "questions": r#""Pick one""# });

        let questions = parse_questions_param(&params).unwrap();

        assert!(questions.is_array());
        assert_eq!(questions[0]["question"], "Pick one");
    }

    #[test]
    fn parse_questions_wraps_single_object() {
        let params = json!({
            "questions": {
                "question": "Pick",
                "header": "Choice",
                "options": []
            }
        });

        let questions = parse_questions_param(&params).unwrap();

        assert!(questions.is_array());
        assert_eq!(questions.as_array().unwrap().len(), 1);
        assert_eq!(questions[0]["question"], "Pick");
    }

    #[test]
    fn parse_questions_rejects_empty_string() {
        let params = json!({ "questions": "" });

        let err = parse_questions_param(&params).unwrap_err().to_string();

        assert!(err.contains("questions parameter cannot be empty"));
    }
}
