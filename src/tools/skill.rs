use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;

pub struct SkillTool;

impl SkillTool {
    pub fn new() -> Self {
        Self
    }

    fn build_description() -> String {
        // Keep the catalog in the system prompt only (OpenCode-style). Embedding
        // <available_skills> here would resend the full list on every tool step.
        String::from(
            "Load a specialized skill that provides domain-specific instructions and workflows.\n\n\
             Use this tool to inject the skill's instructions and resources into the current conversation. \
             The output may contain detailed workflow guidance as well as references to scripts, files, \
             etc in the same directory as the skill.\n\n\
             The skill name must match one of the skills listed under <available_skills> in your system prompt.",
        )
    }
}

#[async_trait]
impl ToolHandler for SkillTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "skill".to_string(),
            description: Self::build_description(),
            parameters: vec![ParameterSchema {
                name: "name".to_string(),
                description: "The name of the skill from available_skills".to_string(),
                required: true,
                param_type: ParameterType::String,
            }],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["name"])?;

        let name = get_string_param(params, "name").unwrap_or_default();
        if name.trim().is_empty() {
            return Err(ToolError::Validation(
                "Skill name cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let name = get_string_param(&params, "name").unwrap_or_default();
        let name = name.trim();

        let store = crate::skill::get_skill_store()
            .ok_or_else(|| ToolError::Execution("Skill store not initialized".to_string()))?;

        let info = store.get(name).ok_or_else(|| {
            let available: Vec<String> = store.all().iter().map(|s| s.name.clone()).collect();
            let msg = if available.is_empty() {
                format!(
                    "Skill \"{}\" not found. No skills are currently available.",
                    name
                )
            } else {
                format!(
                    "Skill \"{}\" not found. Available skills: {}",
                    name,
                    available.join(", ")
                )
            };
            ToolError::NotFound(msg)
        })?;

        let dir = info
            .location
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        let base_url = format!("file://{}", dir.display());

        // Sample up to 10 files in the skill directory (excluding SKILL.md)
        let file_list = sample_skill_files(&dir, 10);

        let output = format!(
            "<skill_content name=\"{name}\">\n\
             # Skill: {name}\n\n\
             {content}\n\n\
             Base directory for this skill: {base_url}\n\
             Relative paths in this skill (e.g., scripts/, reference/) are relative to this base directory.\n\
             Note: file list is sampled.\n\n\
             <skill_files>\n\
             {files}\n\
             </skill_files>\n\
             </skill_content>",
            name = name,
            content = info.content.trim(),
            files = file_list,
        );

        Ok(ToolResult::new(format!("Loaded skill: {}", name), output)
            .with_metadata("name", serde_json::Value::String(info.name.clone()))
            .with_metadata(
                "dir",
                serde_json::Value::String(dir.to_string_lossy().to_string()),
            ))
    }
}

fn sample_skill_files(dir: &std::path::Path, limit: usize) -> String {
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    if file_name != "SKILL.md" && !file_name.starts_with('.') {
                        files.push(path.to_string_lossy().to_string());
                        if files.len() >= limit {
                            break;
                        }
                    }
                }
            }
        }
    }

    files
        .into_iter()
        .map(|f| format!("<file>{}</file>", f))
        .collect::<Vec<_>>()
        .join("\n")
}
