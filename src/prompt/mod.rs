use crate::tools::ToolRegistry;

mod rules;

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
    print_mode: bool,
    tool_registry: Option<ToolRegistry>,
    agent_registry: Option<crate::agent::definition::AgentRegistry>,
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
            print_mode: false,
            tool_registry: None,
            agent_registry: None,
        }
    }

    pub fn with_tool_registry(mut self, registry: ToolRegistry) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    pub fn with_agent_registry(
        mut self,
        registry: crate::agent::definition::AgentRegistry,
    ) -> Self {
        self.agent_registry = Some(registry);
        self
    }

    pub fn with_print_mode(mut self, print_mode: bool) -> Self {
        self.print_mode = print_mode;
        self
    }

    pub async fn compose(&self) -> String {
        let mut parts = Vec::new();

        parts.push(self.get_header());
        parts.push(self.get_core_prompt());
        if self.print_mode {
            parts.push(self.get_print_mode_context());
        }
        parts.push(self.get_environment_context());

        if let Some(ref registry) = self.tool_registry {
            parts.push(self.get_tools_context(registry).await);
        }

        parts.push(self.get_custom_instructions().await);

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
        r#"You are Codex, based on GPT-5. You are running as a coding agent in Crabcode on the user's computer.

Personality:
- Be concise, direct, and friendly.
- Communicate efficiently and keep the user informed about ongoing actions.
- Prioritize actionable guidance, assumptions, prerequisites, and next steps.
- Avoid unnecessary detail unless the user asks for it.

Autonomy and Persistence:
- Persist until the task is fully handled end-to-end within the current turn whenever feasible.
- Do not stop at analysis, partial fixes, or incomplete wiring.
- Carry work through implementation, verification, and a clear explanation of outcomes unless the user explicitly pauses or redirects you.
- Unless the user explicitly asks for a plan, asks a question about the code, or is brainstorming, assume they want you to make code changes or run tools to solve the problem.
- If code changes are expected, do not stop at a proposed solution in chat; implement the change.
- If you hit a blocker, try to resolve it with available tools before yielding.
- Only terminate when you are sure the problem is solved or you have a concrete blocker to report.

Progress Updates and Final Answers:
- Send brief preambles before grouped tool calls.
- Treat preambles and progress updates as interim commentary before tool calls.
- Never send a preamble or progress update as the final answer.
- If work remains, continue with tools instead of sending a final answer.
- Use final answers only when the requested work is complete, verified when practical, and ready to hand back.
- Keep final answers concise and focused on what changed, validation run, and any real blocker.
- For routine code changes, prefer one or two compact sentences plus validation; do not list every edited file unless that detail is needed.
- Once the final answer is complete, stop instead of continuing with extra explanation.

Planning:
- Use update_plan for non-trivial, multi-phase work
- Plans should break the task into meaningful, logically ordered steps that are easy to verify.
- Do not pad simple work with filler steps or obvious actions.
- Do not repeat the full plan after update_plan; the UI already displays it.
- Before starting the next planned step, mark the previous step completed.
- Maintain exactly one in_progress item at a time.
- Do not jump an item from pending directly to completed; set it to in_progress first.
- Update the plan if scope changes, steps split/merge/reorder, or you discover new work.
- Do not let the plan go stale while coding.
- Finish with all plan items completed or explicitly canceled/deferred before ending the turn.
- After update_plan succeeds, proceed with the next concrete tool call; do not call update_plan again unless the plan content or statuses changed.

Task Execution:
- Fix the problem at the root cause rather than applying surface-level patches when possible.
- Keep changes minimal and focused on the user's request.
- Respect the existing codebase style and local patterns.
- Do not fix unrelated bugs or broken tests; mention them if relevant.
- Do not git commit or create branches unless explicitly requested.
- Never add copyright or license headers unless requested.
- Prefer rg/ripgrep for search and targeted file reads for named files.
- Avoid repeating identical reads, searches, or validation commands.
- Do not re-read files solely to confirm a successful edit.

Validation:
- If tests/builds/formatters exist, use focused validation for the changed area first.
- Add or update tests when the codebase has adjacent test patterns and the behavioral risk warrants it.
- Do not add a test framework or formatter to a codebase that does not already use one.
- If validation fails for unrelated reasons, do not fix unrelated issues; report the residual risk.

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

    fn get_print_mode_context(&self) -> String {
        r#"Non-Interactive Print Mode:
- Keep planning internal; do not call update_plan.
- Do not ask the user questions or wait for interactive input.
- Prefer direct read/apply_patch/edit/bash tool use.
- For existing-file edits, prefer apply_patch or edit over rewriting whole files; use write_files mainly for new files or true full rewrites.
- After tests pass, do not run optional one-off formatters or package-manager commands unless the project has an explicit formatter script or the user asked for it.
- After requested validation passes, send a compact final answer and stop."#
            .to_string()
    }

    async fn get_tools_context(&self, registry: &ToolRegistry) -> String {
        let schemas = registry.list_schemas().await;

        if schemas.is_empty() {
            return String::new();
        }

        let tools_json =
            serde_json::to_string_pretty(&schemas).unwrap_or_else(|_| "[]".to_string());

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

    async fn get_custom_instructions(&self) -> String {
        let mut instructions = rules::get_custom_instructions(&self.working_directory).await;

        // Add available skills listing
        if let Some(store) = crate::skill::get_skill_store() {
            let skills = store.all();
            if !skills.is_empty() {
                let skills_xml = skills
                    .iter()
                    .map(|s| {
                        format!(
                            "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>file://{}</location>\n  </skill>",
                            s.name,
                            s.description.as_deref().unwrap_or(""),
                            s.location.display()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                let skills_block = format!(
                    "\n\nSkills provide specialized instructions and workflows for specific tasks.\n\
                     Use the skill tool to load a skill when a task matches its description.\n\
                     <available_skills>\n{}\n</available_skills>",
                    skills_xml
                );

                if !instructions.is_empty() {
                    instructions.push_str("\n\n");
                }
                instructions.push_str(&skills_block);
            }
        }

        // Add available subagents listing
        let registry = self
            .agent_registry
            .clone()
            .unwrap_or_else(crate::agent::definition::AgentRegistry::default);
        let subagents = registry.visible_subagents();
        if !subagents.is_empty() {
            let subagents_xml = subagents
                .iter()
                .map(|s| {
                    format!(
                        "  <subagent>\n    <name>{}</name>\n    <description>{}</description>\n  </subagent>",
                        s.name, s.description
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            let subagents_block = format!(
                "\n\n<available_subagents>\n{}\n</available_subagents>",
                subagents_xml
            );

            if !instructions.is_empty() {
                instructions.push_str("\n\n");
            }
            instructions.push_str(&subagents_block);
        }

        instructions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_detection() {
        assert_eq!(ProviderType::from_model_id("gpt-4"), ProviderType::OpenAI);
        assert_eq!(ProviderType::from_model_id("gpt-5"), ProviderType::Codex);
        assert_eq!(
            ProviderType::from_model_id("claude-3"),
            ProviderType::Anthropic
        );
        assert_eq!(
            ProviderType::from_model_id("gemini-pro"),
            ProviderType::Gemini
        );
        assert_eq!(
            ProviderType::from_model_id("unknown"),
            ProviderType::Generic
        );
    }

    #[test]
    fn codex_prompt_separates_progress_from_final_answers() {
        let composer = SystemPromptComposer::new("gpt-5", ".", true, "test");
        let prompt = composer.get_codex_prompt();

        assert!(prompt.contains("preambles and progress updates as interim commentary"));
        assert!(prompt.contains("Use final answers only when the requested work is complete"));
        assert!(prompt.contains("continue with tools instead of sending a final answer"));
        assert!(prompt.contains("Persist until the task is fully handled end-to-end"));
        assert!(prompt.contains("Do not stop at analysis, partial fixes, or incomplete wiring"));
        assert!(prompt.contains("Do not let the plan go stale while coding"));
        assert!(
            prompt.contains("Finish with all plan items completed or explicitly canceled/deferred")
        );
        assert!(prompt.contains("do not call update_plan again unless the plan content"));
        assert!(prompt.contains("do not stop at a proposed solution in chat"));
    }

    #[test]
    fn print_mode_context_disables_interactive_planning() {
        let composer = SystemPromptComposer::new("gpt-5", ".", true, "test").with_print_mode(true);
        let context = composer.get_print_mode_context();

        assert!(context.contains("do not call update_plan"));
        assert!(context.contains("Do not ask the user questions"));
        assert!(context.contains("apply_patch"));
        assert!(context.contains("write_files"));
        assert!(context.contains("one-off formatters"));
        assert!(context.contains("stop"));
    }
}
