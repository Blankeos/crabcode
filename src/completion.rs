//! `crabcode completion <shell>` — clap_complete scripts, grok-style install.

use anyhow::{Context, Result};
use clap::CommandFactory;
use clap_complete::{generate, shells::Shell};
use std::io::Write;
use std::path::{Path, PathBuf};

const RC_BEGIN: &str = "# >>> crabcode installer >>>";
const RC_END: &str = "# <<< crabcode installer <<<";

pub fn generate_script(shell: Shell) -> Vec<u8> {
    let mut command = crate::Args::command();
    let mut output = Vec::new();
    generate(shell, &mut command, "crabcode", &mut output);
    if shell == Shell::Zsh {
        let raw = String::from_utf8(output).expect("clap_complete is UTF-8");
        fix_zsh_root_prompt_positional(&raw).into_bytes()
    } else {
        output
    }
}

pub fn run(shell: Shell, install: bool) -> Result<()> {
    if install {
        install_completion(shell)
    } else {
        std::io::stdout().write_all(&generate_script(shell))?;
        Ok(())
    }
}

fn install_completion(shell: Shell) -> Result<()> {
    let script_path = script_path(shell)?;
    if let Some(parent) = script_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut script = generate_script(shell);
    let aliases = if shell == Shell::Zsh {
        let rc = std::fs::read_to_string(resolve_existing_path(&zshrc_path())).unwrap_or_default();
        let aliases = crabcode_aliases_from_rc(&rc);
        script = with_zsh_compdef_names(&String::from_utf8(script).expect("UTF-8"), &aliases)
            .into_bytes();
        aliases
    } else {
        Vec::new()
    };
    write_atomic(&script_path, &script)?;
    println!("script {}", display_home_path(&script_path));

    if let Some(hook) = ensure_shell_hook(shell, &script_path, &aliases)? {
        println!("hook   {}", display_home_path(&hook));
    }
    println!("Restart the shell and press Tab.");
    Ok(())
}

fn script_path(shell: Shell) -> Result<PathBuf> {
    let home = home_dir();
    Ok(match shell {
        Shell::Bash => home.join(".local/share/bash-completion/completions/crabcode"),
        Shell::Zsh => home.join(".local/share/zsh/site-functions/_crabcode"),
        Shell::Fish => fish_config_dir().join("completions/crabcode.fish"),
        Shell::Elvish => home.join(".config/elvish/lib/crabcode.elv"),
        Shell::PowerShell => home.join(".local/share/powershell/Completions/crabcode.ps1"),
        _ => anyhow::bail!("unsupported shell {shell}"),
    })
}

fn ensure_shell_hook(shell: Shell, script: &Path, aliases: &[String]) -> Result<Option<PathBuf>> {
    match shell {
        Shell::Zsh => {
            let path = resolve_existing_path(&zshrc_path());
            upsert_rc(&path, &zsh_installer_block(script, aliases))?;
            Ok(Some(path))
        }
        Shell::Bash => {
            let path = resolve_existing_path(&home_dir().join(".bashrc"));
            upsert_rc(
                &path,
                &format!(
                    "{RC_BEGIN}\n[[ -r {script} ]] && source {script}\n{RC_END}\n",
                    script = display_home_path(script)
                ),
            )?;
            Ok(Some(path))
        }
        Shell::Fish => Ok(None),
        Shell::Elvish => {
            let path = home_dir().join(".config/elvish/rc.elv");
            upsert_rc(
                &path,
                &format!(
                    "{RC_BEGIN}\neval (slurp <{script})\n{RC_END}\n",
                    script = display_home_path(script)
                ),
            )?;
            Ok(Some(path))
        }
        Shell::PowerShell => {
            let path = powershell_profile_path();
            upsert_rc(
                &path,
                &format!(
                    "{RC_BEGIN}\n. {script}\n{RC_END}\n",
                    script = display_home_path(script)
                ),
            )?;
            Ok(Some(path))
        }
        _ => Ok(None),
    }
}

fn zsh_installer_block(script: &Path, aliases: &[String]) -> String {
    let dir = script.parent().map_or_else(
        || "~/.local/share/zsh/site-functions".to_string(),
        display_home_path,
    );
    let mut names = vec!["crabcode".to_string()];
    names.extend(aliases.iter().cloned());
    let names = names.join(" ");
    format!(
        "{RC_BEGIN}\nfpath=({dir} $fpath)\n(( $+functions[compdef] )) && autoload -Uz _crabcode && compdef _crabcode {names}\n{RC_END}\n"
    )
}

fn crabcode_aliases_from_rc(rc: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in rc.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("alias ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty()
            || name == "crabcode"
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        let value = value
            .trim()
            .trim_matches(['\'', '"'])
            .split_whitespace()
            .next()
            .unwrap_or("");
        if value == "crabcode" && !names.iter().any(|existing| existing == name) {
            names.push(name.to_string());
        }
    }
    names
}

fn with_zsh_compdef_names(script: &str, extra: &[String]) -> String {
    if extra.is_empty() {
        return script.to_string();
    }
    let mut names = vec!["crabcode".to_string()];
    names.extend(extra.iter().cloned());
    let header = format!("#compdef {}", names.join(" "));
    let sourced = format!("    compdef _crabcode {}", names.join(" "));
    let mut out = String::new();
    for (i, line) in script.lines().enumerate() {
        if i == 0 && line.starts_with("#compdef ") {
            out.push_str(&header);
        } else if line.trim_start().starts_with("compdef _crabcode") {
            out.push_str(&sourced);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn upsert_rc(path: &Path, block: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let next = upsert_marked_block(&existing, block);
    if next != existing {
        write_atomic(path, next.as_bytes())?;
    }
    Ok(())
}

fn upsert_marked_block(rc: &str, block: &str) -> String {
    let rc = strip_inline_usage_completion(rc);
    let rc = strip_marked_block(&rc);
    let mut out = rc.trim_end().to_string();
    if !out.is_empty() {
        out.push('\n');
        out.push('\n');
    }
    out.push_str(block.trim_end());
    out.push('\n');
    out
}

fn strip_marked_block(rc: &str) -> String {
    let Some(start) = rc.find(RC_BEGIN) else {
        return rc.to_string();
    };
    let Some(end_rel) = rc[start..].find(RC_END) else {
        return rc.to_string();
    };
    let end = start + end_rel + RC_END.len();
    let mut out = String::new();
    out.push_str(rc[..start].trim_end());
    let rest = rc[end..].trim_start_matches(['\r', '\n']);
    if !rest.is_empty() {
        if !out.is_empty() {
            out.push('\n');
            out.push('\n');
        }
        out.push_str(rest);
    }
    if rc.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn strip_inline_usage_completion(rc: &str) -> String {
    let lines: Vec<&str> = rc.lines().collect();
    let start = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed == "#compdef crabcode"
            || trimmed.starts_with("# @generated by usage-argv for `crabcode")
    });
    let Some(start) = start else {
        return rc.to_string();
    };
    let Some(end) = lines.iter().skip(start).position(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("compdef _crabcode")
    }) else {
        return rc.to_string();
    };
    let end = start + end;
    let mut kept = Vec::with_capacity(lines.len());
    kept.extend_from_slice(&lines[..start]);
    if end + 1 < lines.len() {
        let rest = &lines[end + 1..];
        let skip = rest
            .iter()
            .take_while(|line| line.trim().is_empty())
            .count();
        kept.extend_from_slice(&rest[skip..]);
    }
    let mut out = kept.join("\n");
    if rc.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// clap_complete + optional `[PROMPT]` puts the subcommand in `$line[2]` (clap#6282).
fn fix_zsh_root_prompt_positional(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    script
        .lines()
        .filter(|line| !line.starts_with("'::prompt -- "))
        .for_each(|line| {
            out.push_str(line);
            out.push('\n');
        });
    for (from, to) in [
        (
            r#"words=($line[2] "${words[@]}")"#,
            r#"words=($line[1] "${words[@]}")"#,
        ),
        (
            r#"curcontext="${curcontext%:*:*}:crabcode-command-$line[2]:""#,
            r#"curcontext="${curcontext%:*:*}:crabcode-command-$line[1]:""#,
        ),
        (r#"case $line[2] in"#, r#"case $line[1] in"#),
    ] {
        out = out.replacen(from, to, 1);
    }
    out
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn display_home_path(path: &Path) -> String {
    path.strip_prefix(home_dir())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

fn zshrc_path() -> PathBuf {
    std::env::var_os("ZDOTDIR")
        .map(PathBuf::from)
        .unwrap_or_else(home_dir)
        .join(".zshrc")
}

fn fish_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
        .join("fish")
}

fn powershell_profile_path() -> PathBuf {
    if cfg!(windows) {
        home_dir().join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1")
    } else {
        home_dir().join(".config/powershell/Microsoft.PowerShell_profile.ps1")
    }
}

fn resolve_existing_path(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::read_link(&current) {
            Ok(target) => {
                current = if target.is_absolute() {
                    target
                } else if let Some(parent) = current.parent() {
                    parent.join(target)
                } else {
                    target
                };
            }
            Err(_) => break,
        }
    }
    current
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("crabcode.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).or_else(|_| {
        std::fs::copy(&tmp, path)?;
        std::fs::remove_file(&tmp)?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_script_drops_prompt_slot() {
        let raw = String::from_utf8(generate_script(Shell::Zsh)).unwrap();
        assert!(raw.starts_with("#compdef crabcode"));
        assert!(
            !raw.contains("::prompt"),
            "prompt positional must not appear"
        );
        assert!(
            !raw.contains("$line[2]"),
            "root dispatch must be on $line[1]"
        );
        assert!(raw.contains("_crabcode_commands") || raw.contains("crabcode-command-$line[1]"));
    }

    #[test]
    fn zsh_hook_appends_after_grok_and_is_idempotent() {
        let script = Path::new("/Users/carlo/.local/share/zsh/site-functions/_crabcode");
        let rc = "alias cc=crabcode\n# >>> grok installer >>>\nfpath=(~/.grok/completions/zsh $fpath)\nautoload -Uz compinit && compinit -C\n# <<< grok installer <<<\n";
        let aliases = crabcode_aliases_from_rc(rc);
        let once = upsert_marked_block(rc, &zsh_installer_block(script, &aliases));
        let grok_end = once.find("# <<< grok installer <<<").unwrap();
        let crab = once.find(RC_BEGIN).unwrap();
        assert!(crab > grok_end, "append after grok; do not nest");
        assert!(once.contains("fpath=(~/.local/share/zsh/site-functions $fpath)"));
        assert!(once.contains("autoload -Uz _crabcode"));
        assert!(once.contains("compdef _crabcode crabcode cc"));
        assert!(!once.contains("source "));
        assert_eq!(
            once,
            upsert_marked_block(&once, &zsh_installer_block(script, &aliases))
        );
        assert_eq!(once.matches("autoload -Uz compinit").count(), 1);
    }

    #[test]
    fn scans_zshrc_aliases_without_hardcoding_cc() {
        let rc = concat!(
            "alias cc=\"crabcode\"\n",
            "alias crc='crabcode'\n",
            "# alias nope=crabcode\n",
            "alias lg=lazygit\n",
            "alias gcc=gcc\n",
        );
        assert_eq!(
            crabcode_aliases_from_rc(rc),
            vec!["cc".to_string(), "crc".to_string()]
        );
        assert!(crabcode_aliases_from_rc("").is_empty());
        let script = with_zsh_compdef_names(
            "#compdef crabcode\n    compdef _crabcode crabcode\n",
            &crabcode_aliases_from_rc(rc),
        );
        assert!(script.starts_with("#compdef crabcode cc crc\n"));
    }
}
