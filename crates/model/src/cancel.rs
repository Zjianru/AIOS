//! 请求取消令牌（对应 DSH 的 AbortSignal）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// 一次请求/一轮的取消令牌：具体类型（不走 trait），内部共用 agent 的
/// abort 标志和 Notify，让 LLM 层能在「等响应头」阶段被打断。
#[derive(Debug, Clone)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelToken {
    pub fn new(flag: Arc<AtomicBool>, notify: Arc<Notify>) -> Self {
        Self { flag, notify }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// 等取消；返回时 is_cancelled() 为 true（不会丢唤醒）。
    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}
