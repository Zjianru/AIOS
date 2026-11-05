//! T013.3：@ 文件候选（工作目录里的文字文件；照 DSH `context/file-reference-local/src/search.ts`）。
//!
//! 和 DSH 的两处不同（spec v1.2）：
//! - 不列目录：候选只含文字文件；
//! - 排好序之后按 git 的规则判断文字（前 8000 字节无 NUL），凑满 maxResults 条为止。

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

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
    list_with(&FileSearchConfig::default(), cwd, query)
}

/// 带配置的版本（测试用）。
pub fn list_with(
    config: &FileSearchConfig,
    cwd: &Path,
    query: &str,
) -> Result<Vec<FileReference>, FileReferenceError> {
    validate_config(config)?;
    let query = query.replace('\\', "/");
    // 越界/绝对路径的查询一律给空（候选只在会话工作目录里）。
    if query.starts_with('/') || query.split('/').any(|segment| segment == "..") {
        return Ok(Vec::new());
    }
    let indexed = scan_workspace(config, cwd)?;
    let slash = query.rfind('/');
    let (directory, fragment) = match slash {
        Some(position) => (&query[..=position], &query[position + 1..]),
        None => ("", query.as_str()),
    };
    let mut ranked: Vec<(IndexedPath, i64)> = indexed
        .into_iter()
        .filter(|entry| {
            if directory.is_empty() {
                visible_for_global_query(&entry.path, &query)
            } else {
                entry.path.starts_with(directory)
            }
        })
        .filter_map(|entry| score_candidate(&entry.path, fragment).map(|score| (entry, score)))
        .collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| {
                if fragment.is_empty() {
                    std::cmp::Ordering::Equal
                } else {
                    left.0.path.len().cmp(&right.0.path.len())
                }
            })
            .then_with(|| left.0.path.cmp(&right.0.path))
    });
    // 排好序后按 git 规则判断文字，凑满 maxResults 条或看完。
    let mut references = Vec::new();
    for (entry, _) in ranked {
        if references.len() >= config.max_results {
            break;
        }
        let absolute = cwd.join(&entry.path);
        let Ok(metadata) = fs::metadata(&absolute) else {
            continue;
        };
        let Ok(text) = read_text_head(&absolute, TEXT_PROBE_BYTES) else {
            continue;
        };
        if !text {
            continue;
        }
        references.push(FileReference {
            path: entry.path,
            bytes: metadata.len(),
        });
    }
    Ok(references)
}

/// 文字判断的探测长度（照 git 的 FIRST_FEW_BYTES）。
const TEXT_PROBE_BYTES: usize = 8000;

fn validate_config(config: &FileSearchConfig) -> Result<(), FileReferenceError> {
    if config.max_results == 0 {
        return Err(FileReferenceError::Config(
            "maxResults 必须是正整数".to_owned(),
        ));
    }
    if config.max_entries == 0 {
        return Err(FileReferenceError::Config(
            "maxEntries 必须是正整数".to_owned(),
        ));
    }
    for name in &config.excluded_directories {
        if name.is_empty() || name.contains('/') || name.contains('\\') {
            return Err(FileReferenceError::Config(format!(
                "排除目录必须是非空目录名：{name:?}"
            )));
        }
    }
    Ok(())
}

/// 一个索引里的路径（文件；AIOS 不列目录）。
struct IndexedPath {
    path: String,
}

/// BFS 遍历工作目录：目录按名字排序、跳过排除目录、条目数有上限。
fn scan_workspace(
    config: &FileSearchConfig,
    cwd: &Path,
) -> Result<Vec<IndexedPath>, FileReferenceError> {
    let excluded: std::collections::HashSet<&str> = config
        .excluded_directories
        .iter()
        .map(String::as_str)
        .collect();
    let mut indexed = Vec::new();
    let mut queue: Vec<PathBuf> = vec![cwd.to_path_buf()];
    let mut relative: Vec<String> = vec![String::new()];
    let mut cursor = 0;
    while cursor < queue.len() && indexed.len() < config.max_entries {
        let absolute = queue[cursor].clone();
        let prefix = relative[cursor].clone();
        cursor += 1;
        // 根目录读不了要报错；分支读不了只丢自己的候选。
        let entries = if cursor == 1 {
            fs::read_dir(&absolute)?
        } else {
            match fs::read_dir(&absolute) {
                Ok(entries) => entries,
                Err(_) => continue,
            }
        };
        let mut sorted: Vec<_> = entries.flatten().collect();
        sorted.sort_by_key(std::fs::DirEntry::file_name);
        for entry in sorted {
            if indexed.len() >= config.max_entries {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                if excluded.contains(name.as_str()) {
                    continue;
                }
                queue.push(absolute.join(&name));
                relative.push(path);
            } else if file_type.is_file() {
                indexed.push(IndexedPath { path });
            }
        }
    }
    Ok(indexed)
}

/// 全局查询（不含 /）的隐藏过滤：query 以点开头或含 "/." 才看隐藏路径。
fn visible_for_global_query(path: &str, query: &str) -> bool {
    if query.starts_with('.') || query.contains("/.") {
        return true;
    }
    !path.split('/').any(|segment| segment.starts_with('.'))
}

/// 打分（照 DSH scoreCandidate；AIOS 没有目录加成）。
fn score_candidate(path: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let path = path.to_lowercase();
    let name = path.rsplit('/').next().unwrap_or(&path);
    let needle = query.to_lowercase();
    if name == needle {
        return Some(1_000);
    }
    if name.starts_with(&needle) {
        return Some(900);
    }
    if name.contains(&needle) {
        return Some(700);
    }
    if path.contains(&needle) {
        return Some(500);
    }
    subsequence_score(&path, &needle).map(|score| 300 + score)
}

/// 子序列命中的紧凑度（照 DSH subsequenceScore）。
fn subsequence_score(target: &str, query: &str) -> Option<i64> {
    let mut target_index = 0;
    let mut gap = 0;
    for character in query.chars() {
        let found = target[target_index..].find(character)?;
        let found = target_index + found;
        gap += found - target_index;
        target_index = found + 1;
    }
    Some((100_i64 - gap as i64).max(0))
}

/// 读文件前 probe 字节，判断是不是文字（前 8000 字节无 NUL）。
fn read_text_head(path: &Path, probe: usize) -> Result<bool, FileReferenceError> {
    let mut file = fs::File::open(path)?;
    let mut buffer = vec![0u8; probe];
    let mut filled = 0;
    while filled < probe {
        let read = file.read(&mut buffer[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(!buffer[..filled].contains(&0))
}
