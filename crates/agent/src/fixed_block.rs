//! T077.2：页面清单这类固定块。已有会话没有当前版本时，在下一次请求前补一条程序消息；
//! 快照身份由正文哈希决定，正文一变化，旧会话下一次请求就能看到新增内容。

use std::sync::{Arc, Mutex};

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};
use model::{Message, MessageSource};
use session::EventKind;

const RPC_PREFIX: &str = "fixed-block:";

fn digest(text: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

pub(crate) struct FixedBlockState {
    text: String,
    identity: String,
    last_seen: Arc<Mutex<Option<String>>>,
}

impl FixedBlockState {
    pub(crate) fn load(store: &session::SessionStore, text: String) -> Self {
        let identity = digest(&text);
        let mut last_seen = None;
        for message in store.derive_messages() {
            let MessageSource::User {
                rpc_id: Some(rpc_id),
            } = &message.source
            else {
                continue;
            };
            if let Some(version) = rpc_id.strip_prefix(RPC_PREFIX) {
                last_seen = Some(version.to_owned());
            }
        }
        Self {
            text,
            identity,
            last_seen: Arc::new(Mutex::new(last_seen)),
        }
    }
}

pub(crate) struct FixedBlockCapability {
    state: FixedBlockState,
}

impl FixedBlockCapability {
    pub(crate) fn new(state: FixedBlockState) -> Self {
        Self { state }
    }
}

impl Capability for FixedBlockCapability {
    fn name(&self) -> &'static str {
        "fixed-block"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::PreStep]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::PreStep { phase, .. } = event else {
                return Ok(HookOutcome::Continue);
            };
            if *phase != PreStepPhase::FixedBlocks {
                return Ok(HookOutcome::Continue);
            }
            if self
                .state
                .last_seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_deref()
                == Some(self.state.identity.as_str())
            {
                return Ok(HookOutcome::Continue);
            }
            let message = Message::user_text_with_request_id(
                format!(
                    "AIOS 固定块（内容有变化时只补这一条；不是新任务）：
{}",
                    self.state.text
                ),
                RPC_PREFIX.to_owned() + &self.state.identity,
            );
            event
                .host()
                .append(&[EventKind::UserMessage {
                    turn: event.turn(),
                    step: event.step(),
                    message,
                }])
                .await?;
            *self
                .state
                .last_seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(self.state.identity.clone());
            Ok(HookOutcome::Continue)
        })
    }
}
