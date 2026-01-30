Product Requirements Document: OpenCode System Prompts Architecture

Executive Summary

OpenCode uses a sophisticated multi-tiered system prompt architecture to deliver contextual agent behavior across different LLM providers and use cases. This PRD documents the complete system prompt ecosystem, including provider-specific optimizations, agent-specific instructions, session management prompts, and the dynamic prompt composition system.

1. System Prompt Architecture Overview

OpenCode employs 4 prompt composition layers:

plaintext

Layer 1: Header (Provider-specific)
↓
Layer 2: Core Instructions (Provider-optimized)
↓
Layer 3: Custom Instructions (User project-specific)
↓
Layer 4: Agent-specific Prompts (Task-specialized)

2. Provider-Specific Prompts (Layer 2: Core)

2.1 Provider Detection

typescript

SystemPrompt.provider(model: Provider.Model): string[] {
if model.api.id includes "gpt-5" → PROMPT_CODEX
if model.api.id includes "gpt-" || "o1" || "o3" → PROMPT_BEAST (GPT-4+)
if model.api.id includes "gemini-" → PROMPT_GEMINI
if model.api.id includes "claude" → PROMPT_ANTHROPIC
else → PROMPT_ANTHROPIC_WITHOUT_TODO (fallback)
}

2.2 GPT Models: "BEAST" Prompt (1,400+ tokens)

Target: GPT-4, o1, o3 models

Key Characteristics:

Autonomy-First: "MUST iterate and keep going until problem is solved"
Extended Reasoning: Encourages thorough thinking, long-form planning
Iterative Workflow: 10-step structured process
Internet Research Mandatory: Forces webfetch for dependency verification
Todo Tracking: Strict emoji-based checklist with status tracking
Code Quality: Rigorous testing, edge case verification, validation loops
Terminal Output: Concise communication via emoji status indicators
Core Directives:

Plan extensively before each function call
Fetch URLs provided by user + discover recursive links
Deeply understand problem via investigation
Research dependencies on internet for accuracy
Develop detailed todo list with emoji status
Make incremental, testable changes
Debug to root cause (not symptoms)
Test frequently after each change
Iterate until problem solved + tests pass
Reflect and validate comprehensively
Output Philosophy:

Concise, casual yet professional tone
Always communicate intent before tool calls
Respond with direct answers + bullet points
Avoid unnecessary explanations
Use emoji for status tracking (✓, ☐, ✗)
Communication Examples:

"Let me fetch the URL you provided to gather more information."
"Ok, I've got all the information I need on the LIFX API and I know how to use it."
"Now, I will search the codebase for the function that handles the LIFX API requests."
"Whelp - I see we have some problems. Let's fix those up."
Notable Features:

Memory System: .github/instructions/memory.instruction.md for persistent user preferences
Git Restrictions: Never auto-commit; requires explicit user request
Sequential Thinking: Available when model supports it
File Rereading Prevention: Avoid re-reading unchanged files
2.3 Claude Models: "ANTHROPIC" Prompt (~1,100 tokens)

Target: Claude 3.x, Claude Sonnet

Key Characteristics:

Task-Driven: "The user will primarily request software engineering tasks"
Task Tracking: Heavy emphasis on TodoWrite tool usage
Conciseness: "Brief answers are best" with complete info
Professional Objectivity: Technical accuracy over validation
Tool Preference: Task tool for codebase exploration (reduces context)
Code References: file_path:line_number notation for navigation
Core Directives:

Use TodoWrite tool VERY frequently
Plan tasks with clear breakdown
Mark todos completed immediately (don't batch)
Minimize output tokens while maintaining quality
Avoid preamble/postamble unless asked
Use Task tool for specialized agent work
Batch independent tool calls in parallel
Use dedicated tools over bash when possible
Output Philosophy:

"Assist with defensive security tasks only"
Keep responses short (< 4 lines typically)
Answer directly without elaboration
No unnecessary explanations post-completion
Provide only requested level of detail
Communication Examples (Extreme Brevity):

plaintext

user: 2 + 2
assistant: 4
user: what command should I run to list files?
assistant: ls
user: what files are in src/?
assistant: [runs ls] foo.c, bar.c, baz.c

Security Policy:

"Assist with defensive security tasks only"
Refuse to create code for malicious purposes
No credential discovery/harvesting assistance
Block SSH key/cookie/wallet bulk crawling
Notable Features:

Help Command: /help displays help info
Feedback: /bug command for issue reporting
Custom Hooks: Treat hook feedback as user feedback
Code Block Explanation: Skip unless requested
2.4 Gemini Models: "GEMINI" Prompt (~2,100 tokens)

Target: Gemini 2, Gemini Pro

Key Characteristics:

Convention-First: "Rigorously adhere to existing project conventions"
Assumption Rejection: NEVER assume library/framework availability
Verification-Heavy: Check imports, config files, neighboring code
Style Mimicry: Match project formatting, naming, architecture
Path Construction: Always use absolute paths
File System Safety: Explain critical commands before execution
Minimal Output: "3 lines max output excluding tool use"
Self-Verification: Include unit tests and debug statements
Core Directives:

Understand via grep/glob (parallel searches)
Build grounded plan based on context
Implement adhering to conventions
Verify with tests if applicable
Execute linting/type-checking commands
Validate against original request
Output Philosophy:

"Adopt professional, direct, concise tone"
Fewer than 3 lines per response
Focus strictly on user's query
No conversational filler or preambles
Format with GitHub-flavored Markdown
Security Rules:

Explain bash commands that modify filesystem
Never introduce code that exposes secrets
Always use absolute paths (relative paths not supported)
Avoid interactive shell commands (non-interactive when possible)
Respect user confirmations—never retry canceled operations
Notable Features:

Interactive Avoidance: Flag commands like git rebase -i
Background Processes: Use & for long-running commands
Path Validation: Must combine project root + relative path
Code Comments: Add sparingly, focus on "why" not "what"
No Reverts: Only revert if user requests or error occurs
2.5 OpenAI Codex: "CODEX" Prompt (~2,500+ tokens, most comprehensive)

Target: GPT-4 Turbo, GPT-4o (when not using BEAST)

Key Characteristics:

Personality: "Concise, direct, friendly" yet highly detailed
AGENTS.md Spec: Hierarchical instruction files with scope precedence
Preamble Guidelines: 1-2 sentence explanations before tool calls
Planning Tool: TodoWrite for non-trivial tasks
Personality Examples:
"I've explored the repo; now checking the API route definitions."
"Next, I'll patch the config and update the related tests."
"Alright, build pipeline order is interesting."
Core Directives:

Obey AGENTS.md files in scope
Keep responses concise, direct, friendly
Send brief preambles before tool calls (8-12 words)
Use todowrite for non-trivial, multi-phase work
Break tasks into meaningful, logically ordered steps
Don't repeat full plan after todowrite
Fix root cause, not surface patches
Keep changes minimal and focused
Validate work via tests/build
Only terminate when problem completely solved
Output Philosophy:

Group related actions in single preamble
Build on prior context for momentum
Keep tone light, friendly, curious
Exception: Skip preambles for trivial single-file reads
Minimal markdown formatting
Planning Guidance:

Use plan tool for non-trivial, multi-phase work
Plans should break task into logical dependencies
Don't pad with obvious steps
Update plans mid-task if needed with explanation
Mark steps completed before moving forward
High-Quality Plan Examples:

plaintext

1. Add CLI entry with file args
2. Parse Markdown via CommonMark library
3. Apply semantic HTML template
4. Handle code blocks, images, links
5. Add error handling for invalid files

File Handling:

Never re-read files after successful edit
Use git log/blame for history context
Never add copyright/license headers
Don't use one-letter variables
Use file_path format for citations (avoid broken 【F:】 syntax)
Approval Modes:

untrusted: Most commands escalated
on-failure: Allow commands; escalate on failure
on-request: Default sandboxed; request when needed
never: Must persist and solve without user approval
Notable Features:

Sequential Thinking: Use when available for complex reasoning
Code References: file_path:line_number for navigation
Task Execution Philosophy: Start specific tests, move to broader
No Unrelated Fixes: Skip unrelated bugs (mention in final message)
Formatting Loops: Up to 3 iterations max for formatting 3. Header Prompts (Layer 1: Provider Detection)

3.1 Anthropic Header

typescript

SystemPrompt.header(providerID: string): string[] {
if providerID.includes("anthropic")
→ [PROMPT_ANTHROPIC_SPOOF.trim()]
else
→ []
}

The Anthropic header is injected when Claude API is detected. It provides context about Claude-specific features.

4. Custom Instructions (Layer 3: User Project Context)

4.1 Custom Instruction File Discovery

OpenCode searches for custom instructions in this priority order:

Local Files (searched bottom-up from project):

AGENTS.md
CLAUDE.md
CONTEXT.md (deprecated)
Global Files:

~/.opencode/AGENTS.md (global config)
~/.claude/CLAUDE.md (if not disabled)
${OPENCODE_CONFIG_DIR}/AGENTS.md
Config-Specified URLs:

Any URLs in config.instructions[] (loaded with 5s timeout)
4.2 Custom Instructions Format

markdown

# AGENTS.md / CLAUDE.md

- Use for coding standards, patterns, project structure
- Scope: entire directory tree rooted at containing folder
- Nested files take precedence
- User instructions override file instructions

  4.3 Environment Context

Injected automatically:

plaintext

<env>
  Working directory: ${Instance.directory}
  Is directory a git repo: yes/no
  Platform: ${process.platform}
  Today's date: ${new Date().toDateString()}
</files>

5. Agent-Specific Prompts (Layer 4: Task Specialization)

5.1 Agent Prompt System

Each agent can have a custom prompt override that specializes behavior:

typescript

Agent.Info {
prompt?: string // Optional custom system prompt
}

Agent-Specific Prompts:

Explore Agent (22 lines)

plaintext

You are a file search specialist. You excel at thoroughly navigating
and exploring codebases.
Guidelines:

- Use Glob for broad file pattern matching
- Use Grep for searching file contents with regex
- Use Read when you know the specific file path
- Use Bash for file operations (copy, move, list)
- Adapt search based on thoroughness level (quick/medium/very thorough)
- Return absolute paths in final response
- Avoid using emojis
- Do not create files or modify system state

Key Features:

Rapid pattern-based navigation
Regex search specialization
Thoroughness level awareness
Read-only constraint
Absolute path requirement
Compaction Agent (13 lines)

plaintext

You are a helpful AI assistant tasked with summarizing conversations.
When asked to summarize, provide detailed but concise summary focusing on:

- What was done
- What is currently being worked on
- Which files are being modified
- What needs to be done next
- Key user requests/constraints/preferences
- Important technical decisions and why

Use Case: Session message truncation for long conversations

Title Agent (44 lines)

plaintext

You are a title generator. You output ONLY a thread title. Nothing else.
Generate a brief title (≤50 chars) that would help user find later.
Rules:

- Never include tool names
- Focus on main topic for retrieval
- Vary phrasing
- When file mentioned, focus on WHAT user wants, not file name
- Keep exact: technical terms, numbers, filenames, HTTP codes
- Remove: the, this, my, a, an
- Never assume tech stack
- If user message short/conversational → reflect tone
  Examples:
  "debug 500 errors in production" → "Debugging production 500 errors"
  "why is app.js failing" → "app.js failure investigation"

Constraints:

Exactly one line output
No explanations
No meta-commentary
No "cannot generate" responses
Summary Agent (12 lines)

plaintext

Summarize what was done in this conversation. Write like PR description.
Rules:

- 2-3 sentences max
- Describe changes made, not process
- Don't mention tests/builds/validation
- Write in first person (I added..., I fixed...)
- Never ask questions
- If question unanswered → preserve exact question
- If imperative request → include exact request

6. Agent Generation Prompt

6.1 Custom Agent Creator Prompt (~76 lines)

OpenCode has a dedicated agent that helps users create custom agents via the "Generate Agent" feature.

Key Sections:

Extract Core Intent
Identify purpose, responsibilities, success criteria
Consider project context from CLAUDE.md
For code review agents: assume recent code, not whole codebase
Design Expert Persona
Create compelling expert identity
Embody domain knowledge
Inspire confidence
Architect Instructions
Clear behavioral boundaries
Specific methodologies and best practices
Anticipate edge cases
Align with project standards
Define output formats
Optimize for Performance
Decision-making frameworks
Quality control mechanisms
Efficient workflows
Clear escalation strategies
Create Identifier
Lowercase, numbers, hyphens only
2-4 words
Clearly indicates function
Memorable, easy to type
Avoid generic terms
Output Format
json

{
"identifier": "code-reviewer",
"whenToUse": "Use this agent when... [include examples]",
"systemPrompt": "You are... [complete prompt]"
}

Agent Generation Examples:

Example 1: Code Review Agent

Context: Review recently written code, not whole codebase
Output: Comprehensive code review with style/best practices
When to Use: After logical code chunks complete
Example 2: Greeting Responder Agent

Context: Respond to user greetings with friendly joke
Output: Friendly joke response
When to Use: When user says "hello", "greeting", "hey" 7. System Prompt Composition Logic

7.1 Full Prompt Stack

typescript

SystemPrompt.compose(session, agent, model, provider) {
// 1. Provider header (if applicable)
parts.push(SystemPrompt.header(provider.id))

// 2. Core provider-specific prompt
parts.push(SystemPrompt.provider(model))

// 3. Environment context
parts.push(await SystemPrompt.environment())

// 4. Custom instructions (AGENTS.md, CLAUDE.md, URLs, config)
parts.push(await SystemPrompt.custom())

// 5. Agent-specific prompt override (if defined)
if (agent.prompt) {
parts.push(agent.prompt)
}

// 6. Agent-specific prompt generator (if applicable)
if (agent.agentID === "explore") {
parts.push(PROMPT_EXPLORE)
}
if (agent.agentID === "compaction") {
parts.push(PROMPT_COMPACTION)
}

return parts.join("\n\n---\n\n")
}

7.2 Composition Order (Priority)

Provider Header (context)
Core Provider Prompt (behavior framework)
Environment (facts about runtime)
Custom Instructions (project-specific rules)
Agent Prompt (specialization)
Later sections can override earlier ones. User/developer instructions override all.

8. Output Truncation Integration

System prompts include guidance on output truncation:

plaintext

// In Beast/Anthropic/Gemini prompts:
"Your output will be displayed on a command line interface.
Your responses should be short and concise (typically < 4 lines,
excluding tool calls)."
// In Codex prompt:
"Minimal Output: Aim for fewer than 3 lines of text output
(excluding tool use/code generation) per response whenever practical."

9. Security & Safety Guardrails (Embedded in Prompts)

9.1 Defensive Security Only

Embedded in: Anthropic (20250930), Anthropic, Claude

plaintext

IMPORTANT: Assist with defensive security tasks only. Refuse to create,
modify, or improve code that may be used maliciously. Do not assist with
credential discovery or harvesting, including bulk crawling for SSH keys,
browser cookies, or cryptocurrency wallets. Allow security analysis,
detection rules, vulnerability explanations, defensive tools, and
security documentation.

9.2 URL Generation Restriction

Embedded in: Anthropic (20250930), Anthropic

plaintext

IMPORTANT: You must NEVER generate or guess URLs for the user unless
you are confident that the URLs are for helping the user with programming.
You may use URLs provided by the user in their messages or local files.

9.3 File System Safety

Embedded in: Gemini, Codex

plaintext

Before executing commands with 'bash' that modify the file system,
codebase, or system state, you _must_ provide a brief explanation of
the command's purpose and potential impact.

10. Non-Functional Requirements

10.1 Prompt Size Management

Provider Typical Size Tokens
Beast (GPT-4) 1,400+ lines ~2,000
Anthropic (Claude) 1,100 lines ~1,400
Gemini 2,100 lines ~2,800
Codex (GPT-4o) 2,500+ lines ~3,200
Strategy:

Provider prompts are static (loaded once)
Custom instructions cached after first load
Agent prompts concatenated at session start
Total system prompt < 20KB typically
10.2 Prompt Loading

Lazy Loading: Agent prompts loaded on first use
Caching: Custom instructions cached after fetch
Timeout: Custom instruction URLs have 5s timeout
Fallback: If custom instruction fetch fails, continue without
10.3 Deterministic Behavior

Same model + agent + project = same prompt composition
No randomization in prompt selection
Prompt version tracking via git tags
No runtime prompt mutation 11. Implementation Strategy

Phase 1: Core (MVP)

Provider detection logic (GPT vs Claude vs Gemini)
Beast prompt for GPT models
Anthropic prompt for Claude
System prompt composition engine
Environment context injection
Phase 2: Extensions

Gemini prompt
Codex prompt
AGENTS.md discovery + parsing
Custom instruction loading
URL instruction fetching
Phase 3: Agent Specialization

Agent-specific prompts (explore, compaction, title, summary)
Custom agent generation (the "Generate Agent" feature)
Agent prompt overrides
Persona builder
Phase 4: Polish

Prompt versioning + change tracking
A/B testing framework for prompts
Telemetry on prompt effectiveness
Performance optimization (caching) 12. Prompt Testing & Validation

12.1 Test Cases

Provider Detection
GPT-5 model → loads CODEX
GPT-4 model → loads BEAST
Claude model → loads ANTHROPIC
Gemini model → loads GEMINI
Composition Order
Header present for Anthropic
Environment context included
Custom instructions prepended
Agent prompt appended
Custom Instructions
AGENTS.md found and loaded
Nested AGENTS.md takes precedence
URL instructions fetched with timeout
Missing files don't break composition
Output Philosophy
Beast: Long-form reasoning + emoji tracking
Anthropic: Minimal output, direct answers
Gemini: Convention-first, <3 lines
Codex: Friendly preambles, logical grouping 13. Key Differences by Provider

Aspect Beast (GPT-4) Anthropic (Claude) Gemini Codex (GPT-4o)
Autonomy Maximum (iterate until solved) Task-driven Convention-driven Balanced
Output Length Long-form encouraged Minimal (<4 lines) Minimal (<3 lines) Moderate (1-2 sentences)
Planning Mandatory upfront TodoWrite frequent Not emphasized Optional, high-quality only
Internet Research Mandatory Optional Optional Optional
Communication Style Casual, emoji-heavy Direct, professional Concise, formal Friendly, conversational
Tool Preference All tools encouraged Task tool priority Grep/Glob focus Balanced with favorites
Error Handling Retry until solved Move forward Skip unrelated Move forward
Prompt Size Large (1400+ lines) Medium (1100 lines) Very large (2100+ lines) Largest (2500+ lines) 14. Future Enhancements

Dynamic Prompt Generation: AI-powered prompt generation based on project analysis
Prompt Versioning: Git-based versioning + rollback capability
Multi-Language Support: Translate system prompts to user's preferred language
Model-Specific Optimizations: Fine-tune prompts for new model releases
Telemetry: Track which prompts perform best for analytics
Prompt Marketplace: Community-contributed prompts for specific tasks
Adaptive Prompts: Adjust verbosity/style based on session history
This PRD captures opencode's sophisticated multi-tiered prompt system. The key insight is that opencode doesn't use one system prompt—it composes prompts dynamically based on the provider, agent type, project context, and user preferences. This allows it to optimize for each model's strengths while maintaining consistent user experience.
