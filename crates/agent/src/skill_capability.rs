//! skill 能力（T006.1c 目录；T006.11b 用户显式 /name 调用通道）。
//!
//! 行为照 DSH tool-skill：
//! - 目录：可用 skill 的 name + description 作为 source=skill-catalog 的 user 消息，有变化才追加；
//! - 用户显式 /name：只扫这一步 claim 到的 source=user 消息，空白边界的 /name 命中
//!   user-invocable skill 时注入全文（renderSkillContent，source=skill-invocation），
//!   放在目录之后（DSH：背景注入先、模型要执行的材料最后）；
//!   disable-model-invocation skill 是这条通道的专属入口，未知/不允许的名字保持普通文本。
//! 手动 /compact 的 PreStep（trigger=Some(Manual)）跳过，避免手动压缩顺带注入。

use std::sync::{Arc, Mutex};

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};
use crate::skill_catalog::SkillCatalogState;
use model::{ContentBlock, Message, MessageSource};
use session::EventKind;
use tools::{invoked_skill_names, load_user_invocable_skill, render_skill_content};

pub(crate) struct SkillCapability {
    state: Arc<Mutex<SkillCatalogState>>,
    roots: Vec<std::path::PathBuf>,
}

impl SkillCapability {
    /// T006.1e：从会话日志对账并装载 skill 目录状态（状态归能力）。
    pub(crate) fn load(
        store: &session::SessionStore,
        roots: Vec<std::path::PathBuf>,
    ) -> (Arc<Mutex<SkillCatalogState>>, Vec<std::path::PathBuf>) {
        let mut state = SkillCatalogState::default();
        state.rehydrate(&store.derive_messages());
        (Arc::new(Mutex::new(state)), roots)
    }

    pub(crate) fn new(
        (state, roots): (Arc<Mutex<SkillCatalogState>>, Vec<std::path::PathBuf>),
    ) -> Self {
        Self { state, roots }
    }
}

impl Capability for SkillCapability {
    fn name(&self) -> &'static str {
        "skill"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::PreStep]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::PreStep {
                trigger,
                phase,
                messages,
                ..
            } = event
            else {
                return Ok(HookOutcome::Continue);
            };
            if *phase != PreStepPhase::Inject || trigger.is_some() {
                return Ok(HookOutcome::Continue);
            }
            let host = event.host();
            let catalog = self
                .state
                .lock()
                .expect("agent skill 目录锁被毒化")
                .prepare(host.cwd(), host.data_dir(), &self.roots);
            if let Some(message) = catalog {
                host.append(&[EventKind::UserMessage {
                    turn: event.turn(),
                    step: event.step(),
                    message,
                }])
                .await?;
            }

            // T006.11b：用户显式 /name（只扫 source=user 的 claimed 消息，排除注入上下文）。
            let mut injected: Vec<String> = Vec::new();
            for message in messages.iter() {
                if !matches!(message.source, MessageSource::User { .. }) {
                    continue;
                }
                for block in &message.content {
                    let ContentBlock::Text { text } = block else {
                        continue;
                    };
                    for name in invoked_skill_names(text) {
                        if injected.iter().any(|existing| existing == &name) {
                            continue;
                        }
                        let Some(skill) = load_user_invocable_skill(
                            host.cwd(),
                            host.data_dir(),
                            &self.roots,
                            &name,
                        ) else {
                            continue;
                        };
                        injected.push(name.clone());
                        let message = Message::skill_invocation(render_skill_content(&skill), name);
                        host.append(&[EventKind::UserMessage {
                            turn: event.turn(),
                            step: event.step(),
                            message,
                        }])
                        .await?;
                    }
                }
            }
            Ok(HookOutcome::Continue)
        })
    }
}
