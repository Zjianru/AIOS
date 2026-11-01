//! 内置只读工具：read、glob、grep。

mod glob;
mod grep;
mod read;

pub use glob::GlobTool;
pub use grep::GrepTool;
pub use read::ReadTool;

use std::path::{Component, Path, PathBuf};

use crate::ToolContext;

/// 把模型给的路径解析成绝对路径（相对会话 cwd）。
pub(crate) fn resolve_path(context: &ToolContext, requested: &str) -> PathBuf {
    let path = Path::new(requested);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        context.cwd.join(path)
    }
}

/// 会话内的展示路径：cwd 相对、/ 分隔；不在 cwd 内时用原路径。
pub(crate) fn display_path(cwd: &Path, path: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(relative) => {
            let parts: Vec<String> = relative
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                ".".to_owned()
            } else {
                parts.join("/")
            }
        }
        Err(_) => path.to_string_lossy().into_owned(),
    }
}
