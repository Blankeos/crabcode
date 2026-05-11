use crate::tools::ToolRegistry;
use crate::agent::config::{get_llm_session, ProviderKind};

const EXPLORE_SYSTEM_PROMPT: &str = r#"You are a fast, read-only code exploration agent. Your job is to search codebases, find files, and answer questions about code structure.

TOOLS AVAILABLE:
- glob: Find files by pattern matching
- grep: Search file contents using regex
- read: Read file contents with pagination
- list: List directory contents

IMPORTANT RULES:
- Only use the tools listed above (glob, grep, read, list)
- Search in parallel when possible (use multiple tool calls at once)
- Be thorough - search patterns, naming conventions, and related files
- Return a single comprehensive message with all findings
- Focus on precise code locations (file paths and line numbers)
- If you can't find something after thorough searching, report that clearly
- Do NOT use bash, write, edit, or any other tools

You will receive a detailed task description from the primary agent. Complete it and return your findings in a single message."#;

const GENERAL_SYSTEM_PROMPT: &str = r#"You are a general-purpose subagent that can use all available tools to complete complex multi-step tasks autonomously.

IMPORTANT RULES:
- Your entire response will be returned to the primary agent as a single tool result
- Complete ALL steps autonomously before returning
- Be thorough and verify your work using available tools
- Return a single comprehensive message with your results
- Do NOT ask questions back to the user - just complete the task
- Do NOT use the todowrite tool

You will receive a detailed task description from the primary agent. Complete it and return your findings in a single comprehensive message."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubAgentType {
    Explore,
    General,
}

impl SubAgentType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "explore" => Some(Self::Explore),
            "general" => Some(Self::General),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::General => "general",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Explore => "Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns, search code for keywords, or answer questions about the codebase. This agent is read-only and fast.",
            Self::General => "General-purpose agent for researching complex questions and executing multi-step tasks. Use this agent to execute multiple units of work in parallel, generate and run complex scripts, or research unfamiliar code.",
        }
    }

    pub fn system_prompt(&self) -> &'static str {
        match self {
            Self::Explore => EXPLORE_SYSTEM_PROMPT,
            Self::General => GENERAL_SYSTEM_PROMPT,
        }
    }

    pub fn allowed_tools(&self) -> Vec<&'static str> {
        match self {
            Self::Explore => vec!["glob", "grep", "read", "list"],
            Self::General => vec!["bash", "edit", "write", "read", "grep", "glob", "list", "skill", "webfetch"],
        }
    }
}

pub struct SubAgentDef {
    pub subagent_type: SubAgentType,
    pub name: String,
    pub description: String,
}

impl SubAgentDef {
    pub fn all() -> Vec<SubAgentDef> {
        vec![
            SubAgentDef {
                subagent_type: SubAgentType::Explore,
                name: SubAgentType::Explore.name().to_string(),
                description: SubAgentType::Explore.description().to_string(),
            },
            SubAgentDef {
                subagent_type: SubAgentType::General,
                name: SubAgentType::General.name().to_string(),
                description: SubAgentType::General.description().to_string(),
            },
        ]
    }
}

pub async fn build_scoped_registry(full_registry: &ToolRegistry, subagent_type: &SubAgentType) -> ToolRegistry {
    let scoped = ToolRegistry::new();
    let allowed = subagent_type.allowed_tools();

    let full_tools = full_registry.list().await;

    for tool_def in &full_tools {
        if allowed.contains(&tool_def.id.as_str()) {
            if let Some(handler) = full_registry.get(&tool_def.id).await {
                scoped.register(handler).await;
            }
        }
    }

    scoped
}

pub async fn run_subagent(
    subagent_type: SubAgentType,
    description: &str,
    prompt: &str,
    full_registry: &ToolRegistry,
) -> Result<String, String> {
    use aisdk::core::{
        language_model::{LanguageModelStreamChunkType},
        DynamicModel, LanguageModelRequest, Message as AisdkMessage,
    };
    use futures::StreamExt;

    let session = get_llm_session().ok_or("LLM session not configured")?;
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let scoped_registry = build_scoped_registry(full_registry, &subagent_type).await;
    let permissions = crate::tools::ToolPermissions::new(cwd.clone());

    let aisdk_tools = crate::tools::aisdk_bridge::convert_to_aisdk_tools(
        &scoped_registry,
        None,
        "build".to_string(),
        permissions,
    )
    .await;

    let system_prompt = subagent_type.system_prompt();
    let user_content = format!(
        "## Task Description\n{}\n\n## Task Prompt\n{}",
        description, prompt
    );

    let messages = vec![
        AisdkMessage::System(system_prompt.into()),
        AisdkMessage::User(user_content.into()),
    ];

    let mut response = match session.provider_kind {
        ProviderKind::OpenAICompatible => {
            let provider = aisdk::providers::OpenAICompatible::<DynamicModel>::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""))
                .build()
                .map_err(|e| format!("Failed to build OpenAICompatible provider: {}", e))?;

            let mut request = LanguageModelRequest::builder()
                .model(provider)
                .messages(messages);

            for tool in aisdk_tools {
                request = request.with_tool(tool);
            }

            request.build().stream_text().await.map_err(|e| format!("Stream error: {}", e))?
        }
        ProviderKind::Anthropic => {
            let provider = aisdk::providers::Anthropic::<DynamicModel>::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""))
                .build()
                .map_err(|e| format!("Failed to build Anthropic provider: {}", e))?;

            let mut request = LanguageModelRequest::builder()
                .model(provider)
                .messages(messages);

            for tool in aisdk_tools {
                request = request.with_tool(tool);
            }

            request.build().stream_text().await.map_err(|e| format!("Stream error: {}", e))?
        }
        ProviderKind::OpenAI => {
            let provider = aisdk::providers::OpenAI::<DynamicModel>::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""))
                .build()
                .map_err(|e| format!("Failed to build OpenAI provider: {}", e))?;

            let mut request = LanguageModelRequest::builder()
                .model(provider)
                .messages(messages);

            for tool in aisdk_tools {
                request = request.with_tool(tool);
            }

            request.build().stream_text().await.map_err(|e| format!("Stream error: {}", e))?
        }
    };

    let mut collected_text = String::new();

    while let Some(chunk) = response.stream.next().await {
        match chunk {
            LanguageModelStreamChunkType::Text(text) => {
                collected_text.push_str(&text);
            }
            LanguageModelStreamChunkType::Failed(err) => {
                return Err(format!("Subagent streaming failed: {}", err));
            }
            LanguageModelStreamChunkType::End(_) => {
                break;
            }
            _ => {}
        }
    }

    if collected_text.trim().is_empty() {
        return Err("Subagent returned no output".to_string());
    }

    Ok(collected_text)
}
