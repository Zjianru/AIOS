//! skill 能力（T006.1c）：可用 skill 目录挂 PreStep。
//!
//! 行为照 T005.6：名字 + description 作为独立 user 消息；有变化才追加。
//! 手动 /compact 的 PreStep（trigger=Some(Manual)）跳过，避免手动压缩顺带注入 skill 目录。

use model::{Message, Role};
use session::EventKind;

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};

pub(crate) struct SkillCapability;

impl Capability for SkillCapability {
    fn name(&self) -> &'static str {
        "skill"
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
            let catalog = host
                .skill_catalog_state()
                .lock()
                .expect("agent skill 目录锁被毒化")
                .prepare(host.cwd(), host.data_dir(), host.skill_roots());
            if let Some(text) = catalog {
                host.append(&[EventKind::UserMessage {
                    turn: event.turn(),
                    step: event.step(),
                    message: Message::text(Role::User, text),
                }])
                .await?;
            }
            Ok(HookOutcome::Continue)
        })
    }
}
