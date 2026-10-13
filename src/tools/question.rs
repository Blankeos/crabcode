use crate::llm::ChunkSender;
use crate::tools::{
    validate_required, ParameterSchema, ParameterType, Tool, ToolContext, ToolError, ToolHandler,
    ToolResult,
};
use async_trait::async_trait;
use serde_json::{Map, Value};
use std::collections::HashMap;

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

    let normalized = match parsed {
        Value::Array(items) if items.is_empty() => {
            return Err(ToolError::Validation(
                "questions array cannot be empty".to_string(),
            ));
        }
        Value::Array(_) => normalize_questions(parsed),
        Value::Object(_) => normalize_questions(Value::Array(vec![parsed])),
        Value::String(s) if !s.trim().is_empty() => {
            normalize_questions(question_from_plain_text(params, &s))
        }
        _ => {
            return Err(ToolError::Validation(
                "questions JSON must decode to an array or object".to_string(),
            ));
        }
    };
    validate_normalized_questions(&normalized)?;
    Ok(normalized)
}

fn validate_normalized_questions(questions: &Value) -> Result<(), ToolError> {
    let items = questions
        .as_array()
        .ok_or_else(|| ToolError::Validation("questions must normalize to an array".to_string()))?;
    for (index, item) in items.iter().enumerate() {
        let object = item.as_object().ok_or_else(|| {
            ToolError::Validation(format!("question {} must be an object", index + 1))
        })?;
        let has_prompt = ["question", "header"]
            .iter()
            .filter_map(|key| object.get(*key).and_then(Value::as_str))
            .any(|value| !value.trim().is_empty());
        if !has_prompt {
            return Err(ToolError::Validation(format!(
                "question {} must include non-empty question or header text",
                index + 1
            )));
        }
        let options = object
            .get("options")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ToolError::Validation(format!("question {} options must be an array", index + 1))
            })?;
        let mut labels = std::collections::HashSet::new();
        for (option_index, option) in options.iter().enumerate() {
            let label = option
                .get("label")
                .and_then(Value::as_str)
                .or_else(|| option.as_str())
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .ok_or_else(|| {
                    ToolError::Validation(format!(
                        "question {} option {} must include a non-empty label",
                        index + 1,
                        option_index + 1
                    ))
                })?;
            if !labels.insert(label.to_string()) {
                return Err(ToolError::Validation(format!(
                    "question {} contains duplicate option label: {label}",
                    index + 1
                )));
            }
        }
    }
    Ok(())
}

fn normalize_questions(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(idx, item)| normalize_question_item(item, idx))
                .collect(),
        ),
        other => normalize_question_item(other, 0),
    }
}

fn normalize_question_item(mut item: Value, idx: usize) -> Value {
    let should_add_options = item
        .as_object()
        .map(|obj| {
            !obj.get("options")
                .and_then(|options| options.as_array())
                .map(|options| !options.is_empty())
                .unwrap_or(false)
        })
        .unwrap_or(false);

    if !should_add_options {
        return item;
    }

    let question_text = question_text_for_model(&item, idx);
    if let Some(obj) = item.as_object_mut() {
        obj.insert(
            "options".to_string(),
            Value::Array(default_options_for_question(&question_text)),
        );
        obj.entry("custom".to_string()).or_insert(Value::Bool(true));
        obj.insert("generated_options".to_string(), Value::Bool(true));
    }

    item
}

fn option(label: &str, description: &str) -> Value {
    serde_json::json!({
        "label": label,
        "description": description,
    })
}

fn default_options_for_question(question: &str) -> Vec<Value> {
    let normalized = question.to_ascii_lowercase();

    if normalized.contains("indoor") && normalized.contains("outdoor") {
        return vec![
            option("Indoor", "Prefer hobbies done inside"),
            option("Outdoor", "Prefer hobbies outside"),
            option("Both", "Enjoy both indoor and outdoor hobbies"),
            option("Depends", "Choice depends on mood, weather, or activity"),
        ];
    }

    if normalized.contains("how much time")
        || normalized.contains("how often")
        || normalized.contains("each week")
        || normalized.contains("per week")
    {
        return vec![
            option("Less than 1 hour", "Only a small amount of time"),
            option("1-3 hours", "A few short sessions"),
            option("4-7 hours", "Several hours most weeks"),
            option("8+ hours", "A major part of the week"),
        ];
    }

    if normalized.contains("budget") || normalized.contains("cost") || normalized.contains("spend")
    {
        return vec![
            option("Free", "Prefer no-cost options"),
            option("Low budget", "Comfortable with small purchases"),
            option("Moderate", "Willing to invest occasionally"),
            option("Flexible", "Depends on the hobby"),
        ];
    }

    if starts_like_yes_no_question(&normalized) {
        return vec![
            option("Yes", "Agree or already do this"),
            option("No", "Disagree or do not do this"),
            option("Not sure", "Need more time to decide"),
            option("It depends", "Answer varies by situation"),
        ];
    }

    if normalized.contains("hobby")
        || normalized.contains("hobbies")
        || normalized.contains("free time")
        || normalized.contains("enjoy")
        || normalized.contains("try")
        || normalized.contains("rewarding")
    {
        return vec![
            option("Creative", "Art, music, writing, crafts, or making things"),
            option("Active", "Sports, fitness, movement, or physical skills"),
            option("Technology", "Coding, gaming, gadgets, or digital projects"),
            option("Outdoors", "Nature, hiking, gardening, or travel"),
            option(
                "Relaxing",
                "Reading, cooking, mindfulness, or low-key hobbies",
            ),
        ];
    }

    vec![
        option("Yes", "This fits"),
        option("No", "This does not fit"),
        option("Not sure", "Need more time to decide"),
        option("It depends", "Answer varies by situation"),
    ]
}

fn starts_like_yes_no_question(question: &str) -> bool {
    [
        "are ", "can ", "could ", "did ", "do ", "does ", "had ", "has ", "have ", "is ",
        "should ", "will ", "would ",
    ]
    .iter()
    .any(|prefix| question.starts_with(prefix))
}

fn question_text_for_model(question: &Value, idx: usize) -> String {
    if let Some(text) = question
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return text.to_string();
    }

    let Some(obj) = question.as_object() else {
        return format!("Question {}", idx + 1);
    };

    for key in ["question", "text", "prompt", "header", "title", "name"] {
        if let Some(text) = obj.get(key).and_then(|value| value.as_str()) {
            let text = text.trim();
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }

    format!("Question {}", idx + 1)
}

fn answer_for_question(response: &Value, idx: usize) -> Value {
    response
        .as_array()
        .and_then(|answers| answers.get(idx))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

fn answer_is_skipped(answer: &Value) -> bool {
    match answer {
        Value::Array(items) => items.is_empty(),
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        _ => false,
    }
}

fn question_tool_model_output(questions: &Value, response: &Value) -> Value {
    let question_items = questions
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![questions.clone()]);
    let total = question_items.len();
    let mut skipped_count = 0usize;

    let items = question_items
        .iter()
        .enumerate()
        .map(|(idx, question)| {
            let answer = answer_for_question(response, idx);
            let skipped = answer_is_skipped(&answer);
            if skipped {
                skipped_count += 1;
            }

            serde_json::json!({
                "question": question_text_for_model(question, idx),
                "answers": answer,
                "skipped": skipped,
            })
        })
        .collect::<Vec<_>>();

    let all_skipped = total > 0 && skipped_count == total;
    let message = if all_skipped {
        format!(
            "The user skipped all {} question(s). Do not call the question tool again unless the user explicitly asks to retry.",
            total
        )
    } else {
        "The user answered the question tool prompt. Continue from these answers without re-asking the same questions.".to_string()
    };

    serde_json::json!({
        "status": if all_skipped { "skipped" } else { "answered" },
        "message": message,
        "questions": items,
    })
}

fn generated_options_count(questions: &Value) -> usize {
    questions
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item.get("generated_options").and_then(|v| v.as_bool()) == Some(true)
                })
                .count()
        })
        .unwrap_or(0)
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
        let mut option_props = HashMap::new();
        option_props.insert("label".to_string(), ParameterType::String);
        option_props.insert("description".to_string(), ParameterType::String);

        let mut question_props = HashMap::new();
        question_props.insert("question".to_string(), ParameterType::String);
        question_props.insert("header".to_string(), ParameterType::String);
        question_props.insert(
            "options".to_string(),
            ParameterType::Array(Box::new(ParameterType::Object(option_props))),
        );
        question_props.insert("multiple".to_string(), ParameterType::Boolean);

        Tool {
            id: "question".to_string(),
            description: "Use this tool when you need to ask the user questions during execution. This allows you to:\n1. Gather user preferences or requirements\n2. Clarify ambiguous instructions\n3. Get decisions on implementation choices as you work\n4. Offer choices to the user about what direction to take.\n\nUsage notes:\n- Each question object must include `question` for the user-facing prompt\n- Use `header` only as a short tab label; do not put the full prompt only in `header`\n- Always include `options` with `{label, description}` items; a custom answer row is added automatically\n- If `options` is omitted or empty, Crabcode will add generic fallback options before showing the prompt\n- For select-all-that-apply questions, set `multiple: true`\n- Questions are answered as arrays of labels or custom text\n- If the result says the user skipped the questions, do not call this tool again unless the user explicitly asks to retry".to_string(),
            parameters: vec![ParameterSchema {
                name: "questions".to_string(),
                description: "Array of question objects with: question (user-facing prompt), header (short label), options (array of {label, description}), and optional multiple (bool)".to_string(),
                required: true,
                param_type: ParameterType::Array(Box::new(ParameterType::Object(question_props))),
            }],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["questions"])?;
        parse_questions_param(params).map(|_| ())
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let questions = parse_questions_param(&params)?;
        let generated_count = generated_options_count(&questions);
        if generated_count > 0 {
            crate::emit_log!(
                "[QUESTION_TOOL] added fallback options to {} optionless question(s)",
                generated_count
            );
        }

        let sender = self.sender.as_ref().ok_or_else(|| {
            ToolError::Execution("Question tool has no sender configured".to_string())
        })?;

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        sender
            .send(crate::llm::ChunkMessage::QuestionRequest {
                tool_call_id: ctx.call_id.clone(),
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

        let model_output = question_tool_model_output(&questions, &response);
        let output =
            serde_json::to_string(&model_output).unwrap_or_else(|_| model_output.to_string());

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
    fn parse_questions_adds_fallback_options_when_missing() {
        let params = json!({
            "questions": [{
                "header": "Hobby Q1",
                "question": "What's a hobby you've always wanted to try but haven't yet?"
            }]
        });

        let questions = parse_questions_param(&params).unwrap();

        assert_eq!(questions[0]["generated_options"], true);
        assert_eq!(questions[0]["options"][0]["label"], "Creative");
        assert!(questions[0]["options"].as_array().unwrap().len() >= 4);
    }

    #[test]
    fn parse_questions_preserves_explicit_options() {
        let params = json!({
            "questions": [{
                "question": "Pick one",
                "options": [{ "label": "A", "description": "First" }]
            }]
        });

        let questions = parse_questions_param(&params).unwrap();

        assert!(questions[0].get("generated_options").is_none());
        assert_eq!(questions[0]["options"][0]["label"], "A");
        assert_eq!(questions[0]["options"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn parse_questions_generates_time_options_for_time_question() {
        let params = json!({
            "questions": [{
                "question": "How much time do you typically spend on hobbies each week?"
            }]
        });

        let questions = parse_questions_param(&params).unwrap();

        assert_eq!(questions[0]["options"][0]["label"], "Less than 1 hour");
        assert_eq!(questions[0]["options"][3]["label"], "8+ hours");
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

    #[test]
    fn parse_questions_rejects_empty_or_malformed_items() {
        for params in [
            json!({ "questions": [] }),
            json!({ "questions": [null] }),
            json!({ "questions": [{ "question": "", "header": "" }] }),
            json!({
                "questions": [{
                    "question": "Pick",
                    "options": [{"label":"A"}, {"label":"A"}]
                }]
            }),
        ] {
            assert!(parse_questions_param(&params).is_err(), "{params}");
        }
    }

    #[test]
    fn model_output_includes_questions_and_answers() {
        let questions = json!([
            {
                "question": "What hobby sounds most interesting?",
                "header": "Favorite Hobby",
                "options": [{ "label": "Reading", "description": "Books" }]
            }
        ]);
        let response = json!([["Reading"]]);

        let output = question_tool_model_output(&questions, &response);

        assert_eq!(output["status"], "answered");
        assert_eq!(
            output["questions"][0]["question"],
            "What hobby sounds most interesting?"
        );
        assert_eq!(output["questions"][0]["answers"][0], "Reading");
        assert_eq!(output["questions"][0]["skipped"], false);
        assert!(output["message"]
            .as_str()
            .unwrap()
            .contains("without re-asking"));
    }

    #[test]
    fn model_output_marks_all_questions_skipped() {
        let questions = json!([
            { "header": "Hobbies Question 1", "options": [] },
            { "header": "Hobbies Question 2", "options": [] }
        ]);
        let response = json!([[], []]);

        let output = question_tool_model_output(&questions, &response);

        assert_eq!(output["status"], "skipped");
        assert_eq!(output["questions"][0]["question"], "Hobbies Question 1");
        assert_eq!(output["questions"][1]["skipped"], true);
        assert!(output["message"]
            .as_str()
            .unwrap()
            .contains("Do not call the question tool again"));
    }

    #[tokio::test]
    async fn question_request_preserves_tool_call_id_and_answers() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let tool = QuestionTool::new().with_sender(sender);
        let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        let ctx = ToolContext::new("session", "message", "Build", abort_rx)
            .with_call_id("question_call_1");
        let task = tokio::spawn(async move {
            tool.execute(
                json!({
                    "questions": [{
                        "question": "Pick one",
                        "header": "Choice",
                        "options": [{"label": "A", "description": "First"}]
                    }]
                }),
                &ctx,
            )
            .await
        });

        let Some(crate::llm::ChunkMessage::QuestionRequest {
            tool_call_id,
            questions,
            response_tx,
        }) = receiver.recv().await
        else {
            panic!("question request");
        };
        assert_eq!(tool_call_id.as_deref(), Some("question_call_1"));
        assert_eq!(questions[0]["question"], "Pick one");
        response_tx.send(json!([["A"]])).expect("question response");

        let result = task.await.expect("question task").expect("tool result");
        assert!(result.output.contains("\"status\":\"answered\""));
        assert_eq!(result.metadata["answers"], json!([["A"]]));
    }
}
