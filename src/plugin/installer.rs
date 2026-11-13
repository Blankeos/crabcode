use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns OpenCode's shared package cache directory. Keeping this location
/// compatible lets Crabcode and OpenCode reuse installed plugin packages.
pub fn package_directory(module: &str) -> Result<PathBuf> {
    if module.is_empty()
        || module.starts_with('-')
        || module.contains('/') && !module.starts_with('@')
    {
        anyhow::bail!("invalid plugin module: {module}");
    }

    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(dirs::cache_dir)
        .context("failed to determine cache directory")?;
    Ok(cache_home.join("opencode").join("packages").join(module))
}

/// Installs a plugin into OpenCode's shared package cache and enables it in
/// the shared global configuration.
pub fn install_plugin(module: &str) -> Result<()> {
    let package_dir = package_directory(module)?;
    fs::create_dir_all(&package_dir)
        .with_context(|| format!("failed to create plugin cache {}", package_dir.display()))?;

    let status = Command::new("bun")
        .args(["add", module])
        .current_dir(&package_dir)
        .status()
        .context("failed to start `bun`; install Bun to add plugins")?;
    if !status.success() {
        anyhow::bail!("failed to install plugin `{module}`");
    }

    let config_path = global_config_path()?;
    if add_plugin_to_config(&config_path, module)? {
        println!(
            "Installed plugin `{module}` and added it to {}",
            config_path.display()
        );
    } else {
        println!(
            "Installed plugin `{module}`; it is already configured in {}",
            config_path.display()
        );
    }
    Ok(())
}

/// Returns the shared global OpenCode config consumed by both applications.
pub fn global_config_path() -> Result<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::config_dir)
        .context("failed to determine config directory")?;
    let opencode_dir = config_home.join("opencode");
    for name in ["opencode.jsonc", "opencode.json"] {
        let path = opencode_dir.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }
    Ok(opencode_dir.join("opencode.jsonc"))
}

/// Adds `module` to the global Crabcode plugin configuration if not already
/// present. Existing JSONC is parsed leniently and written back as JSON.
pub fn add_plugin_to_config(path: &Path, module: &str) -> Result<bool> {
    let mut config = if path.exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        json5::from_str::<Value>(&content)
            .with_context(|| format!("invalid JSON/JSONC in {}", path.display()))?
    } else {
        Value::Object(Map::new())
    };

    let object = config
        .as_object_mut()
        .context("plugin config must be a JSON object")?;
    let plugins = object
        .entry("plugin")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .context("config key `plugin` must be an array")?;

    if plugins.iter().any(|entry| match entry {
        Value::String(source) => source == module,
        Value::Array(values) => values.first().and_then(Value::as_str) == Some(module),
        _ => false,
    }) {
        return Ok(false);
    }

    plugins.push(Value::String(module.to_owned()));
    let content = serde_json::to_string_pretty(&config)? + "\n";
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    let temporary = path.with_extension("jsonc.tmp");
    fs::write(&temporary, content)
        .with_context(|| format!("failed to write config {}", temporary.display()))?;
    fs::rename(&temporary, path)
        .with_context(|| format!("failed to update config {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_plugin_and_preserves_other_config() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("crabcode.jsonc");
        fs::write(&path, "// comment\n{ model: 'test', plugin: ['one'] }").unwrap();

        assert!(add_plugin_to_config(&path, "two").unwrap());
        let value: Value = json5::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["model"], "test");
        assert_eq!(value["plugin"], serde_json::json!(["one", "two"]));
        assert!(!add_plugin_to_config(&path, "two").unwrap());
    }

    #[test]
    fn creates_config_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested/crabcode.jsonc");
        assert!(add_plugin_to_config(&path, "@scope/plugin@latest").unwrap());
        let value: Value = json5::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["plugin"], serde_json::json!(["@scope/plugin@latest"]));
    }
}
