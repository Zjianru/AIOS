//! 大工具结果的 spill 存储、token 保留与模型可见通知（照 DSH 的 spill/spill-policy）。
//!
//! 本模块只管「结果太大时怎么落盘、怎么留 preview」；什么时候调用由 agent 驱动决定。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::ToolFailure;
use crate::session::SpillStore;

/// 取回指引：模型用已有工具按路径取全文（照 DSH spill-local 的 retrievalHint）。
pub const SPILL_RETRIEVAL_HINT: &str =
    "Use read with offset/limit, or grep this path to search within it.";

/// 保留头/尾之间插的间隔（照 DSH spill-policy 的 GAP）。
const GAP: &str = "\n\n[...]\n\n";

/// spill 文件名的随机前缀（DSH spill-local 用 randomBytes(6).hex）：uuid v4 的 6 字节，
/// 12 个十六进制字符；uuid 是既有 workspace 依赖（v4 走 OS entropy），不引新外部 crate。
fn random_prefix() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    raw[..12].to_owned()
}

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
        let Some(directory) = self.session_dir() else {
            return Err(ToolFailure::plain(
                "spill is disabled: no session data directory",
            ));
        };
        if let Err(error) = create_private_dir(&directory) {
            return Err(ToolFailure::plain(format!(
                "cannot create spill directory {}: {error}",
                directory.display()
            )));
        }
        let file_name = format!("{}-{}", random_prefix(), encode_spill_segment(name));
        let locator = directory.join(file_name);
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

impl SpillStore {
    /// 把一个大文件流式复制进 spill 目录（8KB 缓冲，不把全文读进内存），返回 locator/字节数。
    /// 用于 bash 前台超大输出：完整流已经在 jobs 目录落盘，这里只做流式搬运。
    pub fn save_file(&self, name: &str, source: &Path) -> Result<SpillRef, ToolFailure> {
        let Some(directory) = self.session_dir() else {
            return Err(ToolFailure::plain(
                "spill is disabled: no session data directory",
            ));
        };
        if let Err(error) = create_private_dir(&directory) {
            return Err(ToolFailure::plain(format!(
                "cannot create spill directory {}: {error}",
                directory.display()
            )));
        }
        let locator = directory.join(format!(
            "{}-{}",
            random_prefix(),
            encode_spill_segment(name)
        ));
        let copy = (|| -> std::io::Result<u64> {
            let mut input = fs::File::open(source)?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut output = options.open(&locator)?;
            let bytes = std::io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            Ok(bytes)
        })();
        match copy {
            Ok(bytes) => Ok(SpillRef {
                locator,
                bytes: bytes as usize,
            }),
            Err(error) => {
                let _ = fs::remove_file(&locator);
                Err(ToolFailure::plain(format!(
                    "cannot spill file {}: {error}",
                    locator.display()
                )))
            }
        }
    }
}

/// 目录 0700：spill 内容只属主可读（照 DSH spill-local）。jobs 目录复用这个 helper。
pub(crate) fn create_private_dir(root: &Path) -> std::io::Result<()> {
    if root.is_dir() {
        // 已存在目录（升级/续接的旧 data-dir）也修正成 0700。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
        }
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

impl SpillStore {
    /// 启动时一次性清理过期 spill 文件（照 DSH spill-local cleanupPeriodDays；0 = 关闭）。
    /// best-effort：失败静默，不影响会话启动。返回删除的文件数。
    pub fn cleanup(&self, period_days: u64) -> usize {
        if period_days == 0 {
            return 0;
        }
        let Some(root) = self.root() else {
            return 0;
        };
        let cutoff = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(
                period_days.saturating_mul(86_400),
            ))
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        sweep_spill_root(root, cutoff)
    }
}

/// 会话 spill 子目录名（DSH spill-local/src/store.ts:108-121：root/session-<hash>）。
/// DSH 用 sha256(sessionId).hex[..12]；AIOS 不新增依赖（D22），用 FNV-1a 64 的前 12 个十六进制
/// 字符；session id 已经过 SessionId::parse 的 uuid 校验（无路径注入面），这里只取稳定的会话隔离名。
pub fn session_dir_name(session_id: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in session_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let hex = format!("{hash:016x}");
    format!("session-{}", &hex[..12])
}

/// 单段安全路径片段（照 DSH spill-local/src/store.ts encodeSegment）：[A-Za-z0-9._-] 原样保留
/// （'~' 除外），其余按 UTF-16 码元转义成 ~XXXX；空串 -> '~'；'.'/'..' 也转义，保证不穿越。
pub fn encode_spill_segment(raw: &str) -> String {
    if raw.is_empty() {
        return "~".to_owned();
    }
    if raw == "." {
        return "~002E".to_owned();
    }
    if raw == ".." {
        return "~002E~002E".to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    for code in raw.encode_utf16() {
        let ch = char::from_u32(u32::from(code));
        let safe = matches!(ch, Some(character)
            if character != '~'
                && (character.is_ascii_alphanumeric()
                    || matches!(character, '.' | '_' | '-')));
        if safe {
            out.push(ch.expect("安全字符"));
        } else {
            out.push_str(&format!("~{code:04X}"));
        }
    }
    out
}

/// 一次性清理某个 spill 根下过期的会话文件（照 DSH spill-local cleanup：启动时扫一遍）。
/// 只处理 session-* 普通目录里的普通文件：mtime 早于 cutoff 的删除，空会话目录删除；
/// 符号链接、其他目录/文件原样保留；任何 IO 失败都忽略（best-effort）。返回删除的文件数。
pub fn sweep_spill_root(root: &Path, cutoff: std::time::SystemTime) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("session-") {
            continue;
        }
        let Ok(files) = fs::read_dir(&path) else {
            continue;
        };
        let mut kept = 0_usize;
        for file in files.flatten() {
            let Ok(file_type) = file.file_type() else {
                kept += 1;
                continue;
            };
            if !file_type.is_file() {
                kept += 1;
                continue;
            }
            let expired = file
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .is_some_and(|modified| modified < cutoff);
            if expired && fs::remove_file(file.path()).is_ok() {
                removed += 1;
            } else {
                kept += 1;
            }
        }
        if kept == 0 {
            let _ = fs::remove_dir(&path);
        }
    }
    removed
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
    _call_id: &str,
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
    let saved = store.save_text(&format!("{tool_name}.txt"), content).ok()?;
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
