use std::path::Path;
use std::process::Command;

pub fn get_current_branch() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8(output.stdout).ok()?;
        let branch = branch.trim();
        if branch.is_empty() || branch == "HEAD" {
            None
        } else {
            Some(branch.to_string())
        }
    } else {
        None
    }
}

pub fn is_git_repo(path: &str) -> Option<bool> {
    let output = Command::new("git")
        .args(["-C", path, "rev-parse", "--git-dir"])
        .output()
        .ok()?;

    Some(output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_current_branch() {
        let branch = get_current_branch();
        if let Some(branch_name) = branch {
            assert!(!branch_name.is_empty());
            assert_ne!(branch_name, "HEAD");
        }
    }
}
