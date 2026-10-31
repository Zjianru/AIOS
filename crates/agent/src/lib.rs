//! agent loop：一轮多步的驱动、收件箱，和 Agent 的 6 个入口。
//!
//! 机制照 DSH 的 packages/core/agent-loop（strategy 第 1 节）。

mod driver;
mod error;
mod inbox;

use std::path::Path;
use std::sync::Arc;

use model::{DeepSeek, UserMessage};
use session::{SessionId, SessionStore, TurnEndReason};
use tools::ToolSet;

pub use error::AgentError;
pub use inbox::Delivery;

/// 一轮的结束结果（订阅者看到的形式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    MaxTokens,
    Aborted,
    Error { code: String, message: String },
}

impl From<&TurnEndReason> for TurnOutcome {
    fn from(reason: &TurnEndReason) -> Self {
        match reason {
            TurnEndReason::Completed => Self::Completed,
            TurnEndReason::MaxTokens => Self::MaxTokens,
            TurnEndReason::Aborted => Self::Aborted,
            TurnEndReason::Error { code, message } => Self::Error {
                code: code.clone(),
                message: message.clone(),
            },
        }
    }
}

/// 订阅者收到的 agent 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// 一段模型流出的文本（CLI 据此显示）。
    AssistantTextDelta { text: String },
    /// 一轮结束；idle() 返回之前一定已经发出。
    TurnEnded { turn: u64, outcome: TurnOutcome },
}

/// agent 运行的依赖，都是具体类型（策略稿 5.A）。
pub struct Deps {
    pub model: DeepSeek,
    pub tools: ToolSet,
    pub store: SessionStore,
}

/// T004 的 agent：create/resume/send/cancel/subscribe/idle 是唯一入口。
#[derive(Clone)]
pub struct Agent {
    inner: Arc<driver::Inner>,
}

impl Agent {
    /// 新会话上启动 agent（会话日志由 store 建好，尚无事件）。
    pub async fn create(deps: Deps) -> Result<Self, AgentError> {
        Ok(Self {
            inner: driver::Inner::start(deps, 0)?,
        })
    }

    /// 在已有会话上报文续接；从日志里最后一个 turn 编号之后继续。
    pub async fn resume(id: SessionId, deps: Deps) -> Result<Self, AgentError> {
        if deps.store.id() != &id {
            return Err(AgentError::ResumeIdMismatch {
                requested: id.as_str().to_owned(),
                store: deps.store.id().as_str().to_owned(),
            });
        }
        let last_turn = deps
            .store
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                session::EventKind::TurnStart { turn } => Some(*turn),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        Ok(Self {
            inner: driver::Inner::start(deps, last_turn)?,
        })
    }

    /// 投递一条用户消息。delivery 决定它进入下一轮还是下一步。
    pub fn send(&self, message: UserMessage, delivery: Delivery) {
        self.inner.send(message, delivery);
    }

    /// 取消：清空收件箱；正在跑的一轮在下一次检查点停下。
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// 订阅 agent 事件。
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent> {
        self.inner.subscribe()
    }

    /// 等到 agent 空闲（没有正在跑的轮次，也没有会唤醒它的排队输入）。
    pub async fn idle(&self) {
        self.inner.idle().await;
    }

    pub fn id(&self) -> &SessionId {
        self.inner.id()
    }

    pub fn log_path(&self) -> &Path {
        self.inner.log_path()
    }
}
