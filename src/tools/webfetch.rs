use crate::tools::{
    get_integer_param, get_string_param, validate_required, ParameterSchema, ParameterType, Tool,
    ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{
    header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT},
    Response, StatusCode,
};
use serde_json::Value;

pub struct WebfetchTool;

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const HONEST_USER_AGENT: &str = "crabcode/0.1";

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
            description: "Fetches content from a specified URL and returns it as markdown. Handles HTML to markdown conversion.\n\nUsage notes:\n- The URL must be a fully-formed valid URL\n- HTTP URLs will be automatically upgraded to HTTPS, except localhost and loopback URLs\n- Format options: \"markdown\" (default), \"text\", or \"html\"\n- Results may be summarized if the content is very large".to_string(),
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
                    description: "Optional timeout in seconds (max 120)".to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
            ],
            input_schema: None,
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

        if let Some(format) = get_string_param(params, "format") {
            if !matches!(format.as_str(), "markdown" | "text" | "html") {
                return Err(ToolError::Validation(
                    "Format must be one of: markdown, text, html".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let raw_url = get_string_param(&params, "url").unwrap_or_default();
        let format = get_string_param(&params, "format").unwrap_or_else(|| "markdown".to_string());
        let timeout_secs = get_integer_param(&params, "timeout")
            .unwrap_or(DEFAULT_TIMEOUT_SECS as i64)
            .max(1)
            .min(MAX_TIMEOUT_SECS as i64) as u64;

        let url = fetch_url_for(&raw_url);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| ToolError::Execution(format!("Failed to create HTTP client: {}", e)))?;

        let mut response = send_request(&client, &url, &format, BROWSER_USER_AGENT)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to fetch URL: {}", e)))?;

        if is_cloudflare_challenge(&response) {
            response = send_request(&client, &url, &format, HONEST_USER_AGENT)
                .await
                .map_err(|e| ToolError::Execution(format!("Failed to fetch URL: {}", e)))?;
        }

        let status = response.status();
        if !status.is_success() {
            return Err(ToolError::Execution(format!(
                "HTTP error: {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            )));
        }

        if let Some(length) = response.content_length() {
            if length > MAX_RESPONSE_SIZE as u64 {
                return Err(ToolError::Execution(format!(
                    "Response too large (exceeds {}MB limit)",
                    MAX_RESPONSE_SIZE / 1024 / 1024
                )));
            }
        }

        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_lowercase();

        let body_bytes = read_limited_body(response).await?;

        if !is_text_content(&content_type) {
            let output = format!(
                "Fetched non-text content: {} bytes ({})",
                body_bytes.len(),
                content_type
            );

            return Ok(ToolResult::new(format!("Fetched: {}", final_url), output)
                .with_metadata("url", serde_json::json!(final_url))
                .with_metadata("content_type", serde_json::json!(content_type)));
        }

        let body = String::from_utf8_lossy(&body_bytes).into_owned();
        let output = match format.as_str() {
            "html" => body,
            "text" => {
                if content_type.contains("html") {
                    html_to_text(&body)
                } else {
                    body
                }
            }
            "markdown" => {
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

        Ok(
            ToolResult::new(format!("Fetched: {}", final_url), truncated)
                .with_metadata("url", serde_json::json!(final_url))
                .with_metadata("content_type", serde_json::json!(content_type)),
        )
    }
}

fn fetch_url_for(raw_url: &str) -> String {
    if raw_url.starts_with("http://") && !is_loopback_http_url(raw_url) {
        format!("https://{}", &raw_url[7..])
    } else {
        raw_url.to_string()
    }
}

fn is_loopback_http_url(raw_url: &str) -> bool {
    let Ok(url) = url::Url::parse(raw_url) else {
        return false;
    };

    if url.scheme() != "http" {
        return false;
    }

    let Some(host) = url.host_str() else {
        return false;
    };

    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }

    host.trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok_and(|addr| addr.is_loopback())
}

async fn send_request(
    client: &reqwest::Client,
    url: &str,
    format: &str,
    user_agent: &str,
) -> Result<Response, reqwest::Error> {
    client
        .get(url)
        .header(USER_AGENT, user_agent)
        .header(ACCEPT, accept_header(format))
        .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .send()
        .await
}

fn accept_header(format: &str) -> &'static str {
    match format {
        "markdown" => {
            "text/markdown;q=1.0, text/x-markdown;q=0.9, text/plain;q=0.8, text/html;q=0.7, */*;q=0.1"
        }
        "text" => "text/plain;q=1.0, text/markdown;q=0.9, text/html;q=0.8, */*;q=0.1",
        "html" => {
            "text/html;q=1.0, application/xhtml+xml;q=0.9, text/plain;q=0.8, text/markdown;q=0.7, */*;q=0.1"
        }
        _ => "*/*",
    }
}

fn is_cloudflare_challenge(response: &Response) -> bool {
    response.status() == StatusCode::FORBIDDEN
        && response
            .headers()
            .get("cf-mitigated")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("challenge"))
}

async fn read_limited_body(response: Response) -> Result<Vec<u8>, ToolError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| ToolError::Execution(format!("Failed to read response body: {}", e)))?;
        if body.len() + chunk.len() > MAX_RESPONSE_SIZE {
            return Err(ToolError::Execution(format!(
                "Response too large (exceeds {}MB limit)",
                MAX_RESPONSE_SIZE / 1024 / 1024
            )));
        }
        body.extend_from_slice(&chunk);
    }

    Ok(body)
}

fn is_text_content(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("javascript")
        || content_type.contains("x-www-form-urlencoded")
        || content_type.is_empty()
}

fn html_to_markdown(html: &str) -> String {
    let converted = convert_html(html, true);
    if converted.trim().is_empty() {
        metadata_fallback(html)
    } else {
        converted
    }
}

fn html_to_text(html: &str) -> String {
    let converted = convert_html(html, false);
    if converted.trim().is_empty() {
        metadata_fallback(html)
    } else {
        converted
    }
}

fn convert_html(html: &str, markdown: bool) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut raw_tag = String::new();
    let mut skip_tag: Option<String> = None;
    let mut link: Option<LinkState> = None;

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            raw_tag.clear();
            continue;
        }

        if in_tag {
            if ch == '>' {
                in_tag = false;
                handle_tag(&raw_tag, markdown, &mut result, &mut skip_tag, &mut link);
                raw_tag.clear();
                continue;
            }

            raw_tag.push(ch);
            continue;
        }

        if skip_tag.is_some() {
            continue;
        }

        if let Some(link) = &mut link {
            link.text.push(ch);
            continue;
        }

        push_text(&mut result, ch);
    }

    if let Some(link) = link {
        push_link(&mut result, link, markdown);
    }

    clean_output(&result)
}

#[derive(Debug)]
struct HtmlTag {
    name: String,
    attrs: String,
    closing: bool,
    self_closing: bool,
}

#[derive(Debug)]
struct LinkState {
    text: String,
    href: Option<String>,
}

fn handle_tag(
    raw_tag: &str,
    markdown: bool,
    result: &mut String,
    skip_tag: &mut Option<String>,
    link: &mut Option<LinkState>,
) {
    let Some(tag) = parse_tag(raw_tag) else {
        return;
    };

    if let Some(skipped) = skip_tag.as_ref() {
        if tag.closing && tag.name == *skipped {
            *skip_tag = None;
        }
        return;
    }

    if is_skipped_tag(&tag.name) && !tag.closing {
        if !tag.self_closing {
            *skip_tag = Some(tag.name);
        }
        return;
    }

    if tag.name == "a" {
        if tag.closing {
            if let Some(link_state) = link.take() {
                push_link(result, link_state, markdown);
            }
        } else {
            *link = Some(LinkState {
                text: String::new(),
                href: extract_attr(&tag.attrs, "href"),
            });
        }
        return;
    }

    if tag.closing {
        if is_block_tag(&tag.name) || is_heading_tag(&tag.name) {
            ensure_blank_line(result);
        }
        return;
    }

    match tag.name.as_str() {
        "br" => ensure_newline(result),
        "hr" => {
            ensure_blank_line(result);
            if markdown {
                result.push_str("---");
                ensure_blank_line(result);
            }
        }
        "li" => {
            ensure_newline(result);
            if markdown {
                result.push_str("- ");
            }
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            ensure_blank_line(result);
            if markdown {
                let level = tag.name[1..].parse::<usize>().unwrap_or(1);
                result.push_str(&"#".repeat(level));
                result.push(' ');
            }
        }
        name if is_block_tag(name) => ensure_blank_line(result),
        _ => {}
    }
}

fn parse_tag(raw_tag: &str) -> Option<HtmlTag> {
    let mut tag = raw_tag.trim();
    if tag.is_empty() || tag.starts_with('!') || tag.starts_with('?') || tag.starts_with("!--") {
        return None;
    }

    let closing = tag.starts_with('/');
    if closing {
        tag = tag[1..].trim_start();
    }

    let self_closing = tag.ends_with('/');
    if self_closing {
        tag = tag[..tag.len().saturating_sub(1)].trim_end();
    }

    let name_end = tag
        .find(|ch: char| ch.is_whitespace() || ch == '/')
        .unwrap_or(tag.len());
    if name_end == 0 {
        return None;
    }

    Some(HtmlTag {
        name: tag[..name_end].to_ascii_lowercase(),
        attrs: tag[name_end..].trim().to_string(),
        closing,
        self_closing,
    })
}

fn is_skipped_tag(name: &str) -> bool {
    matches!(
        name,
        "head" | "script" | "style" | "noscript" | "iframe" | "object" | "embed" | "svg" | "canvas"
    )
}

fn is_heading_tag(name: &str) -> bool {
    matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "dl"
            | "dt"
            | "dd"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "header"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "tr"
            | "ul"
    )
}

fn extract_attr(attrs: &str, name: &str) -> Option<String> {
    let pattern = format!(
        r#"(?is)(?:^|\s){}\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#,
        regex::escape(name)
    );
    let re = regex::Regex::new(&pattern).ok()?;
    let captures = re.captures(attrs)?;
    for idx in 1..=3 {
        if let Some(value) = captures.get(idx) {
            return Some(decode_html_entities(value.as_str()).trim().to_string());
        }
    }
    None
}

fn push_link(result: &mut String, link: LinkState, markdown: bool) {
    let text = normalize_inline(&link.text);
    if text.is_empty() {
        return;
    }

    if markdown {
        if let Some(href) = link.href.filter(|href| !href.trim().is_empty()) {
            result.push_str(&format!("[{}]({})", text, href.trim()));
        } else {
            result.push_str(&text);
        }
    } else {
        result.push_str(&text);
    }
}

fn push_text(result: &mut String, ch: char) {
    if ch.is_whitespace() {
        if !result
            .chars()
            .last()
            .is_some_and(|last| last.is_whitespace())
        {
            result.push(' ');
        }
    } else {
        result.push(ch);
    }
}

fn ensure_newline(result: &mut String) {
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
}

fn ensure_blank_line(result: &mut String) {
    if result.trim().is_empty() {
        return;
    }
    while result.ends_with(' ') || result.ends_with('\t') {
        result.pop();
    }
    if result.ends_with("\n\n") {
        return;
    }
    ensure_newline(result);
    result.push('\n');
}

fn clean_output(input: &str) -> String {
    let decoded = decode_html_entities(input);
    let mut final_result = String::new();
    let mut blank_count = 0u32;

    for line in decoded.lines() {
        let line = normalize_inline(line);
        if line.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                final_result.push('\n');
            }
        } else {
            blank_count = 0;
            final_result.push_str(&line);
            final_result.push('\n');
        }
    }

    final_result.trim().to_string()
}

fn normalize_inline(input: &str) -> String {
    decode_html_entities(input)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_html_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '&' {
            output.push(ch);
            continue;
        }

        let mut entity = String::new();
        let mut lookahead = chars.clone();
        let mut found_semicolon = false;

        for _ in 0..32 {
            let Some(next) = lookahead.next() else {
                break;
            };
            if next == ';' {
                found_semicolon = true;
                break;
            }
            if next.is_whitespace() || next == '&' {
                break;
            }
            entity.push(next);
        }

        if !found_semicolon {
            output.push('&');
            continue;
        }

        for _ in 0..=entity.chars().count() {
            chars.next();
        }

        if let Some(decoded) = decode_entity(&entity) {
            output.push(decoded);
        } else {
            output.push('&');
            output.push_str(&entity);
            output.push(';');
        }
    }

    output
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ if entity == "#39" => Some('\''),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn metadata_fallback(html: &str) -> String {
    let mut parts = Vec::new();

    if let Some(title) = extract_title(html) {
        parts.push(title);
    }
    if let Some(description) = extract_meta_description(html) {
        parts.push(description);
    }

    parts.join("\n\n")
}

fn extract_title(html: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
    let title = re.captures(html)?.get(1)?.as_str();
    let title = normalize_inline(title);
    (!title.is_empty()).then_some(title)
}

fn extract_meta_description(html: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?is)<meta\s+([^>]+)>").ok()?;
    for captures in re.captures_iter(html) {
        let attrs = captures.get(1).map(|m| m.as_str()).unwrap_or_default();
        let name = extract_attr(attrs, "name")
            .or_else(|| extract_attr(attrs, "property"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "description" | "og:description" | "twitter:description"
        ) {
            let Some(content) = extract_attr(attrs, "content") else {
                continue;
            };
            let content = normalize_inline(&content);
            if !content.is_empty() {
                return Some(content);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_continues_after_script_with_closing_tag() {
        let html = r#"
            <html>
              <head><script src="/app.js"></script></head>
              <body>
                <h1>Profile Heading</h1>
                <p>This paragraph should remain visible after a script tag.</p>
              </body>
            </html>
        "#;

        let markdown = html_to_markdown(html);

        assert!(markdown.contains("# Profile Heading"));
        assert!(markdown.contains("This paragraph should remain visible"));
    }

    #[test]
    fn markdown_skips_script_and_style_text() {
        let html = r#"
            <style>.hidden { display: none; }</style>
            <script>document.body.innerHTML = "not content";</script>
            <main><p>Visible content</p></main>
        "#;

        let markdown = html_to_markdown(html);

        assert_eq!(markdown, "Visible content");
    }

    #[test]
    fn markdown_preserves_links_without_lowercasing_href() {
        let html = r#"<p>Read <a href="https://Example.com/Path?A=1&amp;B=2">the docs</a>.</p>"#;

        let markdown = html_to_markdown(html);

        assert_eq!(
            markdown,
            "Read [the docs](https://Example.com/Path?A=1&B=2)."
        );
    }

    #[test]
    fn fetch_url_preserves_local_http_urls() {
        assert_eq!(
            fetch_url_for("http://127.0.0.1:41234/api/releases.json"),
            "http://127.0.0.1:41234/api/releases.json"
        );
        assert_eq!(
            fetch_url_for("http://localhost:3000/index.html"),
            "http://localhost:3000/index.html"
        );
        assert_eq!(
            fetch_url_for("http://[::1]:3000/index.html"),
            "http://[::1]:3000/index.html"
        );
    }

    #[test]
    fn fetch_url_upgrades_public_http_urls() {
        assert_eq!(
            fetch_url_for("http://example.com/docs"),
            "https://example.com/docs"
        );
        assert_eq!(
            fetch_url_for("https://example.com/docs"),
            "https://example.com/docs"
        );
    }

    #[test]
    fn metadata_fallback_prevents_empty_html_result() {
        let html = r#"
            <html>
              <head>
                <title>Example Page</title>
                <meta name="description" content="Fallback summary for the page." />
              </head>
              <body><script>window.__APP__ = true;</script></body>
            </html>
        "#;

        let markdown = html_to_markdown(html);

        assert_eq!(markdown, "Example Page\n\nFallback summary for the page.");
    }
}
