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
    /// T007.2：根会话不可用，方法一律拒绝；页面用 ready 里的状态显示说明页。
    #[error("Kind 的根会话不可用")]
    RootUnavailable(RootState),
    #[error(transparent)]
    Credentials(#[from] credentials::KeyringError),
    #[error("设置接口只能由根宿主提供")]
    SettingsOnBareHost,
    /// T012.9：写非密钥设置失败（非法值/落盘失败）。
    #[error("设置没保存：{0}")]
    SettingsInvalid(String),
    #[error("路径不合法：{0}")]
    InvalidPath(String),
    /// T013.2：附件参数/id 不合法（坏 base64、坏文件名、坏 id）。
    #[error("附件不合法：{0}")]
    AttachmentInvalid(String),
    /// T013.2：不是文字文件（前 8000 字节有 NUL），拒收。
    #[error("附件不是文字文件")]
    AttachmentNotText,
    /// T013.4：一条消息里文字、粘贴、文件都没有。
    #[error("消息是空的")]
    MessageEmpty,
}

/// T009.1：服务的结构化错误码（DSH remote-error 形状：`<namespace>/<kind>`，小写连字符）。
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
    RequestInvalidPath,
    RequestInvalidJson,
    AttachmentInvalid,
    AttachmentNotText,
    RequestInvalidMessage,
}

impl ServiceErrorCode {
    /// 码表全集（覆盖检查用：多一个少一个都要失败）。
    pub const ALL: [ServiceErrorCode; 15] = [
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
        ServiceErrorCode::RequestInvalidPath,
        ServiceErrorCode::RequestInvalidJson,
        ServiceErrorCode::AttachmentInvalid,
        ServiceErrorCode::AttachmentNotText,
        ServiceErrorCode::RequestInvalidMessage,
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
            Self::RequestInvalidPath => "request/invalid-path",
            Self::RequestInvalidJson => "request/invalid-json",
            Self::AttachmentInvalid => "attachment/invalid",
            Self::AttachmentNotText => "attachment/not-text",
            Self::RequestInvalidMessage => "request/invalid-message",
        }
    }
}

impl ServiceError {
    /// T009.1：这个错误在 wire 上的码。
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
            },
            Self::SettingsOnBareHost | Self::SettingsInvalid(_) => {
                ServiceErrorCode::SettingsUnavailable
            }
            Self::InvalidPath(_) => ServiceErrorCode::RequestInvalidPath,
            Self::AttachmentInvalid(_) => ServiceErrorCode::AttachmentInvalid,
            Self::AttachmentNotText => ServiceErrorCode::AttachmentNotText,
            Self::MessageEmpty => ServiceErrorCode::RequestInvalidMessage,
        }
    }
}

/// T009.1：/api 的错误体（DSH RemoteError 的线上形状）：码 + 诊断 + 可选细节。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ServiceErrorBody {
    /// T009.1：码 + 诊断 + 按码定的细节（root/unavailable 带 root 状态与协议版本）。
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
