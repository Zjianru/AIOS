//! 大工具结果的 spill 存储、token 保留与模型可见通知（照 DSH 的 spill/spill-policy）。
//!
//! 本模块只管「结果太大时怎么落盘、怎么留 preview」；什么时候调用由 agent 驱动决定。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ToolFailure;
use crate::session::SpillStore;

static SPILL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 取回指引：模型用已有工具按路径取全文（照 DSH spill-local 的 retrievalHint）。
pub const SPILL_RETRIEVAL_HINT: &str =
    "Use read with offset/limit, or grep this path to search within it.";

/// 保留头/尾之间插的间隔（照 DSH spill-policy 的 GAP）。
const GAP: &str = "\n\n[...]\n\n";

/// 一次 spill 的结果：模型可读的绝对路径 + 完整文本的 UTF-8 字节数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpillRef {
    pub locator: PathBuf,
    pub bytes: usize,
}

impl SpillStore {
    /// 是否配置了可写的 spill 根。
    pub fn is_enabled(&self) -> bool {
        self.root().is_some()
    }

    /// 把完整文本写进会话自己的 spill 目录（目录 0700、文件 0600、fsync），
    /// 返回 locator 和字节数。会话目录由 SpillStore 的根固定，天然按会话隔离。
    pub fn save_text(&self, name: &str, content: &str) -> Result<SpillRef, ToolFailure> {
        let Some(root) = self.root() else {
            return Err(ToolFailure::plain(
                "spill is disabled: no session data directory",
            ));
        };
        if let Err(error) = create_private_dir(root) {
            return Err(ToolFailure::plain(format!(
                "cannot create spill directory {}: {error}",
                root.display()
            )));
        }
        let file_name = format!(
            "{}-{}.txt",
            safe_segment(name),
            SPILL_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let locator = root.join(file_name);
        let write = (|| -> std::io::Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&locator)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = write {
            return Err(ToolFailure::plain(format!(
                "cannot write spill file {}: {error}",
                locator.display()
            )));
        }
        Ok(SpillRef {
            locator,
            bytes: content.len(),
        })
    }
}

/// 目录 0700：spill 内容只属主可读（照 DSH spill-local）。
fn create_private_dir(root: &Path) -> std::io::Result<()> {
    if root.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(root)
}

/// 文件名只保留单段安全字符；其余替换成下划线（session 目录之外没有路径语义）。
fn safe_segment(name: &str) -> String {
    let mut segment: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if segment.is_empty() || segment == "." || segment == ".." {
        segment = "spill".to_owned();
    }
    segment
}

/// 模型可见 spill 通知：省略字节数 + 完整结果路径 + 取回指引。
pub fn format_spill_notice(omitted_bytes: usize, locator: &Path) -> String {
    format!(
        "(Omitted {omitted_bytes} bytes. Full formatted result stored at: {}. {SPILL_RETRIEVAL_HINT})",
        locator.display()
    )
}

/// 结果太大时返回 preview（头/尾 + 通知）；能完整放下、禁用或不需要 spill 返回 None。
///
/// 预算用启发式 token（每 4 字符 1 token + 4 块开销，照 DSH token-meter），
/// 先给通知和 GAP 预留，再把剩余预算一分为二给头和尾。
pub fn bound_result(
    store: &SpillStore,
    tool_name: &str,
    call_id: &str,
    content: &str,
    max_inline_tokens: usize,
) -> Option<String> {
    if max_inline_tokens == 0 || !store.is_enabled() {
        return None;
    }
    // 照 DSH spill-policy：read 有自己的分页上限，不再包一层 spill。
    if tool_name == "read" {
        return None;
    }
    if estimate_text_tokens(content) <= max_inline_tokens {
        return None;
    }
    let saved = store
        .save_text(&format!("{tool_name}-{call_id}"), content)
        .ok()?;
    let worst_notice = format_spill_notice(content.len(), &saved.locator);
    if estimate_text_tokens(&worst_notice) > max_inline_tokens {
        return None;
    }
    let reserved = estimate_text_tokens(GAP) + estimate_text_tokens(&format!("\n\n{worst_notice}"));
    let budget = max_inline_tokens.saturating_sub(reserved);
    let head = fit_prefix(content, budget.div_ceil(2));
    let tail = fit_suffix(&content[head.len()..], budget / 2);
    let omitted_bytes = content.len() - head.len() - tail.len();
    let notice = format_spill_notice(omitted_bytes, &saved.locator);
    let preview = if head.is_empty() && tail.is_empty() {
        notice
    } else {
        format!("{head}{GAP}{tail}\n\n{notice}")
    };
    if estimate_text_tokens(&preview) > max_inline_tokens {
        return None;
    }
    Some(preview)
}

/// 从文本开头取尽量多的字符，使 token 估计不超过 budget。
fn fit_prefix(text: &str, budget: usize) -> String {
    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    let mut low = 0usize;
    let mut high = boundaries.len();
    while low < high {
        let middle = (low + high).div_ceil(2);
        let end = boundaries.get(middle).copied().unwrap_or(text.len());
        if estimate_text_tokens(&text[..end]) <= budget {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let end = boundaries.get(low).copied().unwrap_or(text.len());
    text[..end].to_owned()
}

/// 从文本末尾取尽量多的字符，使 token 估计不超过 budget。
fn fit_suffix(text: &str, budget: usize) -> String {
    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    let total = boundaries.len();
    let mut low = 0usize;
    let mut high = total;
    while low < high {
        let middle = (low + high).div_ceil(2);
        let start = boundaries.get(total - middle).copied().unwrap_or(0);
        if estimate_text_tokens(&text[start..]) <= budget {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let start = boundaries.get(total - low).copied().unwrap_or(0);
    text[start..].to_owned()
}

/// 文本的启发式 token 估计：每 4 个字符 1 token + 4 个块开销（照 DSH token-meter）。
pub fn estimate_text_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4) + 4
}
