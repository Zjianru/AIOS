//! 会话 id。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 会话 id：由 uuid 生成的 v4 字符串，也是会话 JSONL 的文件名。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// 解析外部传入的会话 id；只接受 uuid 形式，避免路径注入。
    pub fn parse(raw: &str) -> Result<Self, crate::SessionError> {
        let uuid = uuid::Uuid::parse_str(raw)
            .map_err(|_| crate::SessionError::InvalidSessionId(raw.to_owned()))?;
        Ok(Self(uuid.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
