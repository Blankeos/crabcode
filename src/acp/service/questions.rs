use agent_client_protocol::schema::v1::CreateElicitationRequest;
use agent_client_protocol::schema::v1::ElicitationAction;
use agent_client_protocol::schema::v1::ElicitationContentValue;
use agent_client_protocol::schema::v1::ElicitationFormMode;
use agent_client_protocol::schema::v1::ElicitationSchema;
use agent_client_protocol::schema::v1::ElicitationSessionScope;
use agent_client_protocol::schema::v1::EnumOption;
use agent_client_protocol::schema::v1::MultiSelectPropertySchema;
use agent_client_protocol::schema::v1::StringPropertySchema;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

struct AcpQuestionField {
    selection: String,
    custom: String,
    labels: HashMap<String, String>,
    multiple: bool,
}

struct AcpQuestionForm {
    request: CreateElicitationRequest,
    fields: Vec<AcpQuestionField>,
}

pub(super) fn skipped_question_answers(questions: &serde_json::Value) -> serde_json::Value {
    let count = questions.as_array().map_or(1, Vec::len);
    serde_json::Value::Array(
        (0..count)
            .map(|_| serde_json::Value::Array(Vec::new()))
            .collect(),
    )
}

fn question_text(question: &serde_json::Value, key: &str, fallback: &str) -> String {
    question
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn acp_question_form(
    session_id: &str,
    tool_call_id: Option<&str>,
    questions: &serde_json::Value,
) -> AcpQuestionForm {
    let question_items = questions
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![questions.clone()]);
    let mut schema = ElicitationSchema::new()
        .title("Agent questions")
        .description("Answer any fields you want; blank fields are treated as skipped.");
    let mut fields = Vec::with_capacity(question_items.len());

    for (question_index, question) in question_items.iter().enumerate() {
        let selection = format!("question_{question_index}");
        let custom = format!("question_{question_index}_custom");
        let prompt = question_text(question, "question", "Question");
        let header = question_text(
            question,
            "header",
            &format!("Question {}", question_index + 1),
        );
        let mut labels = HashMap::new();
        let options = question
            .get("options")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(option_index, option)| {
                let label = option
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| option.as_str())?
                    .trim();
                if label.is_empty() {
                    return None;
                }

                let value = format!("q{question_index}_option_{option_index}");
                labels.insert(value.clone(), label.to_string());
                let description = option
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let mut option = EnumOption::new(value, label);
                if let Some(description) = description {
                    option = option.description(description);
                }
                Some(option)
            })
            .collect::<Vec<_>>();
        let multiple = question
            .get("multiple")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        if multiple {
            schema = schema.property(
                selection.clone(),
                MultiSelectPropertySchema::titled(options)
                    .title(header.clone())
                    .description(prompt.clone()),
                false,
            );
        } else {
            schema = schema.property(
                selection.clone(),
                StringPropertySchema::new()
                    .title(header.clone())
                    .description(prompt.clone())
                    .one_of(options),
                false,
            );
        }
        schema = schema.property(
            custom.clone(),
            StringPropertySchema::new()
                .title(format!("{header}: custom answer"))
                .description("Optional free-form answer."),
            false,
        );
        fields.push(AcpQuestionField {
            selection,
            custom,
            labels,
            multiple,
        });
    }

    let scope = ElicitationSessionScope::new(session_id.to_string())
        .tool_call_id(tool_call_id.map(agent_client_protocol::schema::v1::ToolCallId::new));
    let request = CreateElicitationRequest::new(
        ElicitationFormMode::new(scope, schema),
        "The agent needs additional input to continue.",
    );
    AcpQuestionForm { request, fields }
}

fn acp_question_answers(
    fields: &[AcpQuestionField],
    action: ElicitationAction,
) -> serde_json::Value {
    let ElicitationAction::Accept(accepted) = action else {
        return serde_json::Value::Array(
            fields
                .iter()
                .map(|_| serde_json::Value::Array(Vec::new()))
                .collect(),
        );
    };
    let content = accepted.content.unwrap_or_default();
    serde_json::Value::Array(
        fields
            .iter()
            .map(|field| {
                let mut answers = Vec::new();
                match content.get(&field.selection) {
                    Some(ElicitationContentValue::String(value)) if !field.multiple => {
                        if let Some(label) = field.labels.get(value) {
                            answers.push(serde_json::Value::String(label.clone()));
                        }
                    }
                    Some(ElicitationContentValue::StringArray(values)) if field.multiple => {
                        for value in values {
                            if let Some(label) = field.labels.get(value) {
                                if !answers.iter().any(|answer| answer.as_str() == Some(label)) {
                                    answers.push(serde_json::Value::String(label.clone()));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(ElicitationContentValue::String(custom)) = content.get(&field.custom) {
                    let custom = custom.trim();
                    if !custom.is_empty() {
                        if !field.multiple {
                            answers.clear();
                        }
                        answers.push(serde_json::Value::String(
                            custom.chars().take(8_192).collect(),
                        ));
                    }
                }
                serde_json::Value::Array(answers)
            })
            .collect(),
    )
}

pub(super) async fn request_questions(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    tool_call_id: Option<&str>,
    questions: &serde_json::Value,
    cancellation: &CancellationToken,
) -> serde_json::Value {
    let form = acp_question_form(session_id, tool_call_id, questions);
    let request = connection.send_request(form.request).block_task();
    tokio::pin!(request);
    let response = tokio::select! {
        _ = cancellation.cancelled() => return skipped_question_answers(questions),
        response = &mut request => response,
    };
    let Ok(response) = response else {
        return skipped_question_answers(questions);
    };
    acp_question_answers(&form.fields, response.action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ElicitationAction;
    use agent_client_protocol::schema::v1::ElicitationContentValue;

    #[test]
    fn acp_question_form_preserves_single_multi_custom_and_scope() {
        let form = acp_question_form(
            "session_1",
            Some("question_call_1"),
            &serde_json::json!([
                {
                    "question": "Pick one",
                    "header": "Single",
                    "options": [
                        {"label": "A", "description": "First"},
                        {"label": "B", "description": "Second"}
                    ]
                },
                {
                    "question": "Pick several",
                    "header": "Multiple",
                    "multiple": true,
                    "options": [
                        {"label": "X", "description": "First"},
                        {"label": "Y", "description": "Second"}
                    ]
                }
            ]),
        );
        let wire = serde_json::to_value(&form.request).expect("elicitation request");

        assert_eq!(wire["mode"], "form");
        assert_eq!(wire["sessionId"], "session_1");
        assert_eq!(wire["toolCallId"], "question_call_1");
        assert_eq!(
            wire["requestedSchema"]["properties"]["question_0"]["type"],
            "string"
        );
        assert_eq!(
            wire["requestedSchema"]["properties"]["question_0"]["oneOf"][0]["title"],
            "A"
        );
        assert_eq!(
            wire["requestedSchema"]["properties"]["question_1"]["type"],
            "array"
        );
        assert_eq!(
            wire["requestedSchema"]["properties"]["question_1_custom"]["type"],
            "string"
        );
    }

    #[test]
    fn acp_question_answers_restore_labels_and_custom_text() {
        let form = acp_question_form(
            "session_1",
            None,
            &serde_json::json!([
                {
                    "question": "Pick one",
                    "options": [{"label": "A"}, {"label": "B"}]
                },
                {
                    "question": "Pick several",
                    "multiple": true,
                    "options": [{"label": "X"}, {"label": "Y"}]
                }
            ]),
        );
        let mut content = std::collections::BTreeMap::new();
        content.insert(
            "question_0".to_string(),
            ElicitationContentValue::String("q0_option_1".to_string()),
        );
        content.insert(
            "question_1".to_string(),
            ElicitationContentValue::StringArray(vec![
                "q1_option_0".to_string(),
                "q1_option_1".to_string(),
            ]),
        );
        content.insert(
            "question_1_custom".to_string(),
            ElicitationContentValue::String("Other choice".to_string()),
        );
        let action = ElicitationAction::Accept(
            agent_client_protocol::schema::v1::ElicitationAcceptAction::new().content(content),
        );

        assert_eq!(
            acp_question_answers(&form.fields, action),
            serde_json::json!([["B"], ["X", "Y", "Other choice"]])
        );
        assert_eq!(
            acp_question_answers(&form.fields, ElicitationAction::Cancel),
            serde_json::json!([[], []])
        );

        let mut custom_content = std::collections::BTreeMap::new();
        custom_content.insert(
            "question_0".to_string(),
            ElicitationContentValue::String("q0_option_0".to_string()),
        );
        custom_content.insert(
            "question_0_custom".to_string(),
            ElicitationContentValue::String("Custom only".to_string()),
        );
        let custom_action = ElicitationAction::Accept(
            agent_client_protocol::schema::v1::ElicitationAcceptAction::new()
                .content(custom_content),
        );
        assert_eq!(
            acp_question_answers(&form.fields, custom_action),
            serde_json::json!([["Custom only"], []])
        );
    }
}
