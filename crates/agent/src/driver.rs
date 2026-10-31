//! agent 的驱动骨架：T004.1 的实现提交之前，只有 6 个入口的空壳。
//!
//! 这里不写 loop 行为；实现见后续提交。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use model::{DeepSeek, UserMessage};
use session::{SessionId, SessionStore};
use tokio::sync::broadcast;

use crate::AgentEvent;
use crate::Deps;
use crate::error::AgentError;
use crate::inbox::{Delivery, Inbox};

const EVENT_CHANNEL_CAPACITY: usize = 256;

/// agent 的共享状态（骨架）。
pub(crate) struct Inner {
    id: SessionId,
    log_path: PathBuf,
    #[allow(dead_code, reason = "骨架：loop 实现前先把依赖存下来")]
    model: DeepSeek,
    #[allow(dead_code, reason = "骨架：loop 实现前先把依赖存下来")]
    store: Mutex<SessionStore>,
    inbox: Mutex<Inbox>,
    events: broadcast::Sender<AgentEvent>,
}

impl Inner {
    pub(crate) fn start(deps: Deps, _last_turn: u64) -> Result<Arc<Self>, AgentError> {
        let Deps {
            model,
            tools: _,
            store,
        } = deps;
        let id = store.id().clone();
        let log_path = store.path().to_path_buf();
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Ok(Arc::new(Self {
            id,
            log_path,
            model,
            store: Mutex::new(store),
            inbox: Mutex::new(Inbox::default()),
            events,
        }))
    }

    pub(crate) fn id(&self) -> &SessionId {
        &self.id
    }

    pub(crate) fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    pub(crate) fn send(&self, message: UserMessage, delivery: Delivery) {
        self.inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .push(delivery.target(), message);
    }

    pub(crate) fn cancel(&self) {
        self.inbox.lock().expect("agent 收件箱锁被毒化").clear();
    }

    pub(crate) async fn idle(&self) {}
}
