//! 指令能力（T006.1c）：AGENTS.md / CLAUDE.md 注入挂 PreStep。
//!
//! 行为照 T005.5：第一次请求带基线；之后仅在发现更深目录或文件变化时追加刷新消息。
//! 手动 /compact 的 PreStep（trigger=Some(Manual)）跳过，避免手动压缩顺带注入指令。

use std::sync::{Arc, Mutex};

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};
use crate::instructions::InstructionState;

use model::Message;
use session::EventKind;

pub(crate) struct InstructionsCapability {
    state: Arc<Mutex<InstructionState>>,
    roots: Vec<std::path::PathBuf>,
}

impl InstructionsCapability {
    /// T006.1e：从会话日志对账并装载指令注入状态（状态归能力，不再由 driver 持有）。
    pub(crate) fn load(
        store: &session::SessionStore,
        roots: Vec<std::path::PathBuf>,
    ) -> (Arc<Mutex<InstructionState>>, Vec<std::path::PathBuf>) {
        let mut state = InstructionState::default();
        state.rehydrate(&store.derive_messages());
        (Arc::new(Mutex::new(state)), roots)
    }

    pub(crate) fn new(
        (state, roots): (Arc<Mutex<InstructionState>>, Vec<std::path::PathBuf>),
    ) -> Self {
        Self { state, roots }
    }
}

impl Capability for InstructionsCapability {
    fn name(&self) -> &'static str {
        "instructions"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::PreStep]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::PreStep { trigger, phase, .. } = event else {
                return Ok(HookOutcome::Continue);
            };
            if *phase != PreStepPhase::Inject || trigger.is_some() {
                return Ok(HookOutcome::Continue);
            }
            let host = event.host();
            let observed: Vec<std::path::PathBuf> = host
                .tool_session()
                .observations()
                .paths()
                .cloned()
                .collect();
            let injection = self.state.lock().expect("agent 指令锁被毒化").prepare(
                host.cwd(),
                host.data_dir(),
                &self.roots,
                observed.into_iter(),
            );
            if let Some(text) = injection {
                host.append(&[EventKind::UserMessage {
                    turn: event.turn(),
                    step: event.step(),
                    message: Message::user_text(text),
                }])
                .await?;
            }
            Ok(HookOutcome::Continue)
        })
    }
}
