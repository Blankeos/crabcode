use crate::agent::config::{get_llm_session, ProviderKind};
use crate::tools::ToolRegistry;

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
- Do NOT use the update_plan tool

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
            Self::General => vec![
                "bash", "edit", "write", "read", "grep", "glob", "list", "skill", "webfetch",
            ],
        }
    }
}

pub struct SubAgentDef {
    pub subagent_type: SubAgentType,
    pub name: String,
    pub description: String,
}

pub struct SubAgentRunResult {
    pub output: String,
    pub tool_call_count: usize,
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

pub async fn build_scoped_registry(
    full_registry: &ToolRegistry,
    subagent_type: &SubAgentType,
) -> ToolRegistry {
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
    sender: Option<crate::llm::ChunkSender>,
    session_id: String,
) -> Result<SubAgentRunResult, String> {
    use aisdk::core::{
        chunk::ChunkType,
        response::{stream_with_tools, StreamTextResponse},
        Message as AisdkMessage,
    };
    use aisdk::{Anthropic, OpenAI, OpenAICompatible};
    use futures::StreamExt;
    use std::collections::HashMap;

    let session = get_llm_session().ok_or("LLM session not configured")?;
    let cwd = crate::utils::cwd::current_dir_or_dot();

    let scoped_registry = build_scoped_registry(full_registry, &subagent_type).await;
    let permissions = crate::tools::ToolPermissions::new(cwd.clone());

    let aisdk_tools = crate::tools::aisdk_bridge::convert_to_aisdk_tools(
        &scoped_registry,
        sender.clone(),
        "build".to_string(),
        permissions,
        Some(session_id.clone()),
        None,
    )
    .await;

    let system_prompt = subagent_type.system_prompt();
    let user_content = format!(
        "## Task Description\n{}\n\n## Task Prompt\n{}",
        description, prompt
    );

    let messages = vec![
        AisdkMessage::system(system_prompt),
        AisdkMessage::user(user_content),
    ];

    let headers = HashMap::new();
    let stream_started_at = std::time::Instant::now();
    crate::emit_log!(
        "[SUBAGENT] stream_start session_id={} subagent_type={} tools={} description_bytes={} prompt_bytes={} sender_present={}",
        session_id,
        subagent_type.name(),
        aisdk_tools.len(),
        description.len(),
        prompt.len(),
        sender.is_some()
    );

    let mut response: StreamTextResponse = match session.provider_kind {
        ProviderKind::OpenAICompatible => {
            let mut builder = OpenAICompatible::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""));
            if let Some(effort) = session.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            let provider = builder
                .build()
                .map_err(|e| format!("Failed to build OpenAICompatible provider: {}", e))?;

            stream_with_tools(provider, messages, aisdk_tools, None, None, headers)
                .await
                .map_err(|e| format!("Stream error: {}", e))?
        }
        ProviderKind::Anthropic => {
            let mut builder = Anthropic::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""));
            if let Some(effort) = session.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            let provider = builder
                .build()
                .map_err(|e| format!("Failed to build Anthropic provider: {}", e))?;

            stream_with_tools(provider, messages, aisdk_tools, None, None, headers)
                .await
                .map_err(|e| format!("Stream error: {}", e))?
        }
        ProviderKind::OpenAI => {
            let mut builder = OpenAI::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""));
            if let Some(effort) = session.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            let provider = builder
                .build()
                .map_err(|e| format!("Failed to build OpenAI provider: {}", e))?;

            stream_with_tools(provider, messages, aisdk_tools, None, None, headers)
                .await
                .map_err(|e| format!("Stream error: {}", e))?
        }
    };

    let mut collected_text = String::new();
    let mut tool_call_count = 0usize;

    while let Some(chunk) = response.stream.next().await {
        match chunk {
            ChunkType::Text(text) => {
                collected_text.push_str(&text);
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Text(text));
                }
            }
            ChunkType::Reasoning(reasoning) => {
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Reasoning(reasoning));
                }
            }
            ChunkType::ToolCall(tool_call) => {
                let calls = serde_json::from_str::<serde_json::Value>(&tool_call)
                    .ok()
                    .and_then(|value| value.as_array().map(|items| items.len()))
                    .unwrap_or(1);
                tool_call_count = tool_call_count.saturating_add(calls);
            }
            ChunkType::Failed(err) => {
                crate::emit_log!(
                    "[SUBAGENT] stream_failed session_id={} subagent_type={} duration_ms={} error={}",
                    session_id,
                    subagent_type.name(),
                    stream_started_at.elapsed().as_millis(),
                    err
                );
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Failed(err.clone()));
                }
                return Err(format!("Subagent streaming failed: {}", err));
            }
            ChunkType::End(_) => {
                break;
            }
            ChunkType::ResponseCompleted { .. } => {
                break;
            }
            ChunkType::Metadata(message) => {
                crate::emit_log!(
                    "[SUBAGENT_METADATA] session_id={} subagent_type={} {}",
                    session_id,
                    subagent_type.name(),
                    message
                );
            }
            _ => {}
        }
    }

    let stop_reason = response.stop_reason().await;
    crate::emit_log!(
        "[SUBAGENT] stream_finish session_id={} subagent_type={} duration_ms={} stop_reason={:?} text_bytes={} tool_call_count={}",
        session_id,
        subagent_type.name(),
        stream_started_at.elapsed().as_millis(),
        stop_reason,
        collected_text.len(),
        tool_call_count
    );

    Ok(SubAgentRunResult {
        output: normalize_subagent_output(collected_text),
        tool_call_count,
    })
}

fn normalize_subagent_output(output: String) -> String {
    if output.trim().is_empty() {
        "Subagent completed without a final text response.".to_string()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_subagent_output;

    #[test]
    fn empty_subagent_output_is_not_an_error_payload() {
        assert_eq!(
            normalize_subagent_output("   \n".to_string()),
            "Subagent completed without a final text response."
        );
    }

    #[test]
    fn non_empty_subagent_output_is_preserved() {
        assert_eq!(
            normalize_subagent_output("Hi there".to_string()),
            "Hi there"
        );
    }
}
