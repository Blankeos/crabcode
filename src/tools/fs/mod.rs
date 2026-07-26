pub mod glob;
pub mod grep;
pub mod list;
pub mod read;
pub mod view_image;
pub mod write;

use crate::tools::ToolContext;
use std::path::PathBuf;

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
