//! 会话层错误。

use thiserror::Error;

/// 会话日志的读写错误。
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("会话 id 非法: {0}")]
    InvalidSessionId(String),
    #[error("会话日志不存在: {0}")]
    NotFound(String),
    /// 写会话已被另一个活着的写者持有（T006.5b 单写者锁；照 DSH SessionAlreadyOwnedError）。
    #[error("会话 {id} 正被另一个写者持有: {path}")]
    AlreadyOwned { id: String, path: String },
    #[error("会话日志为空或缺少 header: {0}")]
    MissingHeader(String),
    #[error("会话日志 header 损坏: {0}")]
    CorruptHeader(String),
    #[error("会话日志格式版本 {version} 不受支持（当前支持 {supported}）")]
    UnsupportedVersion { version: u64, supported: u64 },
    #[error("会话日志第 {line} 行损坏: {detail}")]
    CorruptLine { line: usize, detail: String },
    #[error("会话日志 seq 不连续: 期望 {expected}，读到 {actual}")]
    SeqDiscontinuity { expected: u64, actual: u64 },
    #[error("会话日志文件系统错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("会话日志 JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("丢弃未提交的日志尾部失败: {source}")]
    Truncate {
        #[source]
        source: std::io::Error,
    },
    #[error("写入会话日志失败（已回滚到写之前的长度）: {source}")]
    Write {
        #[source]
        source: std::io::Error,
    },
    #[error("写入会话日志失败，且回滚也失败: write={write}, rollback={rollback}")]
    WriteRollback { write: String, rollback: String },
}
