use crate::tools::{
    expand_permission_pattern, PermissionPolicyAction, PermissionRule, PermissionRules,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const EXPLORE_SYSTEM_PROMPT: &str = r#"You are a fast, read-only code exploration agent. Your job is to search codebases, find files, and answer questions about code structure.

TOOLS AVAILABLE:
- glob: Find files by pattern matching
- grep: Search file contents using regex
- read: Read file contents with pagination
- list: List directory contents

IMPORTANT RULES:
- Only use the tools listed above (glob, grep, read, list)
- Search in parallel when possible (use multiple tool calls at once)
- Be targeted — prefer a few high-signal greps and precise reads; do not broaden scope and over-explore, and do not re-read the same region without new evidence
- Stop as soon as you can answer the task; avoid open-ended mapping of the whole codebase
- Return a single concise message: relevant paths, line refs, and short excerpts the parent needs
- Focus on precise code locations (file paths and line numbers)
- If still unclear after a focused search, report what you found and what is missing — do not keep exploring indefinitely
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

const VLM_SYSTEM_PROMPT: &str = r#"You are a focused vision analysis subagent.

Your job is to inspect local image files provided by the primary agent and return a concise, useful visual analysis.

Rules:
- Always use the view_image tool for every image path you are asked to analyze.
- Do not edit files or run unrelated tools.
- If there are multiple images, label your observations by path.
- Return only the visual findings the primary agent needs in order to answer the user.
- Be specific about text, UI elements, logos, layout, colors, and visible state when relevant."#;

pub const VLM_AGENT_NAME: &str = "vlm-agent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Primary,
    Subagent,
    All,
}

impl AgentMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "primary" => Some(Self::Primary),
            "subagent" => Some(Self::Subagent),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Subagent => "subagent",
            Self::All => "all",
        }
    }

    pub fn can_run_as_subagent(self) -> bool {
        matches!(self, Self::Subagent | Self::All)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub mode: AgentMode,
    mode_explicit: bool,
    pub hidden: bool,
    hidden_explicit: bool,
    pub model: Option<String>,
    pub reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    reasoning_effort_explicit: bool,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_steps: Option<usize>,
    pub tools: Option<Vec<String>>,
    pub permissions: PermissionRules,
    pub task_permissions: PermissionRules,
    pub instructions: Option<String>,
}

impl AgentDefinition {
    pub fn normalized_name(name: &str) -> String {
        name.trim().to_ascii_lowercase()
    }

    pub fn visible_subagent(&self) -> bool {
        self.mode.can_run_as_subagent() && !self.hidden
    }

    pub fn can_invoke(&self, target: &str) -> bool {
        let rules = if self.task_permissions.is_empty() {
            self.permissions
                .iter()
                .filter(|rule| rule.permission == "task" || rule.permission == "*")
                .cloned()
                .collect::<Vec<_>>()
        } else {
            self.task_permissions.clone()
        };

        if rules.is_empty() {
            return true;
        }

        let target = target.trim().to_ascii_lowercase();
        let mut decision = None;
        for rule in rules {
            if !matches!(rule.permission.as_str(), "task" | "*") {
                continue;
            }
            if crate::tools::permission::wildcard_match(&target, &rule.pattern)
                || crate::tools::permission::wildcard_match("*", &rule.pattern)
            {
                decision = Some(rule.action);
            }
        }

        !matches!(decision, Some(PermissionPolicyAction::Deny))
    }

    fn merge(mut self, overlay: AgentDefinition) -> Self {
        if !overlay.description.is_empty() {
            self.description = overlay.description;
        }
        if overlay.mode_explicit {
            self.mode = overlay.mode;
        }
        if overlay.hidden_explicit {
            self.hidden = overlay.hidden;
        }
        self.model = overlay.model.or(self.model);
        if overlay.reasoning_effort_explicit {
            self.reasoning_effort = overlay.reasoning_effort;
            self.reasoning_effort_explicit = true;
        }
        self.temperature = overlay.temperature.or(self.temperature);
        self.top_p = overlay.top_p.or(self.top_p);
        self.max_steps = overlay.max_steps.or(self.max_steps);
        self.tools = overlay.tools.or(self.tools);
        if !overlay.permissions.is_empty() {
            self.permissions.extend(overlay.permissions);
        }
        if !overlay.task_permissions.is_empty() {
            self.task_permissions.extend(overlay.task_permissions);
        }
        self.instructions = overlay.instructions.or(self.instructions);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRegistry {
    agents: BTreeMap<String, AgentDefinition>,
    default_agent: String,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::builtin(None)
    }
}

impl AgentRegistry {
    pub fn builtin(default_agent: Option<&str>) -> Self {
        let mut registry = Self {
            agents: BTreeMap::new(),
            default_agent: normalize_agent_ref(default_agent.unwrap_or("build")),
        };

        for agent in builtin_agents() {
            registry.upsert(agent);
        }

        if !registry.agents.contains_key(&registry.default_agent) {
            registry.default_agent = "build".to_string();
        }
        registry
    }

    pub fn with_definitions(
        default_agent: Option<&str>,
        definitions: impl IntoIterator<Item = AgentDefinition>,
    ) -> Self {
        let mut registry = Self::builtin(default_agent);
        for definition in definitions {
            registry.upsert(definition);
        }
        if !registry.agents.contains_key(&registry.default_agent) {
            registry.default_agent = "build".to_string();
        }
        registry
    }

    pub fn upsert(&mut self, mut definition: AgentDefinition) {
        let key = AgentDefinition::normalized_name(&definition.name);
        if key.is_empty() {
            return;
        }
        definition.name = key.clone();
        if let Some(existing) = self.agents.remove(&key) {
            self.agents.insert(key, existing.merge(definition));
        } else {
            self.agents.insert(key, definition);
        }
    }

    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.get(&AgentDefinition::normalized_name(name))
    }

    pub fn default_agent(&self) -> &str {
        &self.default_agent
    }

    pub fn primary_agent(&self, name: &str) -> Option<&AgentDefinition> {
        self.get(name)
            .filter(|agent| matches!(agent.mode, AgentMode::Primary | AgentMode::All))
    }

    pub fn visible_primary_agents(&self) -> Vec<&AgentDefinition> {
        self.agents
            .values()
            .filter(|agent| matches!(agent.mode, AgentMode::Primary | AgentMode::All))
            .filter(|agent| !agent.hidden)
            .collect()
    }

    pub fn visible_primary_agent_names(&self) -> Vec<String> {
        self.visible_primary_agents()
            .into_iter()
            .map(|agent| agent.name.clone())
            .collect()
    }

    pub fn task_target(&self, name: &str) -> Option<&AgentDefinition> {
        self.get(name)
            .filter(|agent| agent.mode.can_run_as_subagent())
            .filter(|agent| !is_unconfigured_vlm_agent(agent))
    }

    pub fn can_agent_invoke(&self, parent: &str, target: &str) -> bool {
        let Some(target_agent) = self.task_target(target) else {
            return false;
        };
        let parent_agent = self.get(parent);
        parent_agent.is_none_or(|agent| agent.can_invoke(&target_agent.name))
    }

    pub fn visible_subagents(&self) -> Vec<&AgentDefinition> {
        self.agents
            .values()
            .filter(|agent| agent.visible_subagent())
            .filter(|agent| !is_unconfigured_vlm_agent(agent))
            .collect()
    }

    pub fn visible_agent_names_for_mentions(&self) -> Vec<String> {
        self.visible_subagents()
            .into_iter()
            .map(|agent| agent.name.clone())
            .collect()
    }

    pub fn tool_policy_map(&self) -> HashMap<String, Vec<String>> {
        self.agents
            .iter()
            .filter_map(|(name, agent)| agent.tools.clone().map(|tools| (name.clone(), tools)))
            .collect()
    }

    pub fn permission_rules_map(&self) -> HashMap<String, PermissionRules> {
        self.agents
            .iter()
            .filter(|(_, agent)| !agent.permissions.is_empty())
            .map(|(name, agent)| (name.clone(), agent.permissions.clone()))
            .collect()
    }

    pub fn max_steps_map(&self) -> HashMap<String, usize> {
        self.agents
            .iter()
            .filter_map(|(name, agent)| agent.max_steps.map(|steps| (name.clone(), steps)))
            .collect()
    }
}

fn is_unconfigured_vlm_agent(agent: &AgentDefinition) -> bool {
    agent.name == VLM_AGENT_NAME
        && agent
            .model
            .as_deref()
            .is_none_or(|model| model.trim().is_empty())
}

pub fn parse_agent_definitions_from_config(
    value: Option<&Value>,
    warnings: &mut Vec<String>,
) -> Vec<AgentDefinition> {
    let Some(Value::Object(agents)) = value else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (name, value) in agents {
        match parse_agent_definition(name, value, None, warnings, &format!("agent.{}", name)) {
            Some(agent) => out.push(agent),
            None => continue,
        }
    }
    out
}

pub fn load_markdown_agent_definitions(
    paths: &[PathBuf],
    warnings: &mut Vec<String>,
) -> Vec<AgentDefinition> {
    let mut out = Vec::new();
    for path in paths {
        match load_markdown_agent_definition(path, warnings) {
            Some(agent) => out.push(agent),
            None => continue,
        }
    }
    out
}

fn builtin_agents() -> Vec<AgentDefinition> {
    vec![
        AgentDefinition {
            name: "build".to_string(),
            description: "The default agent. Executes tools based on configured permissions."
                .to_string(),
            mode: AgentMode::Primary,
            mode_explicit: true,
            hidden: false,
            hidden_explicit: true,
            model: None,
            reasoning_effort: None,
            reasoning_effort_explicit: false,
            temperature: None,
            top_p: None,
            max_steps: None,
            tools: Some(vec!["*".to_string()]),
            permissions: Vec::new(),
            task_permissions: Vec::new(),
            instructions: None,
        },
        AgentDefinition {
            name: "plan".to_string(),
            description: "Plan mode. Read-only by default, with Task limited to read-only agents."
                .to_string(),
            mode: AgentMode::Primary,
            mode_explicit: true,
            hidden: false,
            hidden_explicit: true,
            model: None,
            reasoning_effort: None,
            reasoning_effort_explicit: false,
            temperature: None,
            top_p: None,
            max_steps: None,
            tools: Some(vec![
                "glob".to_string(),
                "grep".to_string(),
                "list".to_string(),
                "read".to_string(),
                "view_image".to_string(),
                "skill".to_string(),
                "webfetch".to_string(),
                "question".to_string(),
                "update_plan".to_string(),
                "task".to_string(),
            ]),
            permissions: Vec::new(),
            task_permissions: vec![
                PermissionRule {
                    permission: "task".to_string(),
                    pattern: "*".to_string(),
                    action: PermissionPolicyAction::Deny,
                },
                PermissionRule {
                    permission: "task".to_string(),
                    pattern: "explore".to_string(),
                    action: PermissionPolicyAction::Allow,
                },
            ],
            instructions: None,
        },
        AgentDefinition {
            name: "general".to_string(),
            description: "General-purpose agent for researching complex questions and executing multi-step tasks. Use this agent to execute multiple units of work in parallel.".to_string(),
            mode: AgentMode::Subagent,
            mode_explicit: true,
            hidden: false,
            hidden_explicit: true,
            model: None,
            reasoning_effort: None,
            reasoning_effort_explicit: false,
            temperature: None,
            top_p: None,
            max_steps: None,
            tools: Some(vec!["*".to_string()]),
            permissions: Vec::new(),
            task_permissions: Vec::new(),
            instructions: Some(GENERAL_SYSTEM_PROMPT.to_string()),
        },
        AgentDefinition {
            name: "explore".to_string(),
            description: "Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns, search code for keywords, or answer questions about the codebase. This agent is read-only and fast.".to_string(),
            mode: AgentMode::Subagent,
            mode_explicit: true,
            hidden: false,
            hidden_explicit: true,
            model: None,
            reasoning_effort: None,
            reasoning_effort_explicit: false,
            temperature: None,
            top_p: None,
            max_steps: None,
            tools: Some(vec![
                "glob".to_string(),
                "grep".to_string(),
                "read".to_string(),
                "list".to_string(),
            ]),
            permissions: Vec::new(),
            task_permissions: Vec::new(),
            instructions: Some(EXPLORE_SYSTEM_PROMPT.to_string()),
        },
        AgentDefinition {
            name: VLM_AGENT_NAME.to_string(),
            description: "Analyze local images for models that cannot receive image input directly. Configure agent.vlm-agent.model to enable this fallback."
                .to_string(),
            mode: AgentMode::Subagent,
            mode_explicit: true,
            hidden: false,
            hidden_explicit: true,
            model: None,
            reasoning_effort: None,
            reasoning_effort_explicit: false,
            temperature: None,
            top_p: None,
            max_steps: None,
            tools: Some(vec!["view_image".to_string()]),
            permissions: vec![PermissionRule {
                permission: "external_directory".to_string(),
                pattern: "*".to_string(),
                action: PermissionPolicyAction::Allow,
            }],
            task_permissions: Vec::new(),
            instructions: Some(VLM_SYSTEM_PROMPT.to_string()),
        },
    ]
}

fn load_markdown_agent_definition(
    path: &Path,
    warnings: &mut Vec<String>,
) -> Option<AgentDefinition> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) => {
            warnings.push(format!(
                "Failed to read OpenCode agent file {}: {}",
                path.display(),
                err
            ));
            return None;
        }
    };

    let (frontmatter, body) = split_frontmatter(&content);
    let data = match frontmatter {
        Some(raw) if !raw.trim().is_empty() => match serde_yaml::from_str::<serde_yaml::Value>(raw)
        {
            Ok(value) => serde_json::to_value(value).unwrap_or(Value::Null),
            Err(err) => {
                warnings.push(format!(
                    "{}: failed to parse YAML frontmatter: {}",
                    path.display(),
                    err
                ));
                return None;
            }
        },
        _ => Value::Object(serde_json::Map::new()),
    };

    let fallback_name = agent_name_from_path(path);
    parse_agent_definition(
        &fallback_name,
        &data,
        Some(body.trim().to_string()),
        warnings,
        &format!("agent file {}", path.display()),
    )
}

fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let Some(rest) = content.strip_prefix("---") else {
        return (None, content);
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return (None, content);
    };

    let frontmatter_start = content.len() - rest.len();
    let mut offset = frontmatter_start;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            let body_start = offset + line.len();
            return (
                Some(&content[frontmatter_start..offset]),
                &content[body_start..],
            );
        }
        offset += line.len();
    }

    (None, content)
}

fn agent_name_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "agent".to_string())
}

fn parse_agent_definition(
    fallback_name: &str,
    value: &Value,
    instructions: Option<String>,
    warnings: &mut Vec<String>,
    context: &str,
) -> Option<AgentDefinition> {
    let obj = match value {
        Value::Object(obj) => obj,
        Value::Null => return None,
        _ => {
            warnings.push(format!("{} must be an object", context));
            return None;
        }
    };

    if obj
        .get("disable")
        .or_else(|| obj.get("disabled"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return None;
    }

    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_name)
        .trim();
    if name.is_empty() {
        warnings.push(format!("{} has an empty agent name", context));
        return None;
    }

    let mode_value = obj.get("mode").and_then(Value::as_str);
    let parsed_mode = mode_value.and_then(AgentMode::parse);
    let mode = parsed_mode.unwrap_or_else(|| default_mode_for_agent(name));
    let mode_explicit = parsed_mode.is_some();
    if obj.get("mode").is_some() && parsed_mode.is_none() {
        warnings.push(format!(
            "{}.mode must be primary, subagent, or all",
            context
        ));
    }

    let description = obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let hidden_value = obj.get("hidden");
    let hidden = hidden_value.and_then(Value::as_bool).unwrap_or(false);
    let hidden_explicit = hidden_value.and_then(Value::as_bool).is_some();
    let model = string_field(obj.get("model"));
    let (reasoning_effort, reasoning_effort_explicit) = parse_reasoning_effort(
        obj.get("reasoningEffort")
            .or_else(|| obj.get("reasoning_effort")),
        warnings,
        &format!("{}.reasoningEffort", context),
    );
    let temperature = number_field(
        obj.get("temperature"),
        warnings,
        &format!("{}.temperature", context),
    );
    let top_p = number_field(obj.get("top_p"), warnings, &format!("{}.top_p", context));
    let max_steps = parse_steps(
        obj.get("steps")
            .or_else(|| obj.get("maxSteps"))
            .or_else(|| obj.get("max_steps")),
        warnings,
        context,
    );
    let tools = parse_tools(obj.get("tools"), warnings, context);
    let permissions = parse_permission_rules(
        obj.get("permission"),
        warnings,
        &format!("{}.permission", context),
    );
    let task_permissions = parse_task_permission_rules(
        obj.get("task_permissions")
            .or_else(|| obj.get("taskPermissions"))
            .or_else(|| obj.get("task")),
        warnings,
        &format!("{}.task_permissions", context),
    );
    let instructions = instructions
        .filter(|s| !s.trim().is_empty())
        .or_else(|| string_field(obj.get("instructions")))
        .or_else(|| string_field(obj.get("prompt")));

    Some(AgentDefinition {
        name: name.to_string(),
        description,
        mode,
        mode_explicit,
        hidden,
        hidden_explicit,
        model,
        reasoning_effort,
        reasoning_effort_explicit,
        temperature,
        top_p,
        max_steps,
        tools,
        permissions,
        task_permissions,
        instructions,
    })
}

fn parse_reasoning_effort(
    value: Option<&Value>,
    warnings: &mut Vec<String>,
    context: &str,
) -> (Option<crate::model::reasoning::ReasoningEffort>, bool) {
    use crate::model::reasoning::ReasoningEffort;

    let Some(value) = value else {
        return (None, false);
    };

    match value {
        Value::Null => (None, true),
        Value::Bool(false) => (None, true),
        Value::Bool(true) => {
            warnings.push(format!(
                "{} must be null, false, or one of none, minimal, low, medium, high, xhigh, or max",
                context
            ));
            (None, false)
        }
        Value::String(raw) => {
            let normalized = raw.trim().to_ascii_lowercase().replace('_', "-");
            if normalized.is_empty()
                || matches!(normalized.as_str(), "none" | "off" | "false" | "disabled")
            {
                return (None, true);
            }

            match raw.parse::<ReasoningEffort>() {
                Ok(ReasoningEffort::None) => (None, true),
                Ok(effort) => (Some(effort), true),
                Err(_) => {
                    warnings.push(format!(
                        "{} must be null, false, or one of none, minimal, low, medium, high, xhigh, or max; got '{}'",
                        context, raw
                    ));
                    (None, false)
                }
            }
        }
        _ => {
            warnings.push(format!(
                "{} must be null, false, or one of none, minimal, low, medium, high, xhigh, or max",
                context
            ));
            (None, false)
        }
    }
}

fn string_field(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn number_field(value: Option<&Value>, warnings: &mut Vec<String>, context: &str) -> Option<f64> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => n.as_f64(),
        Some(_) => {
            warnings.push(format!("{} must be a number", context));
            None
        }
    }
}

fn parse_steps(value: Option<&Value>, warnings: &mut Vec<String>, context: &str) -> Option<usize> {
    let Some(value) = value else {
        return None;
    };
    let Some(num) = value.as_u64() else {
        warnings.push(format!("{}.steps must be a positive integer", context));
        return None;
    };
    if num == 0 {
        warnings.push(format!("{}.steps must be greater than 0", context));
        return None;
    }
    if num > usize::MAX as u64 {
        warnings.push(format!(
            "{}.steps is too large for this platform; ignoring value {}",
            context, num
        ));
        return None;
    }
    Some(num as usize)
}

fn parse_tools(
    value: Option<&Value>,
    warnings: &mut Vec<String>,
    context: &str,
) -> Option<Vec<String>> {
    let Some(value) = value else {
        return None;
    };

    let mut tools = Vec::new();
    match value {
        Value::Array(arr) => {
            for item in arr {
                if let Some(tool) = item.as_str() {
                    push_tool(&mut tools, tool);
                }
            }
        }
        Value::String(tool) => push_tool(&mut tools, tool),
        Value::Object(map) => {
            for (tool, enabled) in map {
                if enabled.as_bool().unwrap_or(false) {
                    push_tool(&mut tools, tool);
                }
            }
        }
        _ => warnings.push(format!(
            "{}.tools must be a string, array of strings, or object of booleans",
            context
        )),
    }

    (!tools.is_empty()).then_some(tools)
}

fn push_tool(tools: &mut Vec<String>, tool: &str) {
    let tool = tool.trim().to_ascii_lowercase();
    if !tool.is_empty() && !tools.iter().any(|existing| existing == &tool) {
        tools.push(tool);
    }
}

fn parse_task_permission_rules(
    value: Option<&Value>,
    warnings: &mut Vec<String>,
    context: &str,
) -> PermissionRules {
    let Some(value) = value else {
        return Vec::new();
    };

    if let Some(action_text) = value.as_str() {
        return PermissionPolicyAction::parse(action_text)
            .map(|action| {
                vec![PermissionRule {
                    permission: "task".to_string(),
                    pattern: "*".to_string(),
                    action,
                }]
            })
            .unwrap_or_else(|| {
                warnings.push(format!(
                    "{} must be one of allow, ask, or deny; got '{}'",
                    context, action_text
                ));
                Vec::new()
            });
    }

    if let Value::Array(arr) = value {
        let mut rules = vec![PermissionRule {
            permission: "task".to_string(),
            pattern: "*".to_string(),
            action: PermissionPolicyAction::Deny,
        }];
        for item in arr {
            if let Some(agent) = item.as_str() {
                let pattern = agent.trim();
                if !pattern.is_empty() {
                    rules.push(PermissionRule {
                        permission: "task".to_string(),
                        pattern: pattern.to_ascii_lowercase(),
                        action: PermissionPolicyAction::Allow,
                    });
                }
            }
        }
        return rules;
    }

    let Some(map) = value.as_object() else {
        warnings.push(format!(
            "{} must be an action, agent array, or object of agent rules",
            context
        ));
        return Vec::new();
    };

    let mut out = Vec::new();
    for (pattern, action_value) in map {
        let Some(action_text) = action_value.as_str() else {
            warnings.push(format!(
                "{}.{} must be one of allow, ask, or deny",
                context, pattern
            ));
            continue;
        };
        let Some(action) = PermissionPolicyAction::parse(action_text) else {
            warnings.push(format!(
                "{}.{} must be one of allow, ask, or deny; got '{}'",
                context, pattern, action_text
            ));
            continue;
        };
        out.push(PermissionRule {
            permission: "task".to_string(),
            pattern: expand_permission_pattern(pattern).to_ascii_lowercase(),
            action,
        });
    }
    out
}

fn parse_permission_rules(
    value: Option<&Value>,
    warnings: &mut Vec<String>,
    context: &str,
) -> PermissionRules {
    let mut out = Vec::new();
    let Some(value) = value else {
        return out;
    };
    if value.is_null() {
        return out;
    }

    if let Some(action_text) = value.as_str() {
        match PermissionPolicyAction::parse(action_text) {
            Some(action) => out.push(PermissionRule {
                permission: "*".to_string(),
                pattern: "*".to_string(),
                action,
            }),
            None => warnings.push(format!(
                "{} must be one of allow, ask, or deny; got '{}'",
                context, action_text
            )),
        }
        return out;
    }

    let Some(map) = value.as_object() else {
        warnings.push(format!("{} must be a string or object", context));
        return out;
    };

    for (permission, value) in map {
        let permission = permission.trim().to_ascii_lowercase();
        if permission.is_empty() {
            warnings.push(format!("{} contains an empty permission key", context));
            continue;
        }

        if let Some(action_text) = value.as_str() {
            match PermissionPolicyAction::parse(action_text) {
                Some(action) => out.push(PermissionRule {
                    permission,
                    pattern: "*".to_string(),
                    action,
                }),
                None => warnings.push(format!(
                    "{}.{} must be one of allow, ask, or deny; got '{}'",
                    context, permission, action_text
                )),
            }
            continue;
        }

        let Some(patterns) = value.as_object() else {
            warnings.push(format!(
                "{}.{} must be one of allow, ask, deny, or an object of pattern rules",
                context, permission
            ));
            continue;
        };

        for (pattern, action_value) in patterns {
            let Some(action_text) = action_value.as_str() else {
                warnings.push(format!(
                    "{}.{}.{} must be one of allow, ask, or deny",
                    context, permission, pattern
                ));
                continue;
            };
            let Some(action) = PermissionPolicyAction::parse(action_text) else {
                warnings.push(format!(
                    "{}.{}.{} must be one of allow, ask, or deny; got '{}'",
                    context, permission, pattern, action_text
                ));
                continue;
            };
            out.push(PermissionRule {
                permission: permission.clone(),
                pattern: expand_permission_pattern(pattern),
                action,
            });
        }
    }

    out
}

fn normalize_agent_ref(name: &str) -> String {
    AgentDefinition::normalized_name(name)
}

fn default_mode_for_agent(name: &str) -> AgentMode {
    match AgentDefinition::normalized_name(name).as_str() {
        "build" | "plan" => AgentMode::Primary,
        "general" | "explore" => AgentMode::Subagent,
        _ => AgentMode::All,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plan_can_only_task_explore_by_default() {
        let registry = AgentRegistry::default();

        assert!(registry.can_agent_invoke("Plan", "explore"));
        assert!(!registry.can_agent_invoke("Plan", "general"));
    }

    #[test]
    fn builtin_vlm_agent_is_hidden_until_model_is_configured() {
        let registry = AgentRegistry::default();

        assert!(registry.get(VLM_AGENT_NAME).is_some());
        assert!(registry.task_target(VLM_AGENT_NAME).is_none());
        assert!(!registry
            .visible_agent_names_for_mentions()
            .contains(&VLM_AGENT_NAME.to_string()));
    }

    #[test]
    fn builtin_vlm_agent_is_available_and_model_configurable() {
        let mut warnings = Vec::new();
        let registry = AgentRegistry::with_definitions(
            None,
            parse_agent_definitions_from_config(
                Some(&json!({
                    "vlm-agent": {
                        "model": "xai/grok-4.3"
                    }
                })),
                &mut warnings,
            ),
        );

        assert!(warnings.is_empty());
        let agent = registry.get(VLM_AGENT_NAME).expect("vlm-agent");
        assert!(agent.visible_subagent());
        assert!(registry.task_target(VLM_AGENT_NAME).is_some());
        assert_eq!(agent.model.as_deref(), Some("xai/grok-4.3"));
        assert_eq!(
            agent.tools.as_ref().map(Vec::as_slice),
            Some(&["view_image".to_string()][..])
        );
        assert_eq!(
            agent.permissions,
            vec![PermissionRule {
                permission: "external_directory".to_string(),
                pattern: "*".to_string(),
                action: PermissionPolicyAction::Allow,
            }]
        );
        assert!(agent
            .instructions
            .as_deref()
            .is_some_and(|prompt| prompt.contains("view_image")));
    }

    #[tokio::test]
    async fn builtin_vlm_agent_can_view_external_image_paths_without_prompt() {
        let mut warnings = Vec::new();
        let registry = AgentRegistry::with_definitions(
            None,
            parse_agent_definitions_from_config(
                Some(&json!({
                    "vlm-agent": {
                        "model": "xai/grok-4.3"
                    }
                })),
                &mut warnings,
            ),
        );
        let params = serde_json::json!({
            "path": "/var/folders/example/crabcode-clipboard.png"
        });
        let permissions = crate::tools::ToolPermissions::new("/tmp/workspace")
            .with_agent_permission_rules(registry.permission_rules_map());

        assert!(permissions
            .preflight(VLM_AGENT_NAME, "view_image", &params, None)
            .await
            .is_ok());
    }

    #[test]
    fn parses_json_agent_fields() {
        let mut warnings = Vec::new();
        let defs = parse_agent_definitions_from_config(
            Some(&json!({
                "reviewer": {
                    "description": "Review code",
                    "mode": "subagent",
                    "hidden": true,
                    "model": "openai/gpt-5",
                    "reasoningEffort": "low",
                    "temperature": 0.2,
                    "top_p": 0.9,
                    "max_steps": 7,
                    "tools": ["read", "grep"],
                    "permission": { "edit": "deny" },
                    "task_permissions": ["explore"],
                    "prompt": "Read only."
                }
            })),
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert_eq!(defs.len(), 1);
        let def = &defs[0];
        assert_eq!(def.name, "reviewer");
        assert_eq!(def.mode, AgentMode::Subagent);
        assert!(def.hidden);
        assert_eq!(
            def.reasoning_effort,
            Some(crate::model::reasoning::ReasoningEffort::Low)
        );
        assert_eq!(def.max_steps, Some(7));
        assert_eq!(
            def.tools.as_deref(),
            Some(&["read".to_string(), "grep".to_string()][..])
        );
        assert_eq!(def.instructions.as_deref(), Some("Read only."));
        assert_eq!(def.task_permissions.len(), 2);
    }

    #[test]
    fn reasoning_effort_none_aliases_disable_agent_reasoning() {
        let mut warnings = Vec::new();
        let defs = parse_agent_definitions_from_config(
            Some(&json!({
                "fast": {
                    "mode": "subagent",
                    "reasoningEffort": null
                },
                "also-fast": {
                    "mode": "subagent",
                    "reasoningEffort": false
                },
                "string-off": {
                    "mode": "subagent",
                    "reasoningEffort": "none"
                }
            })),
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert_eq!(defs.len(), 3);
        assert!(defs.iter().all(|def| def.reasoning_effort.is_none()));
    }

    #[test]
    fn reasoning_effort_null_overrides_prior_agent_definition() {
        let mut warnings = Vec::new();
        let base = parse_agent_definitions_from_config(
            Some(&json!({
                "frontend-agent": {
                    "mode": "subagent",
                    "reasoningEffort": "high"
                }
            })),
            &mut warnings,
        );
        let overlay = parse_agent_definitions_from_config(
            Some(&json!({
                "frontend-agent": {
                    "mode": "subagent",
                    "reasoningEffort": null
                }
            })),
            &mut warnings,
        );
        let registry = AgentRegistry::with_definitions(None, base.into_iter().chain(overlay));

        assert!(warnings.is_empty());
        assert_eq!(
            registry.get("frontend-agent").unwrap().reasoning_effort,
            None
        );
    }

    #[test]
    fn markdown_body_becomes_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reviewer.md");
        std::fs::write(
            &path,
            "---\ndescription: Review code\nmode: subagent\nhidden: true\nsteps: 3\npermission:\n  edit: deny\n---\nBe strict.\n",
        )
        .unwrap();

        let mut warnings = Vec::new();
        let defs = load_markdown_agent_definitions(&[path], &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "reviewer");
        assert_eq!(defs[0].mode, AgentMode::Subagent);
        assert!(defs[0].hidden);
        assert_eq!(defs[0].max_steps, Some(3));
        assert_eq!(defs[0].permissions[0].permission, "edit");
        assert_eq!(defs[0].instructions.as_deref(), Some("Be strict."));
    }

    #[test]
    fn later_agent_definitions_override_earlier_ones() {
        let mut warnings = Vec::new();
        let markdown = parse_agent_definitions_from_config(
            Some(&json!({
                "reviewer": {
                    "description": "Markdown description",
                    "mode": "subagent",
                    "hidden": true,
                    "steps": 3
                }
            })),
            &mut warnings,
        );
        let json_defs = parse_agent_definitions_from_config(
            Some(&json!({
                "reviewer": {
                    "description": "JSON description",
                    "mode": "subagent",
                    "max_steps": 5
                }
            })),
            &mut warnings,
        );
        let registry = AgentRegistry::with_definitions(None, markdown.into_iter().chain(json_defs));
        let reviewer = registry.get("reviewer").unwrap();

        assert!(warnings.is_empty());
        assert_eq!(reviewer.description, "JSON description");
        assert_eq!(reviewer.mode, AgentMode::Subagent);
        assert!(reviewer.hidden);
        assert_eq!(reviewer.max_steps, Some(5));
    }

    #[test]
    fn explicit_all_mode_and_hidden_false_override_prior_definition() {
        let mut warnings = Vec::new();
        let markdown = parse_agent_definitions_from_config(
            Some(&json!({
                "reviewer": {
                    "mode": "subagent",
                    "hidden": true
                }
            })),
            &mut warnings,
        );
        let json_defs = parse_agent_definitions_from_config(
            Some(&json!({
                "reviewer": {
                    "mode": "all",
                    "hidden": false
                }
            })),
            &mut warnings,
        );
        let registry = AgentRegistry::with_definitions(None, markdown.into_iter().chain(json_defs));
        let reviewer = registry.get("reviewer").unwrap();

        assert!(warnings.is_empty());
        assert_eq!(reviewer.mode, AgentMode::All);
        assert!(!reviewer.hidden);
    }

    #[test]
    fn visible_primary_agents_include_primary_and_all_modes_only() {
        let mut warnings = Vec::new();
        let defs = parse_agent_definitions_from_config(
            Some(&json!({
                "designer": {
                    "description": "Design UI",
                    "mode": "all"
                },
                "internal": {
                    "mode": "primary",
                    "hidden": true
                },
                "reviewer": {
                    "mode": "subagent"
                }
            })),
            &mut warnings,
        );
        let registry = AgentRegistry::with_definitions(None, defs);
        let names = registry
            .visible_primary_agents()
            .into_iter()
            .map(|agent| agent.name.as_str())
            .collect::<Vec<_>>();

        assert!(warnings.is_empty());
        assert!(names.contains(&"build"));
        assert!(names.contains(&"plan"));
        assert!(names.contains(&"designer"));
        assert!(!names.contains(&"reviewer"));
        assert!(!names.contains(&"internal"));
    }
}
