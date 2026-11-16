//! 记忆注入能力：照指令注入，每步问 provider 要 (identity, text)；
//! 当前派生消息里看不到这个 identity 才追加一条带类型的记忆消息。

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};

pub(crate) struct MemoryCapability {
    provider: Option<crate::MemoryContextProvider>,
    identity: Arc<Mutex<Option<String>>>,
}

impl MemoryCapability {
    pub(crate) fn load(
        store: &session::SessionStore,
        provider: Option<crate::MemoryContextProvider>,
    ) -> (
        Arc<Mutex<Option<String>>>,
        Option<crate::MemoryContextProvider>,
    ) {
        let identity = store
            .derive_messages()
            .into_iter()
            .filter_map(|message| match message.source {
                model::MessageSource::MemoryContext { identity } => Some(identity),
                _ => None,
            })
            .last();
        (Arc::new(Mutex::new(identity)), provider)
    }

    pub(crate) fn new(
        state: (
            Arc<Mutex<Option<String>>>,
            Option<crate::MemoryContextProvider>,
        ),
    ) -> Self {
        Self {
            identity: state.0,
            provider: state.1,
        }
    }
}

impl Capability for MemoryCapability {
    fn name(&self) -> &'static str {
        "memory"
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
            let Some(provider) = &self.provider else {
                return Ok(HookOutcome::Continue);
            };
            let Some(context) = provider().map_err(crate::AgentError::MemoryProvider)? else {
                return Ok(HookOutcome::Continue);
            };
            let mut identity = self.identity.lock().await;
            if identity.as_deref() == Some(context.identity.as_str()) {
                return Ok(HookOutcome::Continue);
            }
            let host = event.host();
            let text = format!(
                "完整替换之前的记忆

{}",
                context.text
            );
            let message = model::Message::memory_context(text, context.identity.clone());
            host.append(&[session::EventKind::UserMessage {
                turn: event.turn(),
                step: event.step(),
                message,
            }])
            .await?;
            *identity = Some(context.identity);
            Ok(HookOutcome::Continue)
        })
    }
}
