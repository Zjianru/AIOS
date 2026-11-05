//! T013.2：附件仓库（文字文件的只读副本）。
//!
//! 照 DSH `fs/tool-fs/src/file-store.ts` + `admission.ts`：
//! - `<root>/` 下按 sha256 存一份内容（file-objects/<前2>/<sha>），再按文件名给只读副本（files/<前2>/<sha>/<name>）；
//! - 文字判断照 git：前 8000 字节里有 NUL 就不是文字，拒收（错误码 not-text）；
//! - 不设大小/个数上限，不做清理（日志里还引用着）。

use std::path::{Path, PathBuf};

/// 文字判断的探测长度（照 git `xdiff-interface.c` 的 FIRST_FEW_BYTES）。
pub const TEXT_PROBE_BYTES: usize = 8000;

/// 拒收二进制文件的错误码（服务/页面按码处理）。
pub const NOT_TEXT_CODE: &str = "not-text";

/// 存好的一份附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttachment {
    /// `sha256:<64 位小写 hex>`。
    pub id: String,
    pub name: String,
    /// 内容字节数。
    pub bytes: usize,
    /// 只读副本的绝对路径（模型层投递 fileHandleText 用）。
    pub path: PathBuf,
}

/// 附件仓库的错误。
#[derive(Debug)]
pub enum AttachmentError {
    /// 前 8000 字节里出现 NUL：不是文字文件。
    NotText,
    /// 文件名不合法（空、含路径分隔符、. / ..、超长）。
    BadName(String),
    /// id 不合法（只认 sha256:<64 位小写 hex>）。
    BadId(String),
    Io(std::io::Error),
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotText => formatter.write_str("附件不是文字文件"),
            Self::BadName(name) => write!(formatter, "附件名不合法：{name}"),
            Self::BadId(id) => write!(formatter, "附件 id 不合法：{id}"),
            Self::Io(error) => write!(formatter, "附件读写失败：{error}"),
        }
    }
}

impl std::error::Error for AttachmentError {}

impl From<std::io::Error> for AttachmentError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// 前 8000 字节里有没有 NUL（照 git：有就不是文字）。
pub fn is_text(data: &[u8]) -> bool {
    // T013.2 红阶段：实现前空壳（一律按二进制处理）。
    let _ = data;
    false
}

/// 存一份附件：内容按 sha256 存一份，再给只读命名副本。返回 id/name/bytes/路径。
pub fn store(root: &Path, name: &str, data: &[u8]) -> Result<StoredAttachment, AttachmentError> {
    // T013.2 红阶段：实现前空壳。
    let _ = (root, name, data);
    Err(AttachmentError::Io(std::io::Error::other(
        "attachment store is not implemented yet",
    )))
}

/// 读回一个附件（按 id）。
pub fn read(root: &Path, id: &str) -> Result<StoredAttachment, AttachmentError> {
    // T013.2 红阶段：实现前空壳。
    let _ = (root, id);
    Err(AttachmentError::Io(std::io::Error::other(
        "attachment read is not implemented yet",
    )))
}
