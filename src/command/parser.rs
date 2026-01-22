#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCommand {
    pub name: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputType {
    Command(ParsedCommand),
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
    let parts: Vec<&str> = without_slash.split_whitespace().collect();

    if parts.is_empty() {
        return None;
    }

    let name = parts[0].to_string();
    let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

    Some(ParsedCommand { name, args })
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
                args: vec![]
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
                args: vec!["my-session".to_string()]
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
                args: vec!["nano-gpt".to_string(), "gpt-4".to_string()]
            })
        );
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
                args: vec![]
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
                args: vec![]
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
