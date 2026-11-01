//! 指令能力（T006.1c）：AGENTS.md / CLAUDE.md 注入挂 PreStep。
//!
//! 行为照 T005.5：第一次请求带基线；之后仅在发现更深目录或文件变化时追加刷新消息。
//! 手动 /compact 的 PreStep（trigger=Some(Manual)）跳过，避免手动压缩顺带注入指令。

use model::{Message, Role};
use session::EventKind;

use crate::capabilities::{Capability, HookEvent, HookFuture, HookPoint, PreStepPhase};

pub(crate) struct InstructionsCapability;

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
                return Ok(());
            };
            if *phase != PreStepPhase::Inject || trigger.is_some() {
                return Ok(());
            }
            let host = event.host();
            let observed: Vec<std::path::PathBuf> = host
                .tool_session()
                .observations()
                .paths()
                .cloned()
                .collect();
            let injection = host
                .instructions_state()
                .lock()
                .expect("agent 指令锁被毒化")
                .prepare(
                    host.cwd(),
                    host.data_dir(),
                    host.instruction_roots(),
                    observed.into_iter(),
                );
            if let Some(text) = injection {
                host.append(&[EventKind::UserMessage {
                    turn: event.turn(),
                    step: event.step(),
                    message: Message::text(Role::User, text),
                }])
                .await?;
            }
            Ok(())
        })
    }
}
