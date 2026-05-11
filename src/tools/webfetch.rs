use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;

pub struct WebfetchTool;

impl WebfetchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for WebfetchTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "webfetch".to_string(),
            description: "Fetches content from a specified URL and returns it as markdown. Handles HTML to markdown conversion.\n\nUsage notes:\n- The URL must be a fully-formed valid URL\n- HTTP URLs will be automatically upgraded to HTTPS\n- Format options: \"markdown\" (default), \"text\", or \"html\"\n- Results may be summarized if the content is very large".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "url".to_string(),
                    description: "The URL to fetch content from".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "format".to_string(),
                    description: "The format to return the content in: markdown, text, or html. Defaults to markdown.".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "timeout".to_string(),
                    description: "Optional timeout in seconds (max 30)".to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["url"])?;

        let url = get_string_param(params, "url").unwrap_or_default();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::Validation(
                "URL must start with http:// or https://".to_string(),
            ));
        }

        Ok(())
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let raw_url = get_string_param(&params, "url").unwrap_or_default();
        let format = get_string_param(&params, "format").unwrap_or_else(|| "markdown".to_string());
        let timeout_secs = params
            .get("timeout")
            .and_then(|v| v.as_i64())
            .unwrap_or(30)
            .max(1)
            .min(30) as u64;

        let url = if raw_url.starts_with("http://") {
            format!("https://{}", &raw_url[7..])
        } else {
            raw_url.clone()
        };

        let client = reqwest::Client::builder()
            .user_agent("crabcode/0.1")
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| ToolError::Execution(format!("Failed to create HTTP client: {}", e)))?;

        let response = client.get(&url).send().await.map_err(|e| {
            ToolError::Execution(format!("Failed to fetch URL: {}", e))
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(ToolError::Execution(format!(
                "HTTP error: {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            )));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_lowercase();

        let body = response.text().await.map_err(|e| {
            ToolError::Execution(format!("Failed to read response body: {}", e))
        })?;

        let output = match format.as_str() {
            "html" => body,
            "text" | "markdown" => {
                if content_type.contains("html") {
                    html_to_markdown(&body)
                } else {
                    body
                }
            }
            _ => body,
        };

        let truncated = if output.len() > 100_000 {
            let boundary = output.floor_char_boundary(100_000);
            format!("{}...\n\n[Content truncated at 100KB]", &output[..boundary])
        } else {
            output
        };

        Ok(ToolResult::new(format!("Fetched: {}", url), truncated)
            .with_metadata("url", serde_json::json!(url)))
    }
}

fn html_to_markdown(html: &str) -> String {
    let mut result = String::new();
    let mut in_script = false;
    let mut in_style = false;
    let mut in_tag = false;
    let mut tag_name = String::new();
    let mut link_text = String::new();
    let mut link_href = String::new();
    let mut in_a = false;
    let mut newlines_since_text: u32 = 0;

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            tag_name.clear();
            continue;
        }

        if in_tag {
            if ch == '>' {
                in_tag = false;
                let tn = tag_name.to_lowercase();

                if tn == "script" || tn.starts_with("script ") {
                    in_script = true;
                } else if tn == "/script" {
                    in_script = false;
                } else if tn == "style" || tn.starts_with("style ") {
                    in_style = true;
                } else if tn == "/style" {
                    in_style = false;
                } else if tn == "a" || tn.starts_with("a ") {
                    in_a = true;
                    link_text.clear();
                    link_href.clear();
                    if let Some(href_start) = tn.find("href=") {
                        let after = &tn[href_start + 5..];
                        if let Some(rest) = after.strip_prefix('"').or_else(|| after.strip_prefix('\'')) {
                            if let Some(end) = rest.find('"').or_else(|| rest.find('\'')) {
                                link_href = rest[..end].to_string();
                            }
                        }
                    }
                } else if tn == "/a" {
                    if !link_text.is_empty() && !link_href.is_empty() {
                        result.push_str(&format!("[{}]({})", link_text.trim(), link_href.trim()));
                    } else {
                        result.push_str(&link_text);
                    }
                    in_a = false;
                    link_text.clear();
                } else if tn == "br" || tn == "br/" || tn == "hr" || tn == "hr/" {
                    result.push('\n');
                } else if tn == "p" || tn == "/p" || tn == "div" || tn == "/div"
                    || tn == "/h1" || tn == "/h2" || tn == "/h3" || tn == "/h4" || tn == "/h5" || tn == "/h6"
                    || tn == "/li" || tn == "/ul" || tn == "/ol" || tn == "/tr" || tn == "/blockquote"
                {
                    if !result.ends_with('\n') {
                        result.push('\n');
                    }
                    result.push('\n');
                    newlines_since_text = 2;
                } else if tn == "li" || tn.starts_with("li ") {
                    result.push_str("\n- ");
                } else if tn.starts_with("h1 ") || tn.starts_with("h2 ") || tn.starts_with("h3 ")
                    || tn.starts_with("h4 ") || tn.starts_with("h5 ") || tn.starts_with("h6 ")
                {
                    if !result.ends_with('\n') {
                        result.push('\n');
                    }
                }

                tag_name.clear();
                continue;
            }

            if ch != '/' && !tag_name.is_empty() || ch == ' ' && !tag_name.is_empty() {
                if ch == ' ' {
                    tag_name.push(' ');
                } else if ch != '/' {
                    tag_name.push(ch);
                }
            } else if ch != '/' {
                tag_name.push(ch);
            }
            continue;
        }

        if in_script || in_style {
            continue;
        }

        if in_a {
            link_text.push(ch);
            continue;
        }

        if ch.is_whitespace() {
            if !result.ends_with(' ') && newlines_since_text == 0 && !result.ends_with('\n') {
                result.push(' ');
            }
        } else {
            result.push(ch);
            newlines_since_text = 0;
        }
    }

    if in_a && !link_text.is_empty() {
        if !link_href.is_empty() {
            result.push_str(&format!("[{}]({})", link_text.trim(), link_href.trim()));
        } else {
            result.push_str(&link_text);
        }
    }

    let cleaned = result
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    let trimmed = cleaned.trim().to_string();
    let mut final_result = String::new();
    let mut blank_count = 0u32;
    for line in trimmed.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                final_result.push('\n');
            }
        } else {
            blank_count = 0;
            final_result.push_str(line);
            final_result.push('\n');
        }
    }

    final_result.trim().to_string()
}
