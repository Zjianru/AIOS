//! todo 快照落日志能力（T006.1d）：挂 ToolAfter，成功的 todo_write 把整表快照落成 todo/write。

use session::EventKind;

use crate::capabilities::{Capability, HookEvent, HookFuture, HookOutcome, HookPoint};

pub(crate) struct TodoCapability;

impl Capability for TodoCapability {
    fn name(&self) -> &'static str {
        "todo"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::ToolAfter]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::ToolAfter {
                turn,
                step,
                calls,
                results,
                ..
            } = event
            else {
                return Ok(HookOutcome::Continue);
            };
            let succeeded = calls.iter().any(|call| {
                call.name == "todo_write"
                    && results
                        .iter()
                        .any(|result| result.call_id == call.id && !result.is_error)
            });
            if succeeded && let Some(todos) = event.host().tool_session().take_todos() {
                event
                    .host()
                    .append(&[EventKind::TodoWrite {
                        turn: *turn,
                        step: *step,
                        todos,
                    }])
                    .await?;
            }
            Ok(HookOutcome::Continue)
        })
    }
}
