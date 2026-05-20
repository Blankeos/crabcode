#[derive(Debug, Clone)]
pub struct ParsedCommand<'a> {
    pub name: String,
    pub args: Vec<String>,
    pub raw: String,
    pub prefs_dao: Option<&'a crate::persistence::PrefsDAO>,
    pub active_model_id: Option<String>,
}

impl<'a> ParsedCommand<'a> {
    pub fn raw_args(&self) -> &str {
        let Some(without_slash) = self.raw.trim().strip_prefix('/') else {
            return "";
        };
        let without_name = without_slash
            .strip_prefix(&self.name)
            .unwrap_or(without_slash);
        without_name.trim_start()
    }
}

impl<'a> PartialEq for ParsedCommand<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.args == other.args
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputType<'a> {
    Command(ParsedCommand<'a>),
    Message(String),
}

pub fn parse_input(input: &str) -> InputType {
    let trimmed = input.trim();

    if trimmed.starts_with('/') {
        if let Some(parsed) = parse_command(trimmed) {
            return InputType::Command(parsed);
        }
    }

    InputType::Message(trimmed.to_string())
}

fn parse_command(input: &str) -> Option<ParsedCommand> {
    let without_slash = input.strip_prefix('/')?;
    let parts = shlex::split(without_slash).unwrap_or_else(|| {
        without_slash
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect()
    });

    if parts.is_empty() {
        return None;
    }

    let name = parts[0].to_string();
    let args: Vec<String> = parts[1..].to_vec();

    Some(ParsedCommand {
        name,
        args,
        raw: input.to_string(),
        prefs_dao: None,
        active_model_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_simple() {
        let input = "/exit";
        let result = parse_command(input);
        assert_eq!(
            result,
            Some(ParsedCommand {
                name: "exit".to_string(),
                args: vec![],
                raw: "/exit".to_string(),
                prefs_dao: None,
                active_model_id: None,
            })
        );
    }

    #[test]
    fn test_parse_command_with_args() {
        let input = "/new my-session";
        let result = parse_command(input);
        assert_eq!(
            result,
            Some(ParsedCommand {
                name: "new".to_string(),
                args: vec!["my-session".to_string()],
                raw: "/new my-session".to_string(),
                prefs_dao: None,
                active_model_id: None,
            })
        );
    }

    #[test]
    fn test_parse_command_with_multiple_args() {
        let input = "/connect nano-gpt gpt-4";
        let result = parse_command(input);
        assert_eq!(
            result,
            Some(ParsedCommand {
                name: "connect".to_string(),
                args: vec!["nano-gpt".to_string(), "gpt-4".to_string()],
                raw: "/connect nano-gpt gpt-4".to_string(),
                prefs_dao: None,
                active_model_id: None,
            })
        );
    }

    #[test]
    fn test_parse_command_with_quoted_args() {
        let input = r#"/create-file config.json src "{ \"key\": \"value\" }""#;
        let result = parse_command(input);
        assert_eq!(
            result,
            Some(ParsedCommand {
                name: "create-file".to_string(),
                args: vec![
                    "config.json".to_string(),
                    "src".to_string(),
                    r#"{ "key": "value" }"#.to_string()
                ],
                raw: input.to_string(),
                prefs_dao: None,
                active_model_id: None,
            })
        );
    }

    #[test]
    fn test_raw_args_preserves_user_text_after_command_name() {
        let input = r#"/test "quoted arg" plain"#;
        let result = parse_command(input).unwrap();
        assert_eq!(result.raw_args(), r#""quoted arg" plain"#);
    }

    #[test]
    fn test_parse_command_empty() {
        let input = "/";
        let result = parse_command(input);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_command_only_slash_and_spaces() {
        let input = "/    ";
        let result = parse_command(input);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_input_command() {
        let input = "/exit";
        let result = parse_input(input);
        assert_eq!(
            result,
            InputType::Command(ParsedCommand {
                name: "exit".to_string(),
                args: vec![],
                raw: "/exit".to_string(),
                prefs_dao: None,
                active_model_id: None,
            })
        );
    }

    #[test]
    fn test_parse_input_message() {
        let input = "hello world";
        let result = parse_input(input);
        assert_eq!(result, InputType::Message("hello world".to_string()));
    }

    #[test]
    fn test_parse_input_message_with_leading_spaces() {
        let input = "   hello world";
        let result = parse_input(input);
        assert_eq!(result, InputType::Message("hello world".to_string()));
    }

    #[test]
    fn test_parse_input_command_with_args() {
        let input = "/sessions";
        let result = parse_input(input);
        assert_eq!(
            result,
            InputType::Command(ParsedCommand {
                name: "sessions".to_string(),
                args: vec![],
                raw: "/sessions".to_string(),
                prefs_dao: None,
                active_model_id: None,
            })
        );
    }

    #[test]
    fn test_parse_input_empty() {
        let input = "";
        let result = parse_input(input);
        assert_eq!(result, InputType::Message("".to_string()));
    }

    #[test]
    fn test_parse_input_only_spaces() {
        let input = "   ";
        let result = parse_input(input);
        assert_eq!(result, InputType::Message("".to_string()));
    }
}
