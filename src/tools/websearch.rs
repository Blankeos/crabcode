use crate::config::configuration::{WebsearchConfig, WebsearchProvider};
use crate::tools::{
    get_integer_param, get_string_param, validate_required, ParameterSchema, ParameterType, Tool,
    ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde_json::Value;
use url::Url;

pub struct WebsearchTool {
    config: WebsearchConfig,
    client: reqwest::Client,
}

const DEFAULT_EXA_MCP_ENDPOINT: &str = "https://mcp.exa.ai/mcp";
const DEFAULT_FIRECRAWL_MCP_ENDPOINT: &str = "https://mcp.firecrawl.dev/v2/mcp";
const DEFAULT_EXA_ENDPOINT: &str = "https://api.exa.ai/search";
const DEFAULT_TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";
const DEFAULT_PERPLEXITY_ENDPOINT: &str = "https://api.perplexity.ai/search";
const DEFAULT_BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const DEFAULT_OLLAMA_CLOUD_ENDPOINT: &str = "https://ollama.com/api/web_search";
const DEFAULT_SERPAPI_ENDPOINT: &str = "https://serpapi.com/search.json";
const DEFAULT_KEIRO_ENDPOINT: &str = "https://kierolabs.space/api/v2/keiro";
const DEFAULT_TIMEOUT_SECS: u64 = 25;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const DEFAULT_NUM_RESULTS: i64 = 8;
const MAX_NUM_RESULTS: i64 = 20;
const DEFAULT_CONTEXT_MAX_CHARS: i64 = 10_000;
const MAX_CONTEXT_MAX_CHARS: i64 = 50_000;
const USER_AGENT_VALUE: &str = "crabcode/0.1";
const NO_RESULTS: &str = "No search results found. Please try a different query.";

impl WebsearchTool {
    pub fn new(config: WebsearchConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::builder()
                .use_rustls_tls()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn is_enabled_for_provider(_provider_name: &str, config: &WebsearchConfig) -> bool {
        config.enabled.unwrap_or(true)
    }

    fn adapter(&self) -> Box<dyn WebsearchAdapter + Send + Sync + '_> {
        match self.config.provider {
            WebsearchProvider::ExaHostedMcp => Box::new(ExaHostedMcpAdapter {
                config: &self.config,
            }),
            WebsearchProvider::FirecrawlHostedMcp => Box::new(FirecrawlHostedMcpAdapter {
                config: &self.config,
            }),
            WebsearchProvider::Exa => Box::new(ExaApiAdapter {
                config: &self.config,
            }),
            WebsearchProvider::Tavily => Box::new(TavilyAdapter {
                config: &self.config,
            }),
            WebsearchProvider::Perplexity => Box::new(PerplexityAdapter {
                config: &self.config,
            }),
            WebsearchProvider::Brave => Box::new(BraveAdapter {
                config: &self.config,
            }),
            WebsearchProvider::OllamaCloud => Box::new(OllamaCloudAdapter {
                config: &self.config,
            }),
            WebsearchProvider::SerpApi => Box::new(SerpApiAdapter {
                config: &self.config,
            }),
            WebsearchProvider::Keiro => Box::new(KeiroAdapter {
                config: &self.config,
            }),
        }
    }
}

#[async_trait]
impl ToolHandler for WebsearchTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "websearch".to_string(),
            description: format!(
                "Search the web for current information beyond the model's knowledge cutoff.\n\nProvider: {}. Exa hosted MCP and Firecrawl hosted MCP work without an API key; keyed providers use websearch.apiKey, commonly with {{env:...}} placeholders.\n\nUse websearch for discovery and webfetch when you already know the URL.",
                self.config.provider.as_str()
            ),
            parameters: vec![
                ParameterSchema {
                    name: "query".to_string(),
                    description: "Web search query".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "numResults".to_string(),
                    description: format!("Number of search results to return (default {DEFAULT_NUM_RESULTS}, max {MAX_NUM_RESULTS})"),
                    required: false,
                    param_type: ParameterType::Integer,
                },
                ParameterSchema {
                    name: "livecrawl".to_string(),
                    description: "Live crawl mode: fallback or preferred (supported by Exa providers; default fallback)".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "type".to_string(),
                    description: "Search type: auto, fast, or deep (mapped to provider-specific depth where needed; default auto)".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "contextMaxCharacters".to_string(),
                    description: format!("Maximum context characters (default {DEFAULT_CONTEXT_MAX_CHARS}, max {MAX_CONTEXT_MAX_CHARS})"),
                    required: false,
                    param_type: ParameterType::Integer,
                },
            ],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["query"])?;
        let query = get_string_param(params, "query").unwrap_or_default();
        if query.trim().is_empty() {
            return Err(ToolError::Validation("query must not be empty".to_string()));
        }
        if let Some(num_results) = get_integer_param(params, "numResults") {
            if !(1..=MAX_NUM_RESULTS).contains(&num_results) {
                return Err(ToolError::Validation(format!(
                    "numResults must be between 1 and {MAX_NUM_RESULTS}"
                )));
            }
        }
        if let Some(livecrawl) = get_string_param(params, "livecrawl") {
            if !matches!(livecrawl.as_str(), "fallback" | "preferred") {
                return Err(ToolError::Validation(
                    "livecrawl must be one of: fallback, preferred".to_string(),
                ));
            }
        }
        if let Some(search_type) = get_string_param(params, "type") {
            if !matches!(search_type.as_str(), "auto" | "fast" | "deep") {
                return Err(ToolError::Validation(
                    "type must be one of: auto, fast, deep".to_string(),
                ));
            }
        }
        if let Some(max_chars) = get_integer_param(params, "contextMaxCharacters") {
            if !(1..=MAX_CONTEXT_MAX_CHARS).contains(&max_chars) {
                return Err(ToolError::Validation(format!(
                    "contextMaxCharacters must be between 1 and {MAX_CONTEXT_MAX_CHARS}"
                )));
            }
        }
        Ok(())
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let input = WebsearchInput {
            query: get_string_param(&params, "query").unwrap_or_default(),
            num_results: get_integer_param(&params, "numResults").unwrap_or(DEFAULT_NUM_RESULTS),
            livecrawl: get_string_param(&params, "livecrawl")
                .unwrap_or_else(|| "fallback".to_string()),
            search_type: get_string_param(&params, "type").unwrap_or_else(|| "auto".to_string()),
            context_max_characters: get_integer_param(&params, "contextMaxCharacters")
                .unwrap_or(DEFAULT_CONTEXT_MAX_CHARS),
            session_id: ctx.session_id.clone(),
        };
        let adapter = self.adapter();
        let provider = adapter.provider_name();
        let output = adapter.search(&self.client, &input).await?;
        let output = if output.trim().is_empty() {
            NO_RESULTS.to_string()
        } else {
            output
        };
        Ok(
            ToolResult::new(format!("Web Search: {}", input.query), output)
                .with_metadata("query", Value::String(input.query))
                .with_metadata("provider", Value::String(provider.to_string())),
        )
    }
}

struct WebsearchInput {
    query: String,
    num_results: i64,
    livecrawl: String,
    search_type: String,
    context_max_characters: i64,
    session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchItem {
    title: String,
    url: String,
    snippet: Option<String>,
    date: Option<String>,
}

#[async_trait]
trait WebsearchAdapter {
    fn provider_name(&self) -> &'static str;
    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError>;
}

struct ExaHostedMcpAdapter<'a> {
    config: &'a WebsearchConfig,
}
struct FirecrawlHostedMcpAdapter<'a> {
    config: &'a WebsearchConfig,
}
struct ExaApiAdapter<'a> {
    config: &'a WebsearchConfig,
}
struct TavilyAdapter<'a> {
    config: &'a WebsearchConfig,
}
struct PerplexityAdapter<'a> {
    config: &'a WebsearchConfig,
}
struct BraveAdapter<'a> {
    config: &'a WebsearchConfig,
}
struct OllamaCloudAdapter<'a> {
    config: &'a WebsearchConfig,
}

struct SerpApiAdapter<'a> {
    config: &'a WebsearchConfig,
}

struct KeiroAdapter<'a> {
    config: &'a WebsearchConfig,
}

#[async_trait]
impl WebsearchAdapter for ExaHostedMcpAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "exa-hosted-mcp"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let mut endpoint = endpoint_or(&self.config.endpoint, DEFAULT_EXA_MCP_ENDPOINT);
        if let Some(api_key) = configured_api_key(self.config) {
            endpoint = append_query_param(&endpoint, "exaApiKey", &api_key)?;
        }
        let request = client
            .post(&endpoint)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "web_search_exa",
                    "arguments": {
                        "query": input.query,
                        "type": input.search_type,
                        "numResults": input.num_results,
                        "livecrawl": input.livecrawl,
                        "contextMaxCharacters": input.context_max_characters,
                        "sessionId": input.session_id,
                    }
                }
            }))
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));

        let body = send_text(request, self.provider_name()).await?;
        parse_mcp_response(&body).ok_or_else(|| {
            ToolError::Execution("websearch provider returned no text content".to_string())
        })
    }
}

#[async_trait]
impl WebsearchAdapter for FirecrawlHostedMcpAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "firecrawl-hosted-mcp"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let endpoint = endpoint_or(&self.config.endpoint, DEFAULT_FIRECRAWL_MCP_ENDPOINT);
        let mut request = client
            .post(&endpoint)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "firecrawl_search",
                    "arguments": {
                        "query": input.query,
                        "limit": input.num_results,
                    }
                }
            }))
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        if let Some(api_key) = configured_api_key(self.config) {
            request = request.bearer_auth(api_key);
        }

        let body = send_text(request, self.provider_name()).await?;
        let text = parse_mcp_response(&body).ok_or_else(|| {
            ToolError::Execution("websearch provider returned no text content".to_string())
        })?;
        let value = parse_json_body(&text, self.provider_name())?;
        Ok(format_results(
            self.provider_name(),
            &input.query,
            parse_firecrawl_results(&value),
            None,
        ))
    }
}

#[async_trait]
impl WebsearchAdapter for ExaApiAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "exa"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let api_key = require_api_key(self.config, self.provider_name(), "EXA_API_KEY")?;
        let endpoint = endpoint_or(&self.config.endpoint, DEFAULT_EXA_ENDPOINT);
        let request = client
            .post(&endpoint)
            .header("x-api-key", api_key)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(&serde_json::json!({
                "query": input.query,
                "type": exa_search_type(&input.search_type),
                "numResults": input.num_results,
                "contents": {
                    "highlights": true,
                    "summary": { "query": input.query },
                    "livecrawl": input.livecrawl,
                }
            }))
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        let body = send_text(request, self.provider_name()).await?;
        let value = parse_json_body(&body, self.provider_name())?;
        Ok(format_results(
            self.provider_name(),
            &input.query,
            parse_exa_results(&value),
            value
                .pointer("/output/content")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        ))
    }
}

#[async_trait]
impl WebsearchAdapter for TavilyAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "tavily"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let api_key = require_api_key(self.config, self.provider_name(), "TAVILY_API_KEY")?;
        let endpoint = endpoint_or(&self.config.endpoint, DEFAULT_TAVILY_ENDPOINT);
        let request = client
            .post(&endpoint)
            .bearer_auth(api_key)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(&serde_json::json!({
                "query": input.query,
                "search_depth": tavily_depth(&input.search_type),
                "max_results": input.num_results,
                "include_answer": true,
                "include_raw_content": false,
                "include_favicon": true,
            }))
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        let body = send_text(request, self.provider_name()).await?;
        let value = parse_json_body(&body, self.provider_name())?;
        Ok(format_results(
            self.provider_name(),
            &input.query,
            parse_tavily_results(&value),
            value
                .get("answer")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        ))
    }
}

#[async_trait]
impl WebsearchAdapter for PerplexityAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "perplexity"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let api_key = require_api_key(self.config, self.provider_name(), "PERPLEXITY_API_KEY")?;
        let endpoint = endpoint_or(&self.config.endpoint, DEFAULT_PERPLEXITY_ENDPOINT);
        let request = client
            .post(&endpoint)
            .bearer_auth(api_key)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(&serde_json::json!({
                "query": input.query,
                "max_results": input.num_results,
                "search_context_size": perplexity_context_size(input.context_max_characters),
            }))
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        let body = send_text(request, self.provider_name()).await?;
        let value = parse_json_body(&body, self.provider_name())?;
        Ok(format_results(
            self.provider_name(),
            &input.query,
            parse_perplexity_results(&value),
            None,
        ))
    }
}

#[async_trait]
impl WebsearchAdapter for BraveAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "brave"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let api_key = require_api_key(self.config, self.provider_name(), "BRAVE_SEARCH_API_KEY")?;
        let endpoint = endpoint_or(&self.config.endpoint, DEFAULT_BRAVE_ENDPOINT);
        let count = input.num_results.to_string();
        let request = client
            .get(&endpoint)
            .header("X-Subscription-Token", api_key)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .query(&[
                ("q", input.query.as_str()),
                ("count", count.as_str()),
                ("extra_snippets", "true"),
            ])
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        let body = send_text(request, self.provider_name()).await?;
        let value = parse_json_body(&body, self.provider_name())?;
        Ok(format_results(
            self.provider_name(),
            &input.query,
            parse_brave_results(&value),
            None,
        ))
    }
}

#[async_trait]
impl WebsearchAdapter for OllamaCloudAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "ollama-cloud"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let api_key = require_api_key(self.config, self.provider_name(), "OLLAMA_API_KEY")?;
        let endpoint = endpoint_or(&self.config.endpoint, DEFAULT_OLLAMA_CLOUD_ENDPOINT);
        let request = client
            .post(&endpoint)
            .bearer_auth(api_key)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(&serde_json::json!({
                "query": input.query,
                "max_results": input.num_results.min(10),
            }))
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        let body = send_text(request, self.provider_name()).await?;
        let value = parse_json_body(&body, self.provider_name())?;
        Ok(format_results(
            self.provider_name(),
            &input.query,
            parse_ollama_results(&value),
            None,
        ))
    }
}

#[async_trait]
impl WebsearchAdapter for SerpApiAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "serpapi"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let api_key = require_api_key(self.config, self.provider_name(), "SERPAPI_API_KEY")?;
        let endpoint = endpoint_or(&self.config.endpoint, DEFAULT_SERPAPI_ENDPOINT);
        let num = input.num_results.to_string();
        let request = client
            .get(&endpoint)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .query(&[
                ("engine", "google"),
                ("q", input.query.as_str()),
                ("num", num.as_str()),
                ("api_key", api_key.as_str()),
            ])
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        let body = send_text(request, self.provider_name()).await?;
        let value = parse_json_body(&body, self.provider_name())?;
        Ok(format_results(
            self.provider_name(),
            &input.query,
            parse_serpapi_results(&value),
            None,
        ))
    }
}

#[async_trait]
impl WebsearchAdapter for KeiroAdapter<'_> {
    fn provider_name(&self) -> &'static str {
        "keiro"
    }

    async fn search(
        &self,
        client: &reqwest::Client,
        input: &WebsearchInput,
    ) -> Result<String, ToolError> {
        let api_key = require_api_key(self.config, self.provider_name(), "KEIRO_API_KEY")?;
        let endpoint = endpoint_or(&self.config.endpoint, DEFAULT_KEIRO_ENDPOINT);
        let request = client
            .post(&endpoint)
            .bearer_auth(api_key)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(&serde_json::json!({
                "query": input.query,
                "maxResults": input.num_results,
            }))
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        let body = send_text(request, self.provider_name()).await?;
        let value = parse_json_body(&body, self.provider_name())?;
        Ok(format_results(
            self.provider_name(),
            &input.query,
            parse_keiro_results(&value),
            None,
        ))
    }
}

fn endpoint_or(configured: &Option<String>, default: &str) -> String {
    configured.clone().unwrap_or_else(|| default.to_string())
}

fn append_query_param(endpoint: &str, key: &str, value: &str) -> Result<String, ToolError> {
    let mut url = Url::parse(endpoint).map_err(|err| {
        ToolError::Validation(format!("invalid websearch endpoint '{}': {err}", endpoint))
    })?;
    url.query_pairs_mut().append_pair(key, value);
    Ok(url.to_string())
}

fn configured_api_key(config: &WebsearchConfig) -> Option<String> {
    config
        .api_key
        .clone()
        .filter(|value| !value.trim().is_empty())
}

fn require_api_key(
    config: &WebsearchConfig,
    provider: &str,
    env_hint: &str,
) -> Result<String, ToolError> {
    configured_api_key(config).ok_or_else(|| {
        ToolError::Validation(format!(
            "websearch provider '{provider}' requires websearch.apiKey, for example {{env:{env_hint}}}"
        ))
    })
}

async fn send_text(request: reqwest::RequestBuilder, provider: &str) -> Result<String, ToolError> {
    let response = request.send().await.map_err(|err| {
        ToolError::Execution(format!("{provider} websearch request failed: {err}"))
    })?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|err| {
        ToolError::Execution(format!(
            "failed to read {provider} websearch response: {err}"
        ))
    })?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ToolError::Execution(format!(
            "{provider} websearch response exceeded {MAX_RESPONSE_BYTES} bytes"
        )));
    }
    let body = String::from_utf8_lossy(&bytes).to_string();
    if !status.is_success() {
        return Err(ToolError::Execution(format!(
            "{provider} websearch returned HTTP {status}: {}",
            truncate(&sanitize_provider_error(&body), 500)
        )));
    }
    Ok(body)
}

fn sanitize_provider_error(body: &str) -> String {
    body.replace("web_search_exa", "websearch")
        .replace("firecrawl_search", "websearch")
}

fn parse_json_body(body: &str, provider: &str) -> Result<Value, ToolError> {
    serde_json::from_str(body).map_err(|err| {
        ToolError::Execution(format!("failed to parse {provider} websearch JSON: {err}"))
    })
}

fn exa_search_type(raw: &str) -> &str {
    match raw {
        "fast" => "fast",
        "deep" => "deep",
        _ => "auto",
    }
}

fn tavily_depth(raw: &str) -> &str {
    match raw {
        "fast" => "fast",
        "deep" => "advanced",
        _ => "basic",
    }
}

fn perplexity_context_size(max_chars: i64) -> &'static str {
    if max_chars <= 5_000 {
        "low"
    } else if max_chars <= 20_000 {
        "medium"
    } else {
        "high"
    }
}

fn parse_exa_results(value: &Value) -> Vec<SearchItem> {
    value
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let title = string_field(item, "title")?;
            let url = string_field(item, "url")?;
            let snippet = string_field(item, "summary")
                .or_else(|| first_string(item.get("highlights")))
                .or_else(|| string_field(item, "text"))
                .map(|value| clean_snippet(&value));
            let date = string_field(item, "publishedDate");
            Some(SearchItem {
                title,
                url,
                snippet,
                date,
            })
        })
        .collect()
}

fn parse_firecrawl_results(value: &Value) -> Vec<SearchItem> {
    let items = value
        .pointer("/data/web")
        .or_else(|| value.get("web"))
        .or_else(|| value.get("data"))
        .or_else(|| value.get("results"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten();

    items
        .filter_map(|item| {
            let title = string_field(item, "title").unwrap_or_else(|| {
                string_field(item, "url").unwrap_or_else(|| "Untitled".to_string())
            });
            let url = string_field(item, "url")?;
            let snippet = string_field(item, "description")
                .or_else(|| string_field(item, "snippet"))
                .or_else(|| string_field(item, "markdown"))
                .or_else(|| string_field(item, "content"))
                .map(|value| clean_snippet(&value));
            Some(SearchItem {
                title,
                url,
                snippet,
                date: None,
            })
        })
        .collect()
}

fn parse_tavily_results(value: &Value) -> Vec<SearchItem> {
    parse_standard_results(value, &["content", "raw_content", "snippet", "description"])
}

fn parse_perplexity_results(value: &Value) -> Vec<SearchItem> {
    value
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let title = string_field(item, "title")?;
            let url = string_field(item, "url")?;
            let snippet = string_field(item, "snippet").map(|value| clean_snippet(&value));
            let date = string_field(item, "date").or_else(|| string_field(item, "last_updated"));
            Some(SearchItem {
                title,
                url,
                snippet,
                date,
            })
        })
        .collect()
}

fn parse_brave_results(value: &Value) -> Vec<SearchItem> {
    value
        .pointer("/web/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let title = string_field(item, "title")?;
            let url = string_field(item, "url")?;
            let mut snippets = Vec::new();
            if let Some(description) = string_field(item, "description") {
                snippets.push(description);
            }
            if let Some(extra) = item.get("extra_snippets").and_then(Value::as_array) {
                snippets.extend(
                    extra
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string),
                );
            }
            let snippet = (!snippets.is_empty()).then(|| clean_snippet(&snippets.join(" ")));
            let date = string_field(item, "age");
            Some(SearchItem {
                title,
                url,
                snippet,
                date,
            })
        })
        .collect()
}

fn parse_ollama_results(value: &Value) -> Vec<SearchItem> {
    parse_standard_results(value, &["content", "snippet", "text", "description"])
}

fn parse_serpapi_results(value: &Value) -> Vec<SearchItem> {
    let mut results = Vec::new();

    if let Some(answer_box) = value.get("answer_box") {
        if let Some(url) = string_field(answer_box, "link") {
            if let Some(title) =
                string_field(answer_box, "title").or_else(|| string_field(answer_box, "answer"))
            {
                results.push(SearchItem {
                    title,
                    url,
                    snippet: string_field(answer_box, "snippet")
                        .or_else(|| string_field(answer_box, "answer"))
                        .map(|value| clean_snippet(&value)),
                    date: None,
                });
            }
        }
    }

    results.extend(
        value
            .get("organic_results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let title = string_field(item, "title")?;
                let url = string_field(item, "link")?;
                Some(SearchItem {
                    title,
                    url,
                    snippet: string_field(item, "snippet").map(|value| clean_snippet(&value)),
                    date: string_field(item, "date"),
                })
            }),
    );

    results
}

fn parse_keiro_results(value: &Value) -> Vec<SearchItem> {
    parse_standard_results(value, &["snippet", "content", "text", "description"])
}

fn parse_standard_results(value: &Value, snippet_keys: &[&str]) -> Vec<SearchItem> {
    value
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let title = string_field(item, "title")?;
            let url = string_field(item, "url")?;
            let snippet = snippet_keys
                .iter()
                .find_map(|key| string_field(item, key))
                .map(|value| clean_snippet(&value));
            Some(SearchItem {
                title,
                url,
                snippet,
                date: None,
            })
        })
        .collect()
}

fn format_results(
    provider: &str,
    query: &str,
    results: Vec<SearchItem>,
    answer: Option<String>,
) -> String {
    let mut out = format!("Search provider: {provider}\nQuery: {query}\n");
    if let Some(answer) = answer.filter(|value| !value.trim().is_empty()) {
        out.push_str("\nAnswer/context:\n");
        out.push_str(answer.trim());
        out.push('\n');
    }
    if results.is_empty() {
        out.push_str("\n");
        out.push_str(NO_RESULTS);
        return out;
    }
    out.push_str("\nResults:\n");
    for (idx, item) in results.into_iter().enumerate() {
        out.push_str(&format!("{}. {}\n   {}\n", idx + 1, item.title, item.url));
        if let Some(date) = item.date.filter(|value| !value.trim().is_empty()) {
            out.push_str(&format!("   Date: {}\n", date.trim()));
        }
        if let Some(snippet) = item.snippet.filter(|value| !value.trim().is_empty()) {
            out.push_str(&format!("   {}\n", truncate(snippet.trim(), 900)));
        }
    }
    out
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn clean_snippet(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

pub fn parse_mcp_response(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if let Some(text) = parse_mcp_payload(trimmed) {
        return Some(text);
    }
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if let Some(text) = parse_mcp_payload(data.trim()) {
            return Some(text);
        }
    }
    None
}

fn parse_mcp_payload(payload: &str) -> Option<String> {
    if !payload.trim_start().starts_with('{') {
        return None;
    }
    let value: Value = serde_json::from_str(payload).ok()?;
    value
        .get("result")?
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .find(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_plain_json_rpc_response() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": "search results" }] }
        })
        .to_string();
        assert_eq!(parse_mcp_response(&body).as_deref(), Some("search results"));
    }

    #[test]
    fn parses_sse_json_rpc_response() {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": "search results" }] }
        })
        .to_string();
        assert_eq!(
            parse_mcp_response(&format!(
                "data: [DONE]\nevent: message\ndata: {payload}\n\n"
            ))
            .as_deref(),
            Some("search results")
        );
    }

    #[test]
    fn parses_exa_results() {
        let value = json!({
            "results": [{
                "title": "Exa Result",
                "url": "https://example.com",
                "highlights": ["A useful highlight"],
                "publishedDate": "2026-01-01"
            }]
        });
        assert_eq!(
            parse_exa_results(&value),
            vec![SearchItem {
                title: "Exa Result".to_string(),
                url: "https://example.com".to_string(),
                snippet: Some("A useful highlight".to_string()),
                date: Some("2026-01-01".to_string()),
            }]
        );
    }

    #[test]
    fn parses_firecrawl_results() {
        let value = json!({
            "success": true,
            "data": {
                "web": [{
                    "url": "https://docs.rs/axum",
                    "title": "axum - Rust",
                    "description": "Web framework that focuses on ergonomics and modularity"
                }]
            }
        });
        assert_eq!(
            parse_firecrawl_results(&value),
            vec![SearchItem {
                title: "axum - Rust".to_string(),
                url: "https://docs.rs/axum".to_string(),
                snippet: Some(
                    "Web framework that focuses on ergonomics and modularity".to_string()
                ),
                date: None,
            }]
        );
    }

    #[test]
    fn parses_tavily_results() {
        let value = json!({
            "answer": "short answer",
            "results": [{"title": "Tavily", "url": "https://t.example", "content": "snippet"}]
        });
        assert_eq!(parse_tavily_results(&value)[0].title, "Tavily");
    }

    #[test]
    fn parses_perplexity_results() {
        let value = json!({
            "results": [{"title": "PPLX", "url": "https://p.example", "snippet": "snippet", "date": "2026-01-01"}]
        });
        assert_eq!(parse_perplexity_results(&value)[0].url, "https://p.example");
    }

    #[test]
    fn parses_brave_results() {
        let value = json!({
            "web": { "results": [{"title": "Brave", "url": "https://b.example", "description": "desc", "extra_snippets": ["extra"]}] }
        });
        assert_eq!(
            parse_brave_results(&value)[0].snippet.as_deref(),
            Some("desc extra")
        );
    }

    #[test]
    fn parses_ollama_results() {
        let value = json!({
            "results": [{"title": "Ollama", "url": "https://o.example", "content": "snippet"}]
        });
        assert_eq!(parse_ollama_results(&value)[0].title, "Ollama");
    }

    #[test]
    fn parses_serpapi_results() {
        let value = json!({
            "organic_results": [{"title": "SerpAPI", "link": "https://s.example", "snippet": "snippet", "date": "2026"}]
        });
        assert_eq!(parse_serpapi_results(&value)[0].url, "https://s.example");
    }

    #[test]
    fn parses_keiro_results() {
        let value = json!({
            "results": [{"title": "Keiro", "url": "https://k.example", "snippet": "snippet"}]
        });
        assert_eq!(parse_keiro_results(&value)[0].title, "Keiro");
    }

    #[test]
    fn validates_numeric_controls() {
        let tool = WebsearchTool::new(WebsearchConfig::default());
        assert!(tool
            .validate(&json!({ "query": "rust", "numResults": 21 }))
            .is_err());
        assert!(tool
            .validate(&json!({ "query": "rust", "contextMaxCharacters": 50_001 }))
            .is_err());
        assert!(tool
            .validate(&json!({ "query": "rust", "numResults": 8 }))
            .is_ok());
    }

    #[test]
    fn enabled_by_default_but_config_can_disable() {
        assert!(WebsearchTool::is_enabled_for_provider(
            "openai",
            &WebsearchConfig::default()
        ));

        let mut disabled = WebsearchConfig::default();
        disabled.enabled = Some(false);
        assert!(!WebsearchTool::is_enabled_for_provider(
            "opencode", &disabled
        ));
    }

    #[test]
    fn keyed_providers_require_api_key() {
        let config = WebsearchConfig {
            provider: WebsearchProvider::Tavily,
            ..WebsearchConfig::default()
        };
        assert!(require_api_key(&config, "tavily", "TAVILY_API_KEY").is_err());
    }

    #[test]
    fn sanitizes_internal_exa_tool_name_in_provider_errors() {
        assert_eq!(
            sanitize_provider_error("web_search_exa error (401): Invalid API key"),
            "websearch error (401): Invalid API key"
        );
        assert_eq!(
            sanitize_provider_error("firecrawl_search error (429): rate limited"),
            "websearch error (429): rate limited"
        );
    }
}
