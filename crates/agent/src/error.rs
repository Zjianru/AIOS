//! agent 层错误。

use thiserror::Error;

/// 手动 /compact 的预期失败分类（T006.19 照 DSH compaction 的 ManualCompactionErrorCode：
/// busy / cancelled / changed / summary / commit / persistence，见 compaction/src/index.ts:35-46）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualCompactionErrorCode {
    Busy,
    Cancelled,
    Changed,
    Summary,
    Commit,
    Persistence,
}

impl ManualCompactionErrorCode {
    /// 稳定分类名（DSH 的 code 字符串）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Cancelled => "cancelled",
            Self::Changed => "changed",
            Self::Summary => "summary",
            Self::Commit => "commit",
            Self::Persistence => "persistence",
        }
    }

    /// DSH command-compact 的用户可见文案（command-compact/src/index.ts:24-56 expectedFailure）。
    pub fn text(self) -> &'static str {
        match self {
            Self::Busy => {
                "Compaction is unavailable because this process has an active compaction, or the agent is not idle."
            }
            Self::Cancelled => "Compaction cancelled.",
            Self::Changed => {
                "The history selected for compaction changed before it could be replaced. The attempt is recorded in the session log."
            }
            Self::Summary => {
                "Compaction could not produce a useful summary. The attempt is recorded in the session log."
            }
            Self::Commit => {
                "Compaction did not finish cleanly; some session history may have changed. Inspect the current session state before retrying."
            }
            Self::Persistence => "Compaction finished, but the session could not be saved.",
        }
    }
}

/// agent 的错误。
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("会话日志不可用: {0}")]
    Store(#[from] session::SessionError),
    #[error("模型请求失败: {0}")]
    Model(#[from] model::ModelError),
    #[error("续接的会话 id 不匹配：请求 {requested}，但 store 是 {store}")]
    ResumeIdMismatch { requested: String, store: String },
    #[error("step 失败且收尾也失败: first={first}, second={second}")]
    StepCleanup { first: String, second: String },
    #[error("AgentConfig 非法：{field}（{message}）")]
    InvalidConfig {
        field: &'static str,
        message: String,
    },
    #[error("会话日志里的输入队列损坏: {0}")]
    CorruptInbox(String),
    #[error("压缩失败: {0}")]
    Compaction(String),
    #[error("手动压缩失败（{code:?}）: {message}")]
    ManualCompaction {
        code: ManualCompactionErrorCode,
        message: String,
    },
}

impl AgentError {
    /// 写进 turn/end 的稳定错误码。
    pub(crate) fn code(&self) -> String {
        match self {
            Self::Store(_) | Self::CorruptInbox(_) => "STORE".to_owned(),
            Self::Model(error) => error.code.as_str(),
            Self::ResumeIdMismatch { .. }
            | Self::StepCleanup { .. }
            | Self::InvalidConfig { .. }
            | Self::Compaction(_)
            | Self::ManualCompaction { .. } => "AGENT".to_owned(),
        }
    }
}
