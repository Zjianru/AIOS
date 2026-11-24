//! Kind 写范围的边界：原样是 / 或 ~/ 开头；~ 只在核对过原话后展开。
//!
//! 这里的函数只做路径形状，不查用户原话；核对在 executor/ledger 里做。

use std::path::{Path, PathBuf};

/// 写范围必须是绝对路径或 ~/ 开头；相对路径直接拒绝。
pub(crate) fn validate(raw: &str) -> Result<&str, String> {
    let raw = raw.trim();
    if raw.starts_with('/') || raw.starts_with("~/") {
        Ok(raw)
    } else {
        Err("write_scope 要写全：以 / 或 ~/ 开头（相对路径不派）".to_owned())
    }
}

/// 展开 ~；调用方必须已经用原样字符串核对过用户原话。
pub(crate) fn expand(raw: &str) -> Result<PathBuf, String> {
    let raw = validate(raw)?;
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or_else(|| "找不到主目录，不能展开 ~".to_owned())?;
        Ok(home.join(rest))
    } else {
        Ok(PathBuf::from(raw))
    }
}

/// 写任务的起点目录：用户没点起点时用写范围所在目录；写范围是文件就取上一级。
pub(crate) fn start_dir(raw: &str, scope: &Path) -> PathBuf {
    if raw.trim().ends_with('/') || scope.is_dir() {
        scope.to_path_buf()
    } else {
        scope
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| scope.to_path_buf())
    }
}
