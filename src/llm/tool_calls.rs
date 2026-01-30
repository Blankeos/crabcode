use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub tool_call_id: String,
    pub role: String,
    pub name: String,
    pub content: String,
}

impl ToolCall {
    pub fn parse_from_json(json_str: &str) -> Result<Vec<Self>, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    pub fn parse_from_text(text: &str) -> Option<Vec<Self>> {
        // Try JSON format first: <toolcall>[...]</toolcall>
        if let Some(start) = text.find("<toolcall>") {
            if let Some(end) = text.find("</toolcall>") {
                let json_str = &text[start + 10..end];
                return Self::parse_from_json(json_str).ok();
            }
        }

        // Try XML format: <tool_name param="value" />
        if let Some(tool_calls) = Self::parse_xml_format(text) {
            return Some(tool_calls);
        }

        // Fallback: look for JSON array in brackets
        if let Some(start) = text.find("[") {
            if let Some(end) = text.rfind("]") {
                let json_str = &text[start..=end];
                return Self::parse_from_json(json_str).ok();
            }
        }
        None
    }

    fn parse_xml_format(text: &str) -> Option<Vec<Self>> {
        let mut tool_calls = Vec::new();

        // Match patterns like: <read_file file_path="..." /> or <glob pattern="..." />
        let re = regex::Regex::new(r"<(\w+)\s+([^/>]+)(?:\s*/>|>.*?</\w+>)").ok()?;

        for cap in re.captures_iter(text) {
            let tool_name = cap.get(1)?.as_str();
            let attrs = cap.get(2)?.as_str();

            // Parse attributes into JSON object
            let mut params = serde_json::Map::new();
            let attr_re = regex::Regex::new(r#"(\w+)=["']([^"']+)["']"#).ok()?;

            for attr_cap in attr_re.captures_iter(attrs) {
                let key = attr_cap.get(1)?.as_str();
                let value = attr_cap.get(2)?.as_str();
                params.insert(
                    key.to_string(),
                    serde_json::Value::String(value.to_string()),
                );
            }

            if !params.is_empty() {
                let args = serde_json::to_string(&serde_json::Value::Object(params)).ok()?;
                tool_calls.push(ToolCall {
                    id: format!("call_{}", tool_calls.len() + 1),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: tool_name.to_string(),
                        arguments: args,
                    },
                });
            }
        }

        if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_calls() {
        let json = r#"[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"file_path\":\"/tmp/test.txt\"}"}}]"#;
        let calls = ToolCall::parse_from_json(json).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "read");
    }
}
