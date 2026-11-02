//! T006.16b：Kind 根会话的找回/新建与状态文件（D44、docs/02-Kind与协作.md 第 1 节、strategy §4）。
//!
//! 一个用户只有一条 Kind 根会话：第一次启动建它，把会话 id 记在数据目录的状态文件里；
//! 以后启动只找回它。**记过 id 但会话日志缺失、或状态文件损坏，就报错，不静默新建**
//! （06 篇恢复链：缺失、损坏、身份冲突都明确报告）。
//!
//! 状态文件原子写：先写同目录临时文件、fsync、rename 覆盖；半写不会破坏已提交的状态。

use std::path::{Path, PathBuf};

use agent::{Agent, AgentConfig, AgentError};
use model::DeepSeekConfig;
use serde::{Deserialize, Serialize};
use session::SessionId;

/// 根会话状态文件名（数据目录下）。
pub const ROOT_STATE_FILE: &str = "kind-root.json";
/// 状态文件 schema 版本（AIOS 自己的；与会话日志 FORMAT_VERSION 无关）。
const ROOT_STATE_VERSION: u64 = 1;

/// 状态文件内容：只记根会话的身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RootRecord {
    version: u64,
    #[serde(rename = "sessionId")]
    session_id: String,
}

/// 根会话四种状态里的成功两态（失败两态走 RootError）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootStatus {
    /// 第一次启动：没有状态文件，新建会话并记录。
    Created,
    /// 已有状态文件且会话日志在：找回同一条根会话。
    Restored,
}

/// 根会话错误：记过但缺失 / 状态文件损坏 / 底层错误。
#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("根会话状态文件损坏：{path}（{reason}）")]
    StateCorrupt { path: String, reason: String },
    #[error("根会话记录为 {session_id}，但会话日志缺失：{path}")]
    RecordedSessionMissing { session_id: String, path: String },
    #[error("根会话尚未实现（骨架）")]
    NotImplemented,
}

/// 打开/新建根会话的输入：与 Agent::open 一致，外加根会话状态文件所在的数据目录。
#[derive(Debug, Clone)]
pub struct RootSessionOptions {
    pub data_dir: PathBuf,
    pub cwd: PathBuf,
    pub model: DeepSeekConfig,
    pub config: AgentConfig,
}

/// 根会话打开结果。
pub struct RootSession {
    pub agent: Agent,
    pub session_id: SessionId,
    pub status: RootStatus,
    /// 重启找回时是否领取了 pending inbox（DoD6 的服务重启路径；新建时总是 false）。
    pub claimed_pending: bool,
}

impl RootSession {
    /// 状态文件路径（测试/诊断用）。
    pub fn state_path(data_dir: &Path) -> PathBuf {
        data_dir.join(ROOT_STATE_FILE)
    }
}

/// 找回或新建 Kind 根会话（T006.16b）。
pub async fn open_root_session(_options: RootSessionOptions) -> Result<RootSession, RootError> {
    Err(RootError::NotImplemented)
}
