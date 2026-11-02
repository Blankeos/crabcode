use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AddRequest {
    pub name: String,
    pub url: Option<String>,
    pub env: Vec<String>,
    pub header: Vec<String>,
    pub global: bool,
    pub command: Vec<String>,
    /// Overrides process cwd. `None` uses the current directory.
    pub cwd: Option<PathBuf>,
    /// Overrides `XDG_CONFIG_HOME`. `None` uses the crabcode default.
    pub xdg_config_home: Option<PathBuf>,
}

pub fn add_mcp(request: AddRequest) -> Result<PathBuf> {
    let name = request.name.trim();
    if name.is_empty() || name.chars().any(|ch| ch.is_control()) {
        bail!("MCP server name must be a non-empty string");
    }

    let has_command = !request.command.is_empty();
    if request.url.is_some() && has_command {
        bail!("Provide either --url <url> or a command after --, not both");
    }
    if request.url.is_none() && !has_command {
        bail!("Provide either --url <url> or a command after --");
    }
    if !request.env.is_empty() && request.url.is_some() {
        bail!("--env is only valid for local MCP servers");
    }
    if !request.header.is_empty() && has_command {
        bail!("--header is only valid for remote MCP servers");
    }

    let server = if let Some(url) = request.url.as_deref() {
        let url = url.trim();
        if !is_http_url(url) {
            bail!("Invalid URL: {url}");
        }
        let headers = pairs(&request.header, "HTTP header")?;
        remote_server(url, headers)
    } else {
        let environment = pairs(&request.env, "environment variable")?;
        local_server(&request.command, environment)
    };

    let cwd = match &request.cwd {
        Some(cwd) => cwd.clone(),
        None => crate::utils::cwd::current_dir()?,
    };
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let xdg = request
        .xdg_config_home
        .clone()
        .unwrap_or_else(crate::config::configuration::xdg_config_home);
    let path = if request.global {
        config_path(&global_candidates(&xdg), "Crabcode global")?
    } else {
        let project_root = crate::config::configuration::discover_project_root(&cwd);
        config_path(&project_candidates(&project_root), "Crabcode local")?
    };

    let existing = if path.is_file() {
        fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file {}", path.display()))?
    } else {
        String::new()
    };
    let updated = upsert_mcp_server(&existing, name, &server)
        .with_context(|| format!("Failed to update {}", path.display()))?;
    write_atomic(&path, &updated)?;
    Ok(path)
}

fn is_http_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    match rest {
        Some(rest) => !rest.is_empty() && !rest.contains(char::is_whitespace),
        None => false,
    }
}

fn pairs(entries: &[String], kind: &str) -> Result<Option<Value>> {
    if entries.is_empty() {
        return Ok(None);
    }
    let mut map = Map::new();
    for entry in entries {
        let Some((key, value)) = entry.split_once('=') else {
            bail!("Invalid {kind}: {entry}. Expected KEY=VALUE");
        };
        if key.is_empty() {
            bail!("Invalid {kind}: {entry}. Expected KEY=VALUE");
        }
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
    Ok(Some(Value::Object(map)))
}

fn remote_server(url: &str, headers: Option<Value>) -> Value {
    let mut map = Map::new();
    map.insert("type".to_string(), Value::String("remote".to_string()));
    map.insert("url".to_string(), Value::String(url.to_string()));
    if let Some(headers) = headers {
        map.insert("headers".to_string(), headers);
    }
    Value::Object(map)
}

fn local_server(command: &[String], environment: Option<Value>) -> Value {
    let mut map = Map::new();
    map.insert("type".to_string(), Value::String("local".to_string()));
    map.insert(
        "command".to_string(),
        Value::Array(command.iter().cloned().map(Value::String).collect()),
    );
    if let Some(environment) = environment {
        map.insert("environment".to_string(), environment);
    }
    Value::Object(map)
}

fn project_candidates(project_root: &Path) -> Vec<PathBuf> {
    vec![
        project_root.join(".crabcode").join("crabcode.jsonc"),
        project_root.join(".crabcode").join("crabcode.json"),
        project_root.join(".opencode").join("crabcode.jsonc"),
        project_root.join(".opencode").join("crabcode.json"),
        project_root.join("crabcode.jsonc"),
        project_root.join("crabcode.json"),
    ]
}

fn global_candidates(xdg_config_home: &Path) -> Vec<PathBuf> {
    vec![
        xdg_config_home.join("crabcode").join("crabcode.jsonc"),
        xdg_config_home.join("crabcode").join("crabcode.json"),
        xdg_config_home.join("crabcode.jsonc"),
        xdg_config_home.join("crabcode.json"),
    ]
}

fn config_path(candidates: &[PathBuf], label: &str) -> Result<PathBuf> {
    let existing: Vec<&PathBuf> = candidates.iter().filter(|path| path.is_file()).collect();
    match existing.as_slice() {
        [] => Ok(candidates[0].clone()),
        [path] => Ok((*path).clone()),
        _ => {
            let mut msg = format!("Multiple config files found for {label}. Keep only one:\n");
            for path in existing {
                msg.push_str(&format!("- {}\n", path.display()));
            }
            bail!(msg)
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("tmp")
    ));
    fs::write(&tmp, text).with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn upsert_mcp_server(text: &str, name: &str, server: &Value) -> Result<String> {
    let before = parse_config(text)?;
    let updated = if text_without_comments(text).is_empty() {
        let mut body = text.to_string();
        if !body.is_empty() && !body.ends_with(['\n', '\r']) {
            body.push('\n');
        }
        body.push_str(&fresh_config(name, server));
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body
    } else {
        splice_server(text, name, server)?
    };
    let after = parse_config(&updated)?;
    ensure_preserved(&before, &after, name, server)?;
    Ok(updated)
}

fn fresh_config(name: &str, server: &Value) -> String {
    let mut mcp = Map::new();
    mcp.insert(name.to_string(), server.clone());
    let mut root = Map::new();
    root.insert("mcp".to_string(), Value::Object(mcp));
    let mut pretty = serde_json::to_string_pretty(&Value::Object(root)).expect("json object");
    pretty.push('\n');
    pretty
}

fn parse_config(text: &str) -> Result<Value> {
    if text_without_comments(text).is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    json5::from_str(text).map_err(|err| anyhow!("Invalid JSON/JSONC: {err}"))
}

fn text_without_comments(text: &str) -> String {
    let mut cursor = Cursor::new(text);
    let _ = cursor.skip_ws_and_comments();
    cursor.rest().trim().to_string()
}

fn ensure_preserved(before: &Value, after: &Value, name: &str, server: &Value) -> Result<()> {
    let Some(before_obj) = before.as_object() else {
        bail!("config must be a JSON object");
    };
    let Some(after_obj) = after.as_object() else {
        bail!("config must be a JSON object");
    };
    for (key, value) in before_obj {
        if key != "mcp" && after_obj.get(key) != Some(value) {
            bail!("refusing to rewrite config; key {key} would change");
        }
    }
    let Some(mcp) = after_obj.get("mcp").and_then(Value::as_object) else {
        bail!("mcp must be an object");
    };
    if mcp.get(name) != Some(server) {
        bail!("mcp.{name} was not written");
    }
    if let Some(before_mcp) = before_obj.get("mcp").and_then(Value::as_object) {
        for (key, value) in before_mcp {
            if key != name && mcp.get(key) != Some(value) {
                bail!("refusing to rewrite config; mcp.{key} would change");
            }
        }
    }
    Ok(())
}

fn splice_server(text: &str, name: &str, server: &Value) -> Result<String> {
    let mut cursor = Cursor::new(text);
    cursor.skip_ws_and_comments()?;
    if cursor.peek() != Some('{') {
        bail!("config must be a JSON object");
    }
    let root = cursor.parse_object()?;
    cursor.skip_ws_and_comments()?;
    if !cursor.eof() {
        bail!("config must be a JSON object");
    }

    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let Some(mcp_prop) = root.props.iter().rev().find(|prop| prop.key == "mcp") else {
        let indent = object_child_indent(text, &root, None);
        return Ok(insert_property(
            text,
            &root,
            "mcp",
            &mcp_value(name, server),
            &indent,
            nl,
        ));
    };

    let mut nested = Cursor::new(text);
    nested.i = mcp_prop.value_start;
    if nested.peek() != Some('{') {
        bail!("mcp must be an object");
    }
    let mcp_obj = nested.parse_object()?;
    if nested.i != mcp_prop.value_end {
        bail!("mcp must be an object");
    }

    if let Some(existing) = mcp_obj.props.iter().rev().find(|prop| prop.key == name) {
        let pad = line_indent(text, existing.key_start).unwrap_or_default();
        let formatted = format_embedded(server, &pad, nl);
        let mut out = String::new();
        out.push_str(&text[..existing.value_start]);
        out.push_str(&formatted);
        out.push_str(&text[existing.value_end..]);
        return Ok(out);
    }

    let indent = object_child_indent(text, &mcp_obj, Some(mcp_prop.key_start));
    Ok(insert_property(text, &mcp_obj, name, server, &indent, nl))
}

fn mcp_value(name: &str, server: &Value) -> Value {
    let mut mcp = Map::new();
    mcp.insert(name.to_string(), server.clone());
    Value::Object(mcp)
}

fn insert_property(
    text: &str,
    object: &Obj,
    key: &str,
    value: &Value,
    indent: &str,
    nl: &str,
) -> String {
    let formatted = format_embedded(value, indent, nl);
    let key_json = serde_json::to_string(key).expect("string key");
    let entry = format!("{indent}{key_json}: {formatted}{nl}");
    let comma_at = object
        .props
        .last()
        .filter(|prop| !prop.has_comma)
        .map(|prop| prop.value_end);

    let mut out = String::new();
    if let Some(comma_at) = comma_at {
        out.push_str(&text[..comma_at]);
        out.push(',');
        out.push_str(&text[comma_at..object.close]);
    } else {
        out.push_str(&text[..object.close]);
    }
    if !preceded_by_newline(text, object.close) {
        out.push_str(nl);
    }
    out.push_str(&entry);
    out.push_str(&text[object.close..]);
    out
}

fn format_embedded(value: &Value, pad: &str, nl: &str) -> String {
    let pretty = serde_json::to_string_pretty(value).expect("json value");
    let mut lines = pretty.split('\n');
    let mut out = lines.next().unwrap_or("").to_string();
    for line in lines {
        out.push_str(nl);
        out.push_str(pad);
        out.push_str(line);
    }
    out
}

fn object_child_indent(text: &str, object: &Obj, parent_key: Option<usize>) -> String {
    if let Some(prop) = object.props.first() {
        if let Some(indent) = line_indent(text, prop.key_start) {
            return indent;
        }
    }
    if let Some(parent_key) = parent_key {
        if let Some(indent) = line_indent(text, parent_key) {
            return deeper_indent(&indent);
        }
    }
    "  ".to_string()
}

fn deeper_indent(indent: &str) -> String {
    if indent.contains('\t') && !indent.contains(' ') {
        format!("{indent}\t")
    } else {
        format!("{indent}  ")
    }
}

fn line_indent(text: &str, at: usize) -> Option<String> {
    let line_start = text[..at].rfind('\n').map(|index| index + 1).unwrap_or(0);
    let prefix = &text[line_start..at];
    prefix
        .chars()
        .all(|ch| ch == ' ' || ch == '\t')
        .then(|| prefix.to_string())
}

fn preceded_by_newline(text: &str, at: usize) -> bool {
    let bytes = text.as_bytes();
    let mut index = at;
    while index > 0 {
        index -= 1;
        match bytes[index] {
            b' ' | b'\t' => continue,
            b'\n' => return true,
            _ => return false,
        }
    }
    false
}

struct Prop {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
    has_comma: bool,
}

struct Obj {
    close: usize,
    props: Vec<Prop>,
}

struct Cursor<'a> {
    src: &'a str,
    i: usize,
}

impl<'a> Cursor<'a> {
    fn new(src: &'a str) -> Self {
        let i = if src.starts_with('\u{feff}') { 3 } else { 0 };
        Self { src, i }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.i..]
    }

    fn eof(&self) -> bool {
        self.i >= self.src.len()
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.i += ch.len_utf8();
        Some(ch)
    }

    fn skip_ws_and_comments(&mut self) -> Result<()> {
        loop {
            while self.peek().is_some_and(|ch| ch.is_whitespace()) {
                self.bump();
            }
            if self.rest().starts_with("//") {
                while let Some(ch) = self.bump() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            if self.rest().starts_with("/*") {
                self.bump();
                self.bump();
                loop {
                    if self.rest().starts_with("*/") {
                        self.bump();
                        self.bump();
                        break;
                    }
                    if self.bump().is_none() {
                        bail!("unterminated block comment");
                    }
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn expect(&mut self, expected: char) -> Result<()> {
        match self.bump() {
            Some(ch) if ch == expected => Ok(()),
            _ => bail!("expected '{expected}'"),
        }
    }

    fn parse_object(&mut self) -> Result<Obj> {
        self.expect('{')?;
        let mut props = Vec::new();
        loop {
            self.skip_ws_and_comments()?;
            if self.peek() == Some('}') {
                let close = self.i;
                self.bump();
                return Ok(Obj { close, props });
            }
            if self.eof() {
                bail!("unterminated object");
            }
            let key_start = self.i;
            let key = self.parse_key()?;
            self.skip_ws_and_comments()?;
            self.expect(':')?;
            self.skip_ws_and_comments()?;
            let value_start = self.i;
            self.skip_value()?;
            let value_end = self.i;
            self.skip_ws_and_comments()?;
            let has_comma = if self.peek() == Some(',') {
                self.bump();
                true
            } else {
                false
            };
            props.push(Prop {
                key,
                key_start,
                value_start,
                value_end,
                has_comma,
            });
            if !has_comma {
                self.skip_ws_and_comments()?;
                if self.peek() != Some('}') {
                    bail!("expected ',' or '}}'");
                }
            }
        }
    }

    fn parse_key(&mut self) -> Result<String> {
        match self.peek() {
            Some('"' | '\'') => self.parse_string(),
            Some(ch) if is_ident_start(ch) => self.parse_ident(),
            _ => bail!("expected object key"),
        }
    }

    fn parse_string(&mut self) -> Result<String> {
        let quote = self.bump().context("unterminated string")?;
        let mut out = String::new();
        while let Some(ch) = self.bump() {
            if ch == '\\' {
                let escaped = self.bump().context("unterminated string escape")?;
                match escaped {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        let hex: String = (0..4).filter_map(|_| self.bump()).collect();
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| anyhow!("invalid unicode escape"))?;
                        out.push(char::from_u32(code).context("invalid unicode escape")?);
                    }
                    other => out.push(other),
                }
                continue;
            }
            if ch == quote {
                return Ok(out);
            }
            out.push(ch);
        }
        bail!("unterminated string")
    }

    fn parse_ident(&mut self) -> Result<String> {
        let start = self.i;
        self.bump();
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        Ok(self.src[start..self.i].to_string())
    }

    fn skip_value(&mut self) -> Result<()> {
        match self.peek() {
            Some('{') => {
                self.parse_object()?;
            }
            Some('[') => self.skip_array()?,
            Some('"' | '\'') => {
                self.parse_string()?;
            }
            Some(_) => self.skip_literal(),
            None => bail!("unexpected end of config"),
        }
        Ok(())
    }

    fn skip_array(&mut self) -> Result<()> {
        self.expect('[')?;
        loop {
            self.skip_ws_and_comments()?;
            if self.peek() == Some(']') {
                self.bump();
                return Ok(());
            }
            if self.eof() {
                bail!("unterminated array");
            }
            self.skip_value()?;
            self.skip_ws_and_comments()?;
            if self.peek() == Some(',') {
                self.bump();
            }
        }
    }

    fn skip_literal(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() || matches!(ch, ',' | '}' | ']' | '/') {
                break;
            }
            self.bump();
        }
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == '$'
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample(dir: &Path, xdg: &Path, global: bool) -> AddRequest {
        AddRequest {
            name: "context7".to_string(),
            url: Some("https://mcp.context7.com/mcp".to_string()),
            env: Vec::new(),
            header: Vec::new(),
            global,
            command: Vec::new(),
            cwd: Some(dir.to_path_buf()),
            xdg_config_home: Some(xdg.to_path_buf()),
        }
    }

    #[test]
    fn adds_remote_server_to_new_project_config() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        let path = add_mcp(sample(dir.path(), xdg.path(), false)).unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(path, root.join(".crabcode/crabcode.jsonc"));
        let text = fs::read_to_string(&path).unwrap();
        let value: Value = json5::from_str(&text).unwrap();
        assert_eq!(
            value["mcp"]["context7"],
            json!({
                "type": "remote",
                "url": "https://mcp.context7.com/mcp"
            })
        );
    }

    #[test]
    fn adds_local_server_with_env_and_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        let config = dir.path().join(".crabcode/crabcode.jsonc");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            &config,
            "{\n  // keep me\n  \"model\": \"openai/gpt-5\",\n  \"mcp\": {\n    \"gh_grep\": {\n      \"type\": \"remote\",\n      \"url\": \"https://mcp.grep.app\"\n    }\n  }\n}\n",
        )
        .unwrap();

        let mut request = sample(dir.path(), xdg.path(), false);
        request.name = "everything".to_string();
        request.url = None;
        request.command = vec![
            "npx".to_string(),
            "-y".to_string(),
            "server-everything".to_string(),
        ];
        request.env = vec!["LOG_LEVEL=debug".to_string(), "EMPTY=".to_string()];
        add_mcp(request).unwrap();

        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("// keep me"));
        assert!(text.contains("https://mcp.grep.app"));
        let value: Value = json5::from_str(&text).unwrap();
        assert_eq!(value["model"], "openai/gpt-5");
        assert_eq!(
            value["mcp"]["everything"],
            json!({
                "type": "local",
                "command": ["npx", "-y", "server-everything"],
                "environment": { "LOG_LEVEL": "debug", "EMPTY": "" }
            })
        );
    }

    #[test]
    fn replaces_existing_server_and_keeps_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        let config = dir.path().join("crabcode.json");
        fs::write(
            &config,
            "{\n  \"mcp\": {\n    // servers\n    \"context7\": { \"type\": \"remote\", \"url\": \"https://old.example\" },\n    \"other\": { \"type\": \"remote\", \"url\": \"https://other.example\" }\n  }\n}\n",
        )
        .unwrap();

        let mut request = sample(dir.path(), xdg.path(), false);
        request.header = vec!["X-Token=secret".to_string()];
        let path = add_mcp(request).unwrap();
        assert_eq!(
            path,
            dir.path().canonicalize().unwrap().join("crabcode.json")
        );

        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("// servers"));
        let value: Value = json5::from_str(&text).unwrap();
        assert_eq!(
            value["mcp"]["context7"]["url"],
            "https://mcp.context7.com/mcp"
        );
        assert_eq!(value["mcp"]["context7"]["headers"]["X-Token"], "secret");
        assert_eq!(value["mcp"]["other"]["url"], "https://other.example");
        assert!(!dir.path().join(".crabcode/crabcode.jsonc").exists());
    }

    #[test]
    fn global_flag_writes_the_global_crabcode_config() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        let path = add_mcp(sample(dir.path(), xdg.path(), true)).unwrap();
        assert_eq!(path, xdg.path().join("crabcode/crabcode.jsonc"));
        assert!(!dir.path().join(".crabcode/crabcode.jsonc").exists());
    }

    #[test]
    fn writes_at_the_git_project_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        let sub = dir.path().join("crates/app");
        fs::create_dir_all(&sub).unwrap();
        let xdg = tempfile::tempdir().unwrap();
        let path = add_mcp(sample(&sub, xdg.path(), false)).unwrap();
        assert_eq!(
            path,
            dir.path()
                .canonicalize()
                .unwrap()
                .join(".crabcode/crabcode.jsonc")
        );
    }

    #[test]
    fn rejects_multiple_config_files_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".crabcode")).unwrap();
        fs::write(dir.path().join(".crabcode/crabcode.jsonc"), "{}\n").unwrap();
        fs::write(dir.path().join("crabcode.json"), "{}\n").unwrap();
        let err = add_mcp(sample(dir.path(), xdg.path(), false)).unwrap_err();
        assert!(err.to_string().contains("Multiple config files"));
        assert_eq!(
            fs::read_to_string(dir.path().join(".crabcode/crabcode.jsonc")).unwrap(),
            "{}\n"
        );
    }

    #[test]
    fn rejects_mixed_local_and_remote_flags() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        let mut request = sample(dir.path(), xdg.path(), false);
        request.command = vec!["npx".to_string()];
        let err = add_mcp(request).unwrap_err();
        assert!(err.to_string().contains("not both"));

        let mut request = sample(dir.path(), xdg.path(), false);
        request.env = vec!["A=b".to_string()];
        let err = add_mcp(request).unwrap_err();
        assert!(err.to_string().contains("--env is only valid"));

        let mut request = sample(dir.path(), xdg.path(), false);
        request.url = None;
        request.header = vec!["A=b".to_string()];
        request.command = vec!["npx".to_string()];
        let err = add_mcp(request).unwrap_err();
        assert!(err.to_string().contains("--header is only valid"));

        let mut request = sample(dir.path(), xdg.path(), false);
        request.url = Some("ftp://example.com".to_string());
        let err = add_mcp(request).unwrap_err();
        assert!(err.to_string().contains("Invalid URL"));
    }
}
