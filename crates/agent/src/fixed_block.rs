//! T077.2：页面清单这类固定块。已有会话没有当前版本时，在下一次请求前补一条程序消息；
//! 快照身份由正文哈希决定，正文一变化，旧会话下一次请求就能看到新增内容。
//!
//! T077.6：固定块还负责每轮真实用户消息后的「现在」。它在用户话之后、调模型之前追加，
//! 用这一刻的本机时间；不单独触发一轮，也不画用户气泡。

use std::sync::{Arc, Mutex};

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};
use model::{Message, MessageSource};
use session::EventKind;

const RPC_PREFIX: &str = "fixed-block:";
const NOW_PREFIX: &str = "date-now:";

fn digest(text: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

fn timezone_name() -> String {
    if let Ok(tz) = std::env::var("TZ")
        && !tz.trim().is_empty()
    {
        return tz;
    }
    if let Ok(link) = std::fs::read_link("/etc/localtime")
        && let Some(name) = link
            .to_str()
            .and_then(|path| path.split("zoneinfo/").nth(1))
            .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }
    chrono::Local::now().offset().to_string()
}

fn now_line() -> String {
    format!(
        "现在：{}（{}）",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        timezone_name()
    )
}

fn now_millis() -> i64 {
    chrono::Local::now().timestamp_millis()
}

fn real_user_rpc(rpc_id: Option<&str>) -> bool {
    match rpc_id {
        None => true,
        Some(id) => !id.contains(':'),
    }
}

pub(crate) struct FixedBlockState {
    text: String,
    identity: String,
    now_message: bool,
    last_seen: Arc<Mutex<Option<String>>>,
}

impl FixedBlockState {
    pub(crate) fn load(store: &session::SessionStore, text: String, now_message: bool) -> Self {
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
            now_message,
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

    /// 本轮是否已经有真实用户消息、以及是否已经补过「现在」；只看这一轮。
    fn turn_facts(&self, host: &dyn crate::capabilities::LoopAccess, turn: u64) -> (bool, bool) {
        let Ok(store) = host.session().lock() else {
            return (false, false);
        };
        let mut real_user = false;
        let mut dated = false;
        for event in store.events() {
            let EventKind::UserMessage {
                turn: event_turn,
                message,
                ..
            } = &event.kind
            else {
                continue;
            };
            if *event_turn != turn {
                continue;
            }
            let MessageSource::User { rpc_id } = &message.source else {
                continue;
            };
            if rpc_id
                .as_deref()
                .is_some_and(|id| id.starts_with(NOW_PREFIX))
            {
                dated = true;
            } else if real_user_rpc(rpc_id.as_deref()) {
                real_user = true;
            }
        }
        (real_user, dated)
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
            let HookEvent::PreStep {
                turn,
                step,
                host,
                phase,
                ..
            } = event
            else {
                return Ok(HookOutcome::Continue);
            };
            match phase {
                PreStepPhase::FixedBlocks => self.append_fixed_block(*turn, *step, *host).await,
                PreStepPhase::Inject => {
                    if !self.state.now_message || *step != 1 {
                        return Ok(HookOutcome::Continue);
                    }
                    let (real_user, dated) = self.turn_facts(*host, *turn);
                    if !real_user || dated {
                        return Ok(HookOutcome::Continue);
                    }
                    let at = now_millis();
                    let message = Message::user_text_with_request_id(
                        now_line(),
                        format!("{NOW_PREFIX}{turn}:{at}"),
                    );
                    host.append(&[EventKind::UserMessage {
                        turn: *turn,
                        step: *step,
                        message,
                    }])
                    .await?;
                    Ok(HookOutcome::Continue)
                }
                PreStepPhase::Compact => Ok(HookOutcome::Continue),
            }
        })
    }
}

impl FixedBlockCapability {
    async fn append_fixed_block(
        &self,
        turn: u64,
        step: u64,
        host: &dyn crate::capabilities::LoopAccess,
    ) -> Result<HookOutcome, crate::error::AgentError> {
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
                "AIOS 页面清单（固定块，内容有变化时只补这一条；不是新任务）：
{}",
                self.state.text
            ),
            RPC_PREFIX.to_owned() + &self.state.identity,
        );
        host.append(&[EventKind::UserMessage {
            turn,
            step,
            message,
        }])
        .await?;
        *self
            .state
            .last_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(self.state.identity.clone());
        Ok(HookOutcome::Continue)
    }
}
