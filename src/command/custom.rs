use anyhow::{Context, Result};
use regex::{Captures, Regex};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command as TokioCommand;

const SHELL_TIMEOUT_SECONDS: u64 = 30;
const MAX_SHELL_OUTPUT_BYTES: usize = 51200;
const MAX_REFERENCED_FILE_BYTES: usize = 51200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomCommandSource {
    Config(PathBuf),
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommand {
    pub name: String,
    pub description: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: Option<bool>,
    pub template: String,
    pub source: CustomCommandSource,
    pub workdir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCommand {
    pub prompt: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: Option<bool>,
}

impl CustomCommand {
    pub async fn render(&self, raw_args: &str) -> Result<RenderedCommand> {
        let prompt = apply_arguments(&self.template, raw_args);
        let prompt = expand_shell_blocks(&prompt, &self.workdir).await?;
        let prompt = append_file_references(&prompt, &self.workdir);

        Ok(RenderedCommand {
            prompt: prompt.trim().to_string(),
            agent: self.agent.clone(),
            model: self.model.clone(),
            subtask: self.subtask,
        })
    }
}

pub fn commands_from_config_value(
    value: &Value,
    source_path: &Path,
    workdir: &Path,
    warnings: &mut Vec<String>,
) -> Vec<CustomCommand> {
    let Some(commands) = value.as_object() else {
        warnings.push(format!(
            "command in {} must be an object",
            source_path.display()
        ));
        return Vec::new();
    };

    let mut out = Vec::new();
    for (name, value) in commands {
        let Some(obj) = value.as_object() else {
            warnings.push(format!(
                "command.{} in {} must be an object",
                name,
                source_path.display()
            ));
            continue;
        };

        let Some(template) = obj.get("template").and_then(|v| v.as_str()) else {
            warnings.push(format!(
                "command.{} in {} must include a string template",
                name,
                source_path.display()
            ));
            continue;
        };

        let name = name.trim();
        if name.is_empty() {
            warnings.push(format!(
                "command in {} has an empty name",
                source_path.display()
            ));
            continue;
        }

        let command = CustomCommand {
            name: normalize_command_name(name),
            description: optional_string(obj.get("description")),
            agent: optional_string(obj.get("agent")),
            model: optional_string(obj.get("model")),
            subtask: obj.get("subtask").and_then(|v| v.as_bool()),
            template: template.trim().to_string(),
            source: CustomCommandSource::Config(source_path.to_path_buf()),
            workdir: workdir.to_path_buf(),
        };
        out.push(command);
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn commands_from_directory(
    dir: &Path,
    workdir: &Path,
    warnings: &mut Vec<String>,
) -> Vec<CustomCommand> {
    let mut out = Vec::new();
    let mut files = list_command_files(dir);
    files.sort();
    files.dedup();

    for path in files {
        match command_from_file(dir, &path, workdir) {
            Ok(Some(command)) => out.push(command),
            Ok(None) => {}
            Err(err) => warnings.push(format!(
                "Failed to load command file {}: {}",
                path.display(),
                err
            )),
        }
    }

    out
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn list_command_files(dir: &Path) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for subdir in ["command", "commands"] {
        let pattern = dir
            .join(subdir)
            .join("**")
            .join("*.md")
            .to_string_lossy()
            .to_string();
        let Ok(entries) = glob::glob(&pattern) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.is_file() {
                out.push(entry);
            }
        }
    }
    out
}

fn command_from_file(
    config_dir: &Path,
    path: &Path,
    workdir: &Path,
) -> Result<Option<CustomCommand>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let (frontmatter, body) = split_frontmatter(&content);
    let data = parse_frontmatter(&frontmatter)?;
    let template = body.trim();
    if template.is_empty() {
        return Ok(None);
    }

    let Some(name) = command_name_from_path(config_dir, path) else {
        return Ok(None);
    };

    Ok(Some(CustomCommand {
        name,
        description: data.description,
        agent: data.agent,
        model: data.model,
        subtask: data.subtask,
        template: template.to_string(),
        source: CustomCommandSource::File(path.to_path_buf()),
        workdir: workdir.to_path_buf(),
    }))
}

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    description: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    subtask: Option<bool>,
}

fn parse_frontmatter(frontmatter: &str) -> Result<Frontmatter> {
    if frontmatter.trim().is_empty() {
        return Ok(Frontmatter::default());
    }

    match serde_yaml::from_str(frontmatter) {
        Ok(data) => Ok(data),
        Err(_) => {
            let sanitized = fallback_sanitize_yaml(frontmatter);
            serde_yaml::from_str(&sanitized).context("Invalid YAML frontmatter")
        }
    }
}

fn split_frontmatter(content: &str) -> (String, String) {
    if let Some(rest) = content.strip_prefix("---\n") {
        if let Some((frontmatter, body)) = rest.split_once("\n---") {
            return (frontmatter.to_string(), body.trim_start().to_string());
        }
    }

    if let Some(rest) = content.strip_prefix("---\r\n") {
        if let Some((frontmatter, body)) = rest.split_once("\r\n---") {
            return (frontmatter.to_string(), body.trim_start().to_string());
        }
    }

    (String::new(), content.to_string())
}

fn fallback_sanitize_yaml(frontmatter: &str) -> String {
    let mut result = String::new();

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        let Some((key, value)) = trimmed.split_once(':') else {
            result.push_str(line);
            result.push('\n');
            continue;
        };

        let value = value.trim();
        if value.is_empty()
            || value == ">"
            || value == "|"
            || value.starts_with('"')
            || value.starts_with('\'')
        {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        if value.contains(':') {
            result.push_str(key);
            result.push_str(": |-\n  ");
            result.push_str(value);
            result.push('\n');
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    result
}

fn command_name_from_path(config_dir: &Path, path: &Path) -> Option<String> {
    for subdir in ["command", "commands"] {
        let root = config_dir.join(subdir);
        if let Ok(relative) = path.strip_prefix(&root) {
            let mut without_ext = relative.to_path_buf();
            without_ext.set_extension("");
            let name = without_ext.to_string_lossy().replace('\\', "/");
            let name = normalize_command_name(&name);
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn normalize_command_name(name: &str) -> String {
    name.trim().trim_start_matches('/').replace('\\', "/")
}

fn apply_arguments(template: &str, raw_args: &str) -> String {
    let args = parse_raw_arguments(raw_args);
    let placeholder_re = Regex::new(r"\$(\d+)").expect("valid placeholder regex");
    let placeholders: Vec<usize> = placeholder_re
        .captures_iter(template)
        .filter_map(|caps| caps.get(1)?.as_str().parse::<usize>().ok())
        .collect();
    let last_placeholder = placeholders.iter().copied().max().unwrap_or(0);
    let has_positional_placeholders = !placeholders.is_empty();
    let has_arguments_placeholder = template.contains("$ARGUMENTS");

    let with_positionals = placeholder_re
        .replace_all(template, |caps: &Captures<'_>| {
            let position = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            if position == 0 {
                return String::new();
            }
            let arg_index = position - 1;
            if arg_index >= args.len() {
                return String::new();
            }
            if position == last_placeholder {
                args[arg_index..].join(" ")
            } else {
                args[arg_index].clone()
            }
        })
        .to_string();

    let raw_args = raw_args.trim();
    let mut out = with_positionals.replace("$ARGUMENTS", raw_args);
    if !has_positional_placeholders && !has_arguments_placeholder && !raw_args.is_empty() {
        out.push_str("\n\n");
        out.push_str(raw_args);
    }
    out
}

fn parse_raw_arguments(raw_args: &str) -> Vec<String> {
    if let Some(args) = shlex::split(raw_args) {
        return args;
    }

    let re =
        Regex::new(r#"(?:\[Image\s+\d+\]|"[^"]*"|'[^']*'|[^\s"']+)"#).expect("valid args regex");
    re.find_iter(raw_args)
        .map(|m| trim_wrapping_quotes(m.as_str()).to_string())
        .collect()
}

fn trim_wrapping_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

async fn expand_shell_blocks(template: &str, workdir: &Path) -> Result<String> {
    let re = Regex::new(r"!`([^`]+)`").expect("valid shell regex");
    let mut out = String::new();
    let mut last = 0usize;

    for caps in re.captures_iter(template) {
        let Some(full) = caps.get(0) else {
            continue;
        };
        let Some(command) = caps.get(1).map(|m| m.as_str()) else {
            continue;
        };

        out.push_str(&template[last..full.start()]);
        out.push_str(&run_shell_block(command, workdir).await?);
        last = full.end();
    }

    out.push_str(&template[last..]);
    Ok(out)
}

async fn run_shell_block(command: &str, workdir: &Path) -> Result<String> {
    let mut child = TokioCommand::new("bash");
    child.arg("-c").arg(command).current_dir(workdir);

    let output = tokio::time::timeout(Duration::from_secs(SHELL_TIMEOUT_SECONDS), child.output())
        .await
        .with_context(|| {
            format!(
                "Command timed out after {} seconds: {}",
                SHELL_TIMEOUT_SECONDS, command
            )
        })?
        .with_context(|| format!("Failed to run command: {}", command))?;

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&output.stdout);
    if !output.stderr.is_empty() {
        if !bytes.is_empty() {
            bytes.extend_from_slice(b"\n");
        }
        bytes.extend_from_slice(&output.stderr);
    }

    if bytes.len() > MAX_SHELL_OUTPUT_BYTES {
        bytes.truncate(MAX_SHELL_OUTPUT_BYTES);
        bytes.extend_from_slice(b"\n[Output truncated]");
    }

    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn append_file_references(template: &str, workdir: &Path) -> String {
    let re = Regex::new(r"(^|[^\w`])@(\.?[^\s`,.]*(?:\.[^\s`,.]+)*)")
        .expect("valid file reference regex");
    let mut seen = std::collections::HashSet::new();
    let mut references = Vec::new();

    for caps in re.captures_iter(template) {
        let Some(name) = caps.get(2).map(|m| m.as_str()) else {
            continue;
        };
        if name.is_empty() || !seen.insert(name.to_string()) {
            continue;
        }
        let path = resolve_reference_path(name, workdir);
        if path.is_file() {
            if let Ok(mut content) = fs::read_to_string(&path) {
                if content.len() > MAX_REFERENCED_FILE_BYTES {
                    content.truncate(MAX_REFERENCED_FILE_BYTES);
                    content.push_str("\n[File truncated]");
                }
                references.push(format!("<file path=\"{}\">\n{}\n</file>", name, content));
            }
        } else if path.is_dir() {
            let listing = fs::read_dir(&path)
                .ok()
                .map(|entries| {
                    let mut names = entries
                        .flatten()
                        .map(|entry| entry.file_name().to_string_lossy().to_string())
                        .collect::<Vec<_>>();
                    names.sort();
                    names.join("\n")
                })
                .unwrap_or_default();
            if !listing.is_empty() {
                references.push(format!(
                    "<directory path=\"{}\">\n{}\n</directory>",
                    name, listing
                ));
            }
        }
    }

    if references.is_empty() {
        template.to_string()
    } else {
        format!(
            "{}\n\nReferenced files:\n{}",
            template.trim_end(),
            references.join("\n\n")
        )
    }
}

fn resolve_reference_path(name: &str, workdir: &Path) -> PathBuf {
    if let Some(rest) = name.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }

    let path = PathBuf::from(name);
    if path.is_absolute() {
        path
    } else {
        workdir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_apply_arguments_replaces_arguments_placeholder() {
        let result = apply_arguments("Build $ARGUMENTS", "Button primary");
        assert_eq!(result, "Build Button primary");
    }

    #[test]
    fn test_apply_arguments_replaces_positionals_and_last_consumes_rest() {
        let result = apply_arguments("Create $1 with $2", "file.rs src/lib extra");
        assert_eq!(result, "Create file.rs with src/lib extra");
    }

    #[test]
    fn test_apply_arguments_appends_args_when_template_has_no_placeholders() {
        let result = apply_arguments("Review this", "main branch");
        assert_eq!(result, "Review this\n\nmain branch");
    }

    #[test]
    fn test_append_file_references_includes_file_content() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("example.txt"), "hello").unwrap();

        let result = append_file_references("Review @example.txt", temp.path());

        assert!(result.contains("Referenced files:"));
        assert!(result.contains("<file path=\"example.txt\">"));
        assert!(result.contains("hello"));
    }

    #[test]
    fn test_parse_raw_arguments_preserves_quoted_segments() {
        let args = parse_raw_arguments(r#"config.json src "{ \"key\": \"value\" }""#);
        assert_eq!(args, vec!["config.json", "src", r#"{ "key": "value" }"#]);
    }

    #[test]
    fn test_commands_from_config_value() {
        let value = json!({
            "test": {
                "template": "Run tests",
                "description": "Run the test suite",
                "agent": "build",
                "model": "openai/gpt-5",
                "subtask": true
            }
        });
        let mut warnings = Vec::new();
        let commands = commands_from_config_value(
            &value,
            Path::new("/tmp/opencode.json"),
            Path::new("/workspace"),
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "test");
        assert_eq!(
            commands[0].description.as_deref(),
            Some("Run the test suite")
        );
        assert_eq!(commands[0].agent.as_deref(), Some("build"));
        assert_eq!(commands[0].model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(commands[0].subtask, Some(true));
    }

    #[test]
    fn test_commands_from_directory_supports_plural_and_nested_names() {
        let temp = tempfile::tempdir().unwrap();
        let commands_dir = temp.path().join("commands").join("team");
        fs::create_dir_all(&commands_dir).unwrap();
        fs::write(
            commands_dir.join("review.md"),
            "---\ndescription: Review changes\nagent: build\n---\nReview $ARGUMENTS",
        )
        .unwrap();

        let mut warnings = Vec::new();
        let commands = commands_from_directory(temp.path(), temp.path(), &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "team/review");
        assert_eq!(commands[0].description.as_deref(), Some("Review changes"));
        assert_eq!(commands[0].template, "Review $ARGUMENTS");
    }
}
