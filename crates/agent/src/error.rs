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
}
