//! 续接时的中断修复：给最后一个没收尾的 turn 补收尾事件。
//!
//! 机制照 DSH 的 packages/core/session/src/repair.ts（strategy 第 1 节）：
//! 扫描日志边界，给 assistant 已请求、但结果没有落盘的工具调用补保守的错误结果
//! （结果未知 / 还没开始），再补 step/end 和 turn/end。只读日志，不重跑工具。

use model::{Message, MessageId};

use crate::{Event, EventKind, ToolError, TurnEndReason};

/// 已记录开始、但结果没有落盘的调用：结果未知，重试前先核实外部状态。
const TOOL_OUTCOME_UNKNOWN: &str = "TOOL_OUTCOME_UNKNOWN";
/// assistant 请求过、但 Harness 还没记录开始的调用：需要时可以重试。
const TOOL_NOT_STARTED: &str = "TOOL_NOT_STARTED";

/// 错误身份名照 DSH repair.ts:187-190。
const OUTCOME_UNKNOWN_ERROR: &str = "ToolOutcomeUnknownError";
const NOT_STARTED_ERROR: &str = "ToolNotStartedError";

/// 模型可见文案逐字照抄 DSH 的 repair.ts:32-41；不改写、不翻译。
const OUTCOME_UNKNOWN_TEXT: &str = "The tool call was interrupted after it was recorded, but no result was durably recorded. Its outcome is unknown. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly.";
const NOT_STARTED_TEXT: &str = "The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed.";

/// 一个 assistant 请求过、但日志里还没有结果的调用。
struct PendingCall {
    call_id: String,
    turn: u64,
    step: u64,
    /// 日志里出现过对应的 tool/call（Harness 记录过"开始"）。
    started: bool,
}

/// 修复要给开着的 turn 补的一批收尾事件，以及这批事件共用的时间。
///
/// time 复用最后一个真实事件的时间（照 DSH repair.ts:88-96：确定性，不发明未来时间）。
#[derive(Debug, Clone, PartialEq)]
pub struct TurnClosers {
    pub events: Vec<EventKind>,
    pub time: u64,
}

/// 崩溃/被杀之后续接时，给最后一个没收尾的 turn 造收尾事件（照 DSH 的
/// interruptedTurnClosers）：按 assistant 顺序补 tool/result，再补 step/end
/// （如果开着）和 turn/end（Interrupted）。平衡日志或空日志返回 None。
///
/// 事件本身不带 seq：由 SessionStore 作为一批分配，保持 seq 连续、一次 fsync、
/// 失败回滚（spec 的存储决定）。
pub fn interrupted_turn_closers(events: &[Event]) -> Option<TurnClosers> {
    let mut open_turn: Option<u64> = None;
    let mut open_step: Option<u64> = None;
    let mut pending: Vec<PendingCall> = Vec::new();
    for event in events {
        match &event.kind {
            EventKind::TurnStart { turn } => {
                open_turn = Some(*turn);
                open_step = None;
                pending.clear();
            }
            EventKind::TurnEnd { .. } => {
                open_turn = None;
                open_step = None;
                pending.clear();
            }
            EventKind::StepStart { step, .. } => open_step = Some(*step),
            EventKind::StepEnd { .. } => {
                open_step = None;
                pending.clear();
            }
            EventKind::QuestionAsked { .. }
            | EventKind::QuestionAnswered { .. }
            | EventKind::QuestionCancelled { .. } => {}
            EventKind::JobRegistered { .. } | EventKind::JobSettled { .. } => {}
            EventKind::SessionModel { .. } => {}
            // 加载记录只记工具表的变化，不参与收尾。
            EventKind::ToolRegistry { .. } => {}
            EventKind::AssistantMessage {
                turn,
                step,
                message,
                ..
            } => {
                for block in &message.content {
                    if let Some((call_id, _, _)) = block.as_tool_call() {
                        pending.push(PendingCall {
                            call_id: call_id.to_owned(),
                            turn: *turn,
                            step: *step,
                            started: false,
                        });
                    }
                }
            }
            EventKind::ToolCall { call_id, .. } => {
                if let Some(call) = pending.iter_mut().find(|call| call.call_id == *call_id) {
                    call.started = true;
                }
            }
            EventKind::ToolResult {
                turn,
                step,
                message,
                ..
            } => {
                if let Some(call_id) = message.tool_call_id.as_deref()
                    && let Some(position) = pending.iter().position(|call| {
                        call.call_id == call_id && call.turn == *turn && call.step == *step
                    })
                {
                    pending.remove(position);
                }
            }
            EventKind::SystemMessage { .. }
            | EventKind::UserMessage { .. }
            | EventKind::AssistantAttempt { .. }
            | EventKind::LlmRetry { .. }
            | EventKind::LlmRetryStarted { .. }
            | EventKind::CompactionPrune { .. }
            | EventKind::TodoWrite { .. }
            | EventKind::CompactionStart { .. }
            | EventKind::CompactionSummary { .. }
            | EventKind::CompactionEnd { .. }
            | EventKind::InboxSpliced { .. } => {}
        }
    }
    let turn = open_turn?;
    // 最后一个真实事件给合成事件提供时间基准（DSH repair.ts:88-96：确定性，不发明未来时间）。
    let time = events.last().map_or(0, |event| event.time);
    let mut closers = Vec::with_capacity(pending.len() + 2);
    for call in pending {
        let (name, code, text) = if call.started {
            (
                OUTCOME_UNKNOWN_ERROR,
                TOOL_OUTCOME_UNKNOWN,
                OUTCOME_UNKNOWN_TEXT,
            )
        } else {
            (NOT_STARTED_ERROR, TOOL_NOT_STARTED, NOT_STARTED_TEXT)
        };
        // 确定性 message id：AIOS 的修复在分配 seq 前造事件，用 cause+callId 代替 DSH 的
        // `${cause}-tool-result-${callId}-${seq}`（handback known）。
        let message_id = MessageId::parse(format!("interrupted-tool-result-{}", call.call_id));
        closers.push(EventKind::ToolResult {
            turn: call.turn,
            step: call.step,
            message: Message::tool_result_with_id(message_id, call.call_id, text, true),
            error: Some(ToolError {
                name: name.to_owned(),
                code: code.to_owned(),
            }),
            meta: None,
        });
    }
    if let Some(step) = open_step {
        closers.push(EventKind::StepEnd { turn, step });
    }
    closers.push(EventKind::TurnEnd {
        turn,
        reason: TurnEndReason::Interrupted,
    });
    Some(TurnClosers {
        events: closers,
        time,
    })
}
