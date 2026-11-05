//! T013.3：@ 文件候选（工作目录里的文字文件；照 DSH `context/file-reference-local/src/search.ts`）。
//!
//! 和 DSH 的两处不同（spec v1.2）：
//! - 不列目录：候选只含文字文件；
//! - 排好序之后按 git 的规则判断文字（前 8000 字节无 NUL），凑满 maxResults 条为止。

use std::path::Path;

/// 一次查询最多返回的候选数（DSH DEFAULT_FILE_SEARCH_MAX_RESULTS）。
pub const DEFAULT_MAX_RESULTS: usize = 20;

/// 索引最多保留的条目数（DSH DEFAULT_FILE_SEARCH_MAX_ENTRIES）。
pub const DEFAULT_MAX_ENTRIES: usize = 50_000;

/// 默认跳过的目录名（DSH DEFAULT_FILE_SEARCH_EXCLUDED_DIRECTORIES）。
pub const DEFAULT_EXCLUDED_DIRECTORIES: [&str; 15] = [
    ".git",
    "node_modules",
    "dist",
    "build",
    "out",
    "coverage",
    "target",
    ".next",
    ".nuxt",
    ".turbo",
    ".venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".gradle",
];

/// 一个 @ 候选：相对工作目录的路径（用 `/` 分隔）与内容字节数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReference {
    pub path: String,
    pub bytes: u64,
}

/// 搜索配置（测试用来收紧上限；线上用 Default）。
#[derive(Debug, Clone)]
pub struct FileSearchConfig {
    pub max_results: usize,
    pub max_entries: usize,
    pub excluded_directories: Vec<String>,
}

impl Default for FileSearchConfig {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            max_entries: DEFAULT_MAX_ENTRIES,
            excluded_directories: DEFAULT_EXCLUDED_DIRECTORIES
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        }
    }
}

/// 列表失败（工作目录读不了等）；查询本身的异常一律给空数组。
#[derive(Debug)]
pub enum FileReferenceError {
    Io(std::io::Error),
    /// 配置不合法（maxResults/maxEntries 必须是正整数；排除目录必须是非空 basename）。
    Config(String),
}

impl std::fmt::Display for FileReferenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "文件候选读取失败：{error}"),
            Self::Config(detail) => write!(formatter, "文件候选配置不合法：{detail}"),
        }
    }
}

impl std::error::Error for FileReferenceError {}

impl From<std::io::Error> for FileReferenceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// 按查询列出文字文件候选（query 是 @ 后面那段；空查询给排好序的前 maxResults 条）。
pub fn list(cwd: &Path, query: &str) -> Result<Vec<FileReference>, FileReferenceError> {
    // T013.3 红阶段：实现前空壳。
    let _ = (cwd, query);
    Ok(Vec::new())
}

/// 带配置的版本（测试用）。
pub fn list_with(
    config: &FileSearchConfig,
    cwd: &Path,
    query: &str,
) -> Result<Vec<FileReference>, FileReferenceError> {
    // T013.3 红阶段：实现前空壳。
    let _ = (config, cwd, query);
    Ok(Vec::new())
}
