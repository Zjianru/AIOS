//! 服务错误与线上码表：ServiceError 到 <namespace>/<kind> 的映射只在这里。

use serde::Serialize;

use crate::protocol::{PROTOCOL_VERSION, RootState};

/// 服务错误（协议层错误码的第一版；Web 适配器在 17 映射成线上错误）。
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Agent(#[from] agent::AgentError),
    #[error("回答被拒绝的问题：{0}")]
    AnswerRejected(String),
    /// 根会话不可用，方法一律拒绝；页面用 ready 里的状态显示说明页。
    #[error("Kind 的根会话不可用")]
    RootUnavailable(RootState),
    #[error(transparent)]
    Credentials(#[from] credentials::KeyringError),
    #[error("设置接口只能由根宿主提供")]
    SettingsOnBareHost,
    /// 写非密钥设置失败（非法值/落盘失败）。
    #[error("设置没保存：{0}")]
    SettingsInvalid(String),
    /// settings.json 存在但读不出来：不写、也不覆盖（带检查的读法报这个）。
    #[error("设置读不出来：{0}")]
    SettingsUnreadable(String),
    /// 还没有存 Exa 的 key，验证无从谈起（设置页的「验证」只在该存了以后出现）。
    #[error("还没有 Exa 的 key")]
    ExaKeyMissing,
    #[error("路径不合法：{0}")]
    InvalidPath(String),
    /// 附件参数/id 不合法（坏 base64、坏文件名、坏 id）。
    #[error("附件不合法：{0}")]
    AttachmentInvalid(String),
    /// 不是文字文件（前 8000 字节有 NUL），拒收。
    #[error("附件不是文字文件")]
    AttachmentNotText,
    /// 一条消息里文字、粘贴、文件都没有。
    #[error("消息是空的")]
    MessageEmpty,
    /// 任务对话打不开（任务不存在、没有会话、会话日志缺失/损坏）；文字已是可读原因。
    #[error("{0}")]
    TaskUnavailable(String),
}

/// 服务的结构化错误码（DSH remote-error 形状：`<namespace>/<kind>`，小写连字符）。
///
/// 码表只在这一处；页面按码处理，说明只进监控/诊断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceErrorCode {
    AgentInternal,
    QuestionAnswerRejected,
    RootUnavailable,
    KeyringKeyFileMissing,
    KeyringUndecryptable,
    KeyringCorrupt,
    KeyringIo,
    ConnectionMissing,
    ConnectionInvalid,
    SettingsUnavailable,
    SettingsUnreadable,
    ExaNoKey,
    ExaInvalidKey,
    RequestInvalidPath,
    RequestInvalidJson,
    AttachmentInvalid,
    AttachmentNotText,
    RequestInvalidMessage,
    TaskUnavailable,
}

impl ServiceErrorCode {
    /// 码表全集（覆盖检查用：多一个少一个都要失败）。
    pub const ALL: [ServiceErrorCode; 19] = [
        ServiceErrorCode::AgentInternal,
        ServiceErrorCode::QuestionAnswerRejected,
        ServiceErrorCode::RootUnavailable,
        ServiceErrorCode::KeyringKeyFileMissing,
        ServiceErrorCode::KeyringUndecryptable,
        ServiceErrorCode::KeyringCorrupt,
        ServiceErrorCode::KeyringIo,
        ServiceErrorCode::ConnectionMissing,
        ServiceErrorCode::ConnectionInvalid,
        ServiceErrorCode::SettingsUnavailable,
        ServiceErrorCode::SettingsUnreadable,
        ServiceErrorCode::ExaNoKey,
        ServiceErrorCode::ExaInvalidKey,
        ServiceErrorCode::RequestInvalidPath,
        ServiceErrorCode::RequestInvalidJson,
        ServiceErrorCode::AttachmentInvalid,
        ServiceErrorCode::AttachmentNotText,
        ServiceErrorCode::RequestInvalidMessage,
        ServiceErrorCode::TaskUnavailable,
    ];

    /// 线上的码名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentInternal => "agent/internal",
            Self::QuestionAnswerRejected => "question/answer-rejected",
            Self::RootUnavailable => "root/unavailable",
            Self::KeyringKeyFileMissing => "keyring/key-file-missing",
            Self::KeyringUndecryptable => "keyring/undecryptable",
            Self::KeyringCorrupt => "keyring/corrupt",
            Self::KeyringIo => "keyring/io",
            Self::ConnectionMissing => "connection/missing",
            Self::ConnectionInvalid => "connection/invalid",
            Self::SettingsUnavailable => "settings/unavailable",
            Self::SettingsUnreadable => "settings/unreadable",
            Self::ExaNoKey => "exa/no-key",
            Self::ExaInvalidKey => "exa/invalid-key",
            Self::RequestInvalidPath => "request/invalid-path",
            Self::RequestInvalidJson => "request/invalid-json",
            Self::AttachmentInvalid => "attachment/invalid",
            Self::AttachmentNotText => "attachment/not-text",
            Self::RequestInvalidMessage => "request/invalid-message",
            Self::TaskUnavailable => "task/unavailable",
        }
    }
}

impl ServiceError {
    /// 这个错误在 wire 上的码。
    pub fn code(&self) -> ServiceErrorCode {
        match self {
            Self::Agent(_) => ServiceErrorCode::AgentInternal,
            Self::AnswerRejected(_) => ServiceErrorCode::QuestionAnswerRejected,
            Self::RootUnavailable(_) => ServiceErrorCode::RootUnavailable,
            Self::Credentials(error) => match error {
                credentials::KeyringError::KeyFileMissing(_) => {
                    ServiceErrorCode::KeyringKeyFileMissing
                }
                credentials::KeyringError::Crypto(_) => ServiceErrorCode::KeyringUndecryptable,
                credentials::KeyringError::Corrupt(_) => ServiceErrorCode::KeyringCorrupt,
                credentials::KeyringError::Io(_) => ServiceErrorCode::KeyringIo,
                credentials::KeyringError::MissingConnection(_) => {
                    ServiceErrorCode::ConnectionMissing
                }
                credentials::KeyringError::InvalidConnection(_) => {
                    ServiceErrorCode::ConnectionInvalid
                }
                credentials::KeyringError::InvalidToolKey(_) => ServiceErrorCode::ExaInvalidKey,
            },
            Self::SettingsOnBareHost | Self::SettingsInvalid(_) => {
                ServiceErrorCode::SettingsUnavailable
            }
            Self::SettingsUnreadable(_) => ServiceErrorCode::SettingsUnreadable,
            Self::ExaKeyMissing => ServiceErrorCode::ExaNoKey,
            Self::InvalidPath(_) => ServiceErrorCode::RequestInvalidPath,
            Self::AttachmentInvalid(_) => ServiceErrorCode::AttachmentInvalid,
            Self::AttachmentNotText => ServiceErrorCode::AttachmentNotText,
            Self::MessageEmpty => ServiceErrorCode::RequestInvalidMessage,
            Self::TaskUnavailable(_) => ServiceErrorCode::TaskUnavailable,
        }
    }
}

/// /api 的错误体（DSH RemoteError 的线上形状）：码 + 诊断 + 可选细节。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ServiceErrorBody {
    /// 码 + 诊断 + 按码定的细节（root/unavailable 带 root 状态与协议版本）。
    pub fn from_service_error(error: &ServiceError) -> Self {
        let details = match error {
            ServiceError::RootUnavailable(root) => Some(serde_json::json!({
                "root": root,
                "protocolVersion": PROTOCOL_VERSION,
            })),
            _ => None,
        };
        Self {
            code: error.code().as_str().to_owned(),
            message: error.to_string(),
            details,
        }
    }
}
