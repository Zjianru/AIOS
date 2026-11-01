//! T006.5a：中断修复的原因和时间照 DSH 的 `packages/core/session/src/repair.ts:88-96`。
//!
//! 收尾事件顺序 tool/result → step/end → turn/end；turn/end 原因是 interrupted；
//! 合成事件复用最后一个真实事件的时间；错误身份是 {name, code}。

use model::{ContentBlock, Message, Role};

use crate::{Event, EventKind, SessionStore, ToolError, TurnEndReason, interrupted_turn_closers};

fn at(seq: u64, time: u64, kind: EventKind) -> Event {
    Event { seq, time, kind }
}

fn assistant_tool_call(time: u64) -> Event {
    at(
        4,
        time,
        EventKind::AssistantMessage {
            turn: 1,
            step: 1,
            message: Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolCall {
                    id: "call_0".to_owned(),
                    name: "read".to_owned(),
                    arguments: "{}".to_owned(),
                }],
            ),
            usage: None,
            interrupted: None,
        },
    )
}

/// tool/call 已落盘、结果没落盘（结果未知）的日志前缀。
fn started_call_events() -> Vec<Event> {
    vec![
        at(1, 1_000, EventKind::TurnStart { turn: 1 }),
        at(2, 1_100, EventKind::StepStart { turn: 1, step: 1 }),
        at(
            3,
            1_200,
            EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: Message::text(Role::User, "跑工具"),
            },
        ),
        assistant_tool_call(1_300),
        at(
            5,
            1_400,
            EventKind::ToolCall {
                turn: 1,
                step: 1,
                call_id: "call_0".to_owned(),
                name: "read".to_owned(),
                arguments: "{}".to_owned(),
            },
        ),
    ]
}

/// 只有 assistant 的调用块、tool/call 还没落盘的日志前缀。
fn not_started_call_events() -> Vec<Event> {
    vec![
        at(1, 1_000, EventKind::TurnStart { turn: 1 }),
        at(2, 1_100, EventKind::StepStart { turn: 1, step: 1 }),
        assistant_tool_call(1_300),
    ]
}

fn create_store() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    (data_dir, cwd, store)
}

#[test]
fn closers_use_interrupted_reason_and_reuse_last_event_time() {
    let events = started_call_events();
    let last = events.last().expect("日志非空");
    let closers = interrupted_turn_closers(&events).expect("开着的 turn 要补收尾");

    // DSH repair.ts:88-96：time 复用最后一个真实事件的时间。
    assert_eq!(
        closers.time, last.time,
        "修复事件复用最后一个真实事件的时间"
    );
    // 顺序：tool/result → step/end → turn/end；原因是 interrupted（DSH repair.ts:96）。
    assert_eq!(closers.events.len(), 3);
    assert_eq!(
        closers.events[2],
        EventKind::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Interrupted,
        }
    );
    assert_eq!(closers.events[1], EventKind::StepEnd { turn: 1, step: 1 });
    assert!(matches!(
        closers.events[0],
        EventKind::ToolResult {
            turn: 1,
            step: 1,
            ..
        }
    ));
}

#[test]
fn appended_closers_keep_last_event_time_and_contiguous_seq() {
    let (data_dir, cwd, mut store) = create_store();
    let _ = (data_dir, cwd);
    store
        .append(&[
            EventKind::TurnStart { turn: 1 },
            EventKind::StepStart { turn: 1, step: 1 },
        ])
        .unwrap();
    let last = store.events().last().expect("日志非空").clone();
    let closers = interrupted_turn_closers(store.events()).expect("开着的 turn 要补收尾");
    store
        .append_with_time(&closers.events, closers.time)
        .unwrap();

    let appended = &store.events()[2..];
    assert_eq!(appended.len(), closers.events.len());
    for (offset, event) in appended.iter().enumerate() {
        assert_eq!(
            event.time, last.time,
            "修复事件沿用前一个事件的时间（DSH repair.ts:88-90）"
        );
        assert_eq!(
            event.seq,
            last.seq + offset as u64 + 1,
            "seq 仍由 store 连续分配"
        );
    }
    assert_eq!(
        appended.last().map(|event| &event.kind),
        Some(&EventKind::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Interrupted,
        })
    );
}

#[test]
fn outcome_unknown_closer_carries_dsh_identity() {
    let closers = interrupted_turn_closers(&started_call_events()).expect("要补收尾");
    let EventKind::ToolResult { message, error, .. } = &closers.events[0] else {
        panic!("第一条应是 tool/result");
    };
    // DSH repair.ts:188-189 的错误身份。
    assert_eq!(
        error,
        &Some(ToolError {
            name: "ToolOutcomeUnknownError".to_owned(),
            code: "TOOL_OUTCOME_UNKNOWN".to_owned(),
        })
    );
    let ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error,
    } = &message.content[0]
    else {
        panic!("tool/result 消息里应是 ToolResult 块");
    };
    assert_eq!(tool_use_id, "call_0");
    assert!(*is_error);
    // 模型可见文案逐字照 DSH repair.ts:34。
    assert_eq!(
        content,
        "The tool call was interrupted after it was recorded, but no result was durably recorded. Its outcome is unknown. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly."
    );
}

#[test]
fn not_started_closer_carries_dsh_identity() {
    let closers = interrupted_turn_closers(&not_started_call_events()).expect("要补收尾");
    let EventKind::ToolResult { message, error, .. } = &closers.events[0] else {
        panic!("第一条应是 tool/result");
    };
    assert_eq!(
        error,
        &Some(ToolError {
            name: "ToolNotStartedError".to_owned(),
            code: "TOOL_NOT_STARTED".to_owned(),
        })
    );
    let ContentBlock::ToolResult { content, .. } = &message.content[0] else {
        panic!("tool/result 消息里应是 ToolResult 块");
    };
    assert_eq!(
        content,
        "The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."
    );
}
