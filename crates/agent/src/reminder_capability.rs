//! 重复调用提醒能力（T006.1d）：挂 ToolAfter，文案与 T005.9 完全一致。

use model::Message;
use session::EventKind;

use crate::capabilities::{Capability, HookEvent, HookFuture, HookOutcome, HookPoint};

pub(crate) struct ReminderCapability {
    chain: std::sync::Arc<std::sync::Mutex<Option<(String, usize)>>>,
}

impl ReminderCapability {
    pub(crate) fn new(chain: std::sync::Arc<std::sync::Mutex<Option<(String, usize)>>>) -> Self {
        Self { chain }
    }
}

const THRESHOLDS: [usize; 3] = [3, 5, 8];

fn truncate_preview(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    format!("{}…", text.chars().take(max_chars).collect::<String>())
}

impl ReminderCapability {
    fn repeat_reminder(
        chain: &std::sync::Mutex<Option<(String, usize)>>,
        calls: &[tools::ToolCall],
    ) -> Option<String> {
        let mut reminder: Option<String> = None;
        for call in calls {
            if call.name == "todo_write" {
                continue;
            }
            let key = format!(
                "{}\0{}",
                call.name,
                tools::canonical_arguments(&call.arguments)
            );
            let count = {
                let mut chain = chain.lock().expect("agent 重复链锁被毒化");
                let count = match chain.as_ref() {
                    Some((previous, count)) if *previous == key => count + 1,
                    _ => 1,
                };
                *chain = Some((key, count));
                count
            };
            if THRESHOLDS.contains(&count) && reminder.is_none() {
                reminder = Some(if count == THRESHOLDS[0] {
                    "You are repeating the exact same tool call with identical arguments. Carefully analyze the previous result before calling again: if the task is not complete, try a different approach or different arguments instead of repeating the call.".to_owned()
                } else {
                    format!(
                        "Repeated tool call detected:\n- tool: {}\n- consecutive_calls: {count}\n- arguments: {}\nThe repeated calls are not making progress. Do not call this tool with these exact arguments again. Inspect the latest result and choose a different action, different arguments, or finish the task if enough evidence has been gathered.",
                        call.name,
                        truncate_preview(&tools::canonical_arguments(&call.arguments), 500)
                    )
                });
            }
        }
        reminder
    }
}

impl Capability for ReminderCapability {
    fn name(&self) -> &'static str {
        "reminder"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::ToolAfter]
    }

    fn reset_repeat_chain(&self) {
        *self.chain.lock().expect("agent 重复链锁被毒化") = None;
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::ToolAfter {
                turn,
                step,
                calls,
                cancelled,
                ..
            } = event
            else {
                return Ok(HookOutcome::Continue);
            };
            if *cancelled {
                return Ok(HookOutcome::Continue);
            }
            let host = event.host();
            if let Some(reminder) = Self::repeat_reminder(&self.chain, calls) {
                host.append(&[EventKind::UserMessage {
                    turn: *turn,
                    step: *step,
                    message: Message::user_text(reminder),
                }])
                .await?;
            }
            Ok(HookOutcome::Continue)
        })
    }
}
