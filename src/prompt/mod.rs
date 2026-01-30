use crate::tools::ToolRegistry;

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderType {
    OpenAI,
    Anthropic,
    Gemini,
    Codex,
    Generic,
}

impl ProviderType {
    pub fn from_model_id(model_id: &str) -> Self {
        let lower = model_id.to_lowercase();
        
        if lower.contains("gpt-5") {
            ProviderType::Codex
        } else if lower.contains("gpt-") || lower.contains("o1") || lower.contains("o3") {
            ProviderType::OpenAI
        } else if lower.contains("gemini-") {
            ProviderType::Gemini
        } else if lower.contains("claude") {
            ProviderType::Anthropic
        } else {
            ProviderType::Generic
        }
    }
}

pub struct SystemPromptComposer {
    provider_type: ProviderType,
    working_directory: String,
    is_git_repo: bool,
    platform: String,
    tool_registry: Option<ToolRegistry>,
}

impl SystemPromptComposer {
    pub fn new(
        model_id: &str,
        working_directory: impl Into<String>,
        is_git_repo: bool,
        platform: impl Into<String>,
    ) -> Self {
        Self {
            provider_type: ProviderType::from_model_id(model_id),
            working_directory: working_directory.into(),
            is_git_repo,
            platform: platform.into(),
            tool_registry: None,
        }
    }

    pub fn with_tool_registry(mut self, registry: ToolRegistry) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    pub async fn compose(&self,
    ) -> String {
        let mut parts = Vec::new();

        parts.push(self.get_header());
        parts.push(self.get_core_prompt());
        parts.push(self.get_environment_context());
        
        if let Some(ref registry) = self.tool_registry {
            parts.push(self.get_tools_context(registry).await);
        }

        parts.push(self.get_custom_instructions());

        parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    }

    fn get_header(&self) -> String {
        match self.provider_type {
            ProviderType::Anthropic => {
                "You are Claude, an AI assistant made by Anthropic.".to_string()
            }
            _ => String::new(),
        }
    }

    fn get_core_prompt(&self) -> String {
        match self.provider_type {
            ProviderType::OpenAI => self.get_beast_prompt(),
            ProviderType::Anthropic => self.get_anthropic_prompt(),
            ProviderType::Gemini => self.get_gemini_prompt(),
            ProviderType::Codex => self.get_codex_prompt(),
            ProviderType::Generic => self.get_anthropic_prompt(),
        }
    }

    fn get_beast_prompt(&self) -> String {
        r#"You are an expert software engineer. You MUST iterate and keep going until the problem is solved.

Core Directives:
- Plan extensively before each function call
- Fetch URLs provided by user + discover recursive links
- Deeply understand problem via investigation
- Research dependencies on internet for accuracy
- Make incremental, testable changes
- Debug to root cause (not symptoms)
- Test frequently after each change
- Iterate until problem solved + tests pass
- Reflect and validate comprehensively

Output Philosophy:
- Concise, casual yet professional tone
- Always communicate intent before tool calls
- Respond with direct answers + bullet points
- Avoid unnecessary explanations
- Use emoji for status tracking (✓, ☐, ✗)

Communication Examples:
- "Let me fetch the URL you provided to gather more information."
- "Ok, I've got all the information I need."
- "Now, I will search the codebase for the relevant function."
- "Whelp - I see we have some problems. Let's fix those up."

Security:
- Assist with defensive security tasks only
- Refuse to create code for malicious purposes
- Never auto-commit; requires explicit user request

Your output will be displayed on a command line interface. Your responses should be short and concise (typically < 4 lines, excluding tool calls)."#.to_string()
    }

    fn get_anthropic_prompt(&self) -> String {
        r#"The user will primarily request software engineering tasks.

Core Directives:
- Plan tasks with clear breakdown
- Mark todos completed immediately (don't batch)
- Minimize output tokens while maintaining quality
- Avoid preamble/postamble unless asked
- Batch independent tool calls in parallel
- Use dedicated tools over bash when possible
- Keep responses short (< 4 lines typically)
- Answer directly without elaboration
- No unnecessary explanations post-completion
- Provide only requested level of detail

Security:
- Assist with defensive security tasks only
- Refuse to create code for malicious purposes
- No credential discovery/harvesting assistance

When referencing specific functions or pieces of code, include the pattern `file_path:line_number` to allow the user to easily navigate to the source code location.

Your output will be displayed on a command line interface. Your responses should be short and concise (typically < 4 lines, excluding tool calls)."#.to_string()
    }

    fn get_gemini_prompt(&self) -> String {
        r#"You are an expert software engineer. Rigorously adhere to existing project conventions.

Core Directives:
- Understand via grep/glob (parallel searches)
- Build grounded plan based on context
- Implement adhering to conventions
- Verify with tests if applicable
- Execute linting/type-checking commands
- Validate against original request

Output Philosophy:
- Adopt professional, direct, concise tone
- Fewer than 3 lines per response
- Focus strictly on user's query
- No conversational filler or preambles
- Format with GitHub-flavored Markdown

Security:
- Explain bash commands that modify filesystem
- Never introduce code that exposes secrets
- Always use absolute paths
- Avoid interactive shell commands

Your output will be displayed on a command line interface. Your responses should be short and concise (typically < 4 lines, excluding tool calls)."#.to_string()
    }

    fn get_codex_prompt(&self) -> String {
        r#"You are an expert software engineer with a concise, direct, friendly personality.

Core Directives:
- Keep responses concise, direct, friendly
- Send brief preambles before tool calls (8-12 words)
- Break tasks into meaningful, logically ordered steps
- Don't repeat full plan after todowrite
- Fix root cause, not surface patches
- Keep changes minimal and focused
- Validate work via tests/build
- Only terminate when problem completely solved

Output Philosophy:
- Group related actions in single preamble
- Build on prior context for momentum
- Keep tone light, friendly, curious
- Exception: Skip preambles for trivial single-file reads
- Minimal markdown formatting

Planning:
- Use plan tool for non-trivial, multi-phase work
- Plans should break task into logical dependencies
- Don't pad with obvious steps
- Update plans mid-task if needed with explanation
- Mark steps completed before moving forward

File Handling:
- Never re-read files after successful edit
- Use git log/blame for history context
- Never add copyright/license headers
- Don't use one-letter variables
- Use file_path format for citations

Your output will be displayed on a command line interface. Your responses should be short and concise (typically < 4 lines, excluding tool calls)."#.to_string()
    }

    fn get_environment_context(&self) -> String {
        let git_status = if self.is_git_repo { "yes" } else { "no" };
        let date = chrono::Local::now().format("%a %b %d %Y").to_string();
        
        format!(
            r#"<env>
  Working directory: {}
  Is directory a git repo: {}
  Platform: {}
  Today's date: {}
 </env>"#,
            self.working_directory, git_status, self.platform, date
        )
    }

    async fn get_tools_context(&self,
        registry: &ToolRegistry,
    ) -> String {
        let schemas = registry.list_schemas().await;
        
        if schemas.is_empty() {
            return String::new();
        }

        let tools_json = serde_json::to_string_pretty(&schemas)
            .unwrap_or_else(|_| "[]".to_string());

        format!(
            r#"You have access to the following tools (JSON schema):

{}

Tool use:
- Use the model's built-in tool/function calling mechanism (do not print tool calls as text).
- If you need file contents, directory listings, running commands, or edits, call the appropriate tool.
- After tool results are returned, use them to answer.
"#,
            tools_json
        )
    }

    fn get_custom_instructions(&self) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_detection() {
        assert_eq!(ProviderType::from_model_id("gpt-4"), ProviderType::OpenAI);
        assert_eq!(ProviderType::from_model_id("gpt-5"), ProviderType::Codex);
        assert_eq!(ProviderType::from_model_id("claude-3"), ProviderType::Anthropic);
        assert_eq!(ProviderType::from_model_id("gemini-pro"), ProviderType::Gemini);
        assert_eq!(ProviderType::from_model_id("unknown"), ProviderType::Generic);
    }
}
