//! agent 层错误。

use thiserror::Error;

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
}

impl AgentError {
    /// 写进 turn/end 的稳定错误码。
    pub(crate) fn code(&self) -> String {
        match self {
            Self::Store(_) | Self::CorruptInbox(_) => "STORE".to_owned(),
            Self::Model(error) => error.code.as_str(),
            Self::ResumeIdMismatch { .. }
            | Self::StepCleanup { .. }
            | Self::InvalidConfig { .. } => "AGENT".to_owned(),
        }
    }
}
