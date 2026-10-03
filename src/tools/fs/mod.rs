pub mod glob;
pub mod grep;
pub mod list;
pub mod read;
pub mod view_image;
pub mod write;

use crate::tools::ToolContext;
use std::path::PathBuf;

/// Keep hidden source/config files visible, but prune ignored trees by default.
/// Git ignore rules only apply inside repositories, matching ripgrep's defaults.
fn search_walker(base: &std::path::Path, include_ignored: bool) -> ignore::WalkBuilder {
    let mut walker = ignore::WalkBuilder::new(base);
    walker
        .standard_filters(!include_ignored)
        .hidden(false)
        .require_git(true)
        .filter_entry(|entry| entry.file_name() != ".git");
    walker
}

fn resolve_path(path: Option<&str>, ctx: &ToolContext) -> PathBuf {
    let path = path.filter(|value| !value.trim().is_empty()).unwrap_or(".");
    crate::tools::permission::resolve_path(path, ctx.workdir())
}

pub use glob::GlobTool;
pub use grep::GrepTool;
pub use list::ListTool;
pub use read::ReadTool;
pub use view_image::ViewImageTool;
pub use write::{WriteFilesTool, WriteTool};

#[cfg(test)]
mod tests {
    use super::{GlobTool, GrepTool};
    use crate::tools::{ToolContext, ToolHandler};
    use serde_json::json;

    #[tokio::test]
    async fn searches_respect_ignores_with_explicit_opt_out() {
        let dir = tempfile::tempdir().unwrap();
        for name in [".git", "target", "nested/.git", "src"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        std::fs::write(dir.path().join(".gitignore"), "target/\nsrc/ignored.txt\n").unwrap();
        for name in [
            "visible.txt",
            ".hidden.txt",
            "target/ignored.txt",
            "src/visible.txt",
            "src/ignored.txt",
            ".git/secret.txt",
            "nested/.git/secret.txt",
        ] {
            std::fs::write(dir.path().join(name), "search-marker").unwrap();
        }
        let visited: Vec<_> = super::search_walker(dir.path(), false)
            .build()
            .map(|entry| entry.unwrap().into_path())
            .collect();
        assert!(!visited.iter().any(|path| path.ends_with("target")));
        assert!(!visited.iter().any(|path| path.ends_with(".git")));
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let ctx = ToolContext::new("session", "message", "Plan", rx).with_workdir(dir.path());
        for tool in [
            Box::new(GlobTool::new()) as Box<dyn ToolHandler>,
            Box::new(GrepTool::new()) as Box<dyn ToolHandler>,
        ] {
            let pattern = if tool.definition().id == "glob" {
                "**/*.txt"
            } else {
                "search-marker"
            };
            for include_ignored in [false, true] {
                let result = tool
                    .execute(
                        json!({"pattern": pattern, "include_ignored": include_ignored}),
                        &ctx,
                    )
                    .await
                    .unwrap();
                assert!(result.output.contains("visible.txt"));
                assert!(result.output.contains(".hidden.txt"));
                assert_eq!(result.output.contains("ignored.txt"), include_ignored);
                assert!(!result.output.contains("secret.txt"));
            }
            // Omission has the same behavior as false, including parent ignore rules.
            let result = tool
                .execute(json!({"pattern": pattern, "path": "src"}), &ctx)
                .await
                .unwrap();
            assert!(result.output.contains("visible.txt"));
            assert!(!result.output.contains("ignored.txt"));
            // An explicitly selected root is still searchable, even if ignored.
            let result = tool
                .execute(json!({"pattern": pattern, "path": "target"}), &ctx)
                .await
                .unwrap();
            assert!(result.output.contains("ignored.txt"));
        }
    }

    #[tokio::test]
    async fn searches_respect_dot_ignore_outside_git() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".ignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.path().join("visible.txt"), "search-marker").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "search-marker").unwrap();
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let ctx = ToolContext::new("session", "message", "Plan", rx).with_workdir(dir.path());
        for tool in [
            Box::new(GlobTool::new()) as Box<dyn ToolHandler>,
            Box::new(GrepTool::new()) as Box<dyn ToolHandler>,
        ] {
            let pattern = if tool.definition().id == "glob" {
                "**/*.txt"
            } else {
                "search-marker"
            };
            let result = tool
                .execute(json!({"pattern": pattern}), &ctx)
                .await
                .unwrap();
            assert!(result.output.contains("visible.txt"));
            assert!(!result.output.contains("ignored.txt"));
            let result = tool
                .execute(json!({"pattern": pattern, "include_ignored": true}), &ctx)
                .await
                .unwrap();
            assert!(result.output.contains("ignored.txt"));
        }
    }
}
