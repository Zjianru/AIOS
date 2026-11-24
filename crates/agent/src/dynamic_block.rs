//! 可动态更新的固定块：来源每步现读，内容变了就在末尾补一条程序消息。
//!
//! 只做通用机制；具体内容（项目清单）由服务装配时给来源。身份用正文 FNV-1a，
//! 续接/压缩从会话日志恢复，不重复补同一条。

use std::sync::{Arc, Mutex};

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};
use model::{Message, MessageSource};
use session::EventKind;

const RPC_PREFIX: &str = "dynamic-block:";

fn digest(text: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

/// 动态固定块：标题和 kind 由来源给，agent 只负责内容变了在末尾补一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicBlock {
    pub title: String,
    pub kind: String,
    pub text: String,
}

pub type DynamicBlockSource = Arc<dyn Fn() -> Option<DynamicBlock> + Send + Sync>;

pub(crate) struct DynamicBlockState {
    source: DynamicBlockSource,
    last_seen: Arc<Mutex<Option<String>>>,
}

impl DynamicBlockState {
    pub(crate) fn load(store: &session::SessionStore, source: DynamicBlockSource) -> Self {
        let mut last_seen = None;
        for message in store.derive_messages() {
            if let MessageSource::User {
                rpc_id: Some(rpc_id),
            } = &message.source
                && let Some(identity) = rpc_id.strip_prefix(RPC_PREFIX)
            {
                last_seen = Some(identity.to_owned());
            }
        }
        Self {
            source,
            last_seen: Arc::new(Mutex::new(last_seen)),
        }
    }
}

pub(crate) struct DynamicBlockCapability {
    state: DynamicBlockState,
}

impl DynamicBlockCapability {
    pub(crate) fn new(state: DynamicBlockState) -> Self {
        Self { state }
    }
}

impl Capability for DynamicBlockCapability {
    fn name(&self) -> &'static str {
        "dynamic-block"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::PreStep]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::PreStep {
                turn,
                step,
                phase,
                host,
                ..
            } = event
            else {
                return Ok(HookOutcome::Continue);
            };
            if *phase != PreStepPhase::FixedBlocks {
                return Ok(HookOutcome::Continue);
            }
            let Some(block) = (self.state.source)() else {
                return Ok(HookOutcome::Continue);
            };
            let identity = digest(&block.text);
            if self
                .state
                .last_seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_deref()
                == Some(identity.as_str())
            {
                return Ok(HookOutcome::Continue);
            }
            let message = Message::user_text_with_request_id(
                crate::instructions::frame_system_reminder(&format!(
                    "{}
{}",
                    block.title, block.text
                )),
                RPC_PREFIX.to_owned() + &identity,
            );
            host.append(&[EventKind::UserMessage {
                turn: *turn,
                step: *step,
                message,
            }])
            .await?;
            *self
                .state
                .last_seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(identity);
            Ok(HookOutcome::Continue)
        })
    }
}
