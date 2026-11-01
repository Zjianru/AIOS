//! T006.5a：事件记录照 DSH 的 SessionEvent：`{type, seq, time, data}`，data 放该事件字段。
//!
//! DSH 依据：`packages/core/session/src/index.ts:826-836`（事件的 type/seq/time/data）、
//! `packages/session/session-format-v1-to-v2/src/codec.ts:21`（物理行的必填键）、
//! `packages/core/session/src/types.ts:287-436`（逐事件的 data 形状）。

use std::fs;

use model::{Message, Role, Usage};
use serde_json::{Value, json};

use crate::{
    AbortCause, EventKind, LlmFailure, SessionStore, TodoItem, TodoStatus, ToolError, TurnEndReason,
};

fn create_store() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    (data_dir, cwd, store)
}

/// 日志里的 header 和每个事件行，解析成 JSON。
fn log_rows(store: &SessionStore) -> (Value, Vec<Value>) {
    let text = fs::read_to_string(store.path()).unwrap();
    let mut lines = text.lines();
    let header: Value = serde_json::from_str(lines.next().expect("日志有 header 行")).unwrap();
    let rows = lines
        .map(|line| serde_json::from_str(line).expect("事件行是 JSON"))
        .collect();
    (header, rows)
}

fn assert_envelope(row: &Value) {
    let mut keys: Vec<&str> = row
        .as_object()
        .expect("事件行是 JSON 对象")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["data", "seq", "time", "type"],
        "事件行的字段集合照 DSH 的 SessionEvent：{row}"
    );
    assert!(row["data"].is_object(), "事件字段都放在 data 里：{row}");
}

#[test]
fn event_rows_use_dsh_envelope() {
    let (_data_dir, _cwd, mut store) = create_store();
    store
        .append(&[
            EventKind::TurnStart { turn: 1 },
            EventKind::ToolCall {
                turn: 1,
                step: 1,
                call_id: "call_0".to_owned(),
                name: "read".to_owned(),
                arguments: "{}".to_owned(),
            },
        ])
        .unwrap();

    let (header, rows) = log_rows(&store);
    assert_eq!(header["type"], "session");
    assert_eq!(rows.len(), 2);
    assert_envelope(&rows[0]);
    assert_envelope(&rows[1]);

    assert_eq!(rows[0]["type"], "turn/start");
    assert_eq!(rows[0]["seq"], 1);
    assert!(rows[0]["time"].as_u64().is_some());
    assert_eq!(rows[0]["data"], json!({"turn": 1}));

    assert_eq!(rows[1]["type"], "tool/call");
    assert_eq!(
        rows[1]["data"],
        json!({
            "turn": 1,
            "step": 1,
            "callId": "call_0",
            "name": "read",
            "arguments": "{}",
        })
    );
}

#[test]
fn turn_end_reasons_use_dsh_shapes() {
    let (_data_dir, _cwd, mut store) = create_store();
    store
        .append(&[
            EventKind::TurnEnd {
                turn: 1,
                reason: TurnEndReason::Completed,
            },
            EventKind::TurnEnd {
                turn: 2,
                reason: TurnEndReason::MaxTokens,
            },
            EventKind::TurnEnd {
                turn: 3,
                reason: TurnEndReason::Aborted {
                    reason: AbortCause::User,
                },
            },
            EventKind::TurnEnd {
                turn: 4,
                reason: TurnEndReason::Interrupted,
            },
            EventKind::TurnEnd {
                turn: 5,
                reason: TurnEndReason::Error {
                    error: LlmFailure {
                        message: "boom".to_owned(),
                        code: "SERVER".to_owned(),
                    },
                },
            },
        ])
        .unwrap();

    let (_header, rows) = log_rows(&store);
    // DSH 的 TurnEndReasonMap（packages/core/session/src/types.ts:206-235）与
    // AgentCancelCause（types.ts:194-201）。
    assert_eq!(
        rows[0]["data"],
        json!({"turn": 1, "reason": {"kind": "completed"}})
    );
    assert_eq!(
        rows[1]["data"],
        json!({"turn": 2, "reason": {"kind": "max-tokens"}})
    );
    assert_eq!(
        rows[2]["data"],
        json!({"turn": 3, "reason": {"kind": "aborted", "reason": {"kind": "user"}}})
    );
    assert_eq!(
        rows[3]["data"],
        json!({"turn": 4, "reason": {"kind": "interrupted"}})
    );
    assert_eq!(
        rows[4]["data"],
        json!({"turn": 5, "reason": {"kind": "error", "error": {"message": "boom", "code": "SERVER"}}})
    );
}

#[test]
fn tool_result_error_uses_dsh_identity() {
    let (_data_dir, _cwd, mut store) = create_store();
    store
        .append(&[EventKind::ToolResult {
            turn: 1,
            step: 1,
            message: Message::tool_result("call_0", "Error: tool failed", true),
            error: Some(ToolError {
                name: "ToolOutcomeUnknownError".to_owned(),
                code: "TOOL_OUTCOME_UNKNOWN".to_owned(),
            }),
        }])
        .unwrap();

    let (_header, rows) = log_rows(&store);
    // DSH 的 tool/result.error：{name, code}（packages/core/session/src/types.ts:383-393）。
    assert_eq!(
        rows[0]["data"],
        json!({
            "turn": 1,
            "step": 1,
            "message": {
                "role": "tool",
                "content": [{"type": "tool_result", "tool_use_id": "call_0", "content": "Error: tool failed", "is_error": true}],
            },
            "error": {"name": "ToolOutcomeUnknownError", "code": "TOOL_OUTCOME_UNKNOWN"},
        })
    );
    assert!(
        rows[0]["data"].get("error_code").is_none(),
        "错误身份不再用扁平的 error_code 字段：{row}",
        row = rows[0]["data"]
    );
}

#[test]
fn todo_write_matches_dsh_payload() {
    let (_data_dir, _cwd, mut store) = create_store();
    store
        .append(&[EventKind::TodoWrite {
            todos: vec![
                TodoItem {
                    content: "写测试".to_owned(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    content: "做实现".to_owned(),
                    status: TodoStatus::Pending,
                },
            ],
        }])
        .unwrap();

    let (_header, rows) = log_rows(&store);
    // DSH 的 todo/write 只有 {todos}（packages/todo/tool-todo/src/types.ts:33、index.ts:200）。
    assert_eq!(
        rows[0]["data"],
        json!({"todos": [
            {"content": "写测试", "status": "in_progress"},
            {"content": "做实现", "status": "pending"},
        ]})
    );
}

#[test]
fn all_event_kinds_round_trip_under_dsh_envelope() {
    let (data_dir, _cwd, mut store) = create_store();
    let id = store.id().clone();
    let kinds = vec![
        EventKind::TurnStart { turn: 1 },
        EventKind::StepStart { turn: 1, step: 1 },
        EventKind::SystemMessage {
            turn: 1,
            step: 1,
            message: Message::text(Role::System, "系统提示"),
        },
        EventKind::UserMessage {
            turn: 1,
            step: 1,
            message: Message::text(Role::User, "你好"),
        },
        EventKind::QuestionAsked {
            turn: 1,
            step: 1,
            question_id: "q1".to_owned(),
            question: "继续？".to_owned(),
            header: Some("确认".to_owned()),
            options: vec!["是".to_owned()],
            multi_select: false,
        },
        EventKind::QuestionCancelled {
            turn: 1,
            step: 1,
            question_id: "q1".to_owned(),
        },
        EventKind::AssistantMessage {
            turn: 1,
            step: 1,
            message: Message::text(Role::Assistant, "好的"),
            usage: Some(Usage {
                input_tokens: Some(3),
                output_tokens: Some(2),
                ..Usage::default()
            }),
            interrupted: Some(true),
        },
        EventKind::ToolCall {
            turn: 1,
            step: 1,
            call_id: "call_0".to_owned(),
            name: "read".to_owned(),
            arguments: "{}".to_owned(),
        },
        EventKind::ToolResult {
            turn: 1,
            step: 1,
            message: Message::tool_result("call_0", "内容", false),
            error: None,
        },
        EventKind::CompactionPrune {
            call_id: "call_0".to_owned(),
            content: "修剪后".to_owned(),
        },
        EventKind::TodoWrite {
            todos: vec![TodoItem {
                content: "收尾".to_owned(),
                status: TodoStatus::Completed,
            }],
        },
        EventKind::CompactionStart {
            compaction_id: "c1".to_owned(),
            turn: 1,
        },
        EventKind::CompactionSummary {
            compaction_id: "c1".to_owned(),
            summary: "摘要".to_owned(),
            shadowed_start: 1,
            shadowed_end: 2,
            shadowed_token_count: 3,
        },
        EventKind::CompactionEnd {
            compaction_id: "c1".to_owned(),
            turn: 1,
            error: None,
        },
        EventKind::StepEnd { turn: 1, step: 1 },
        EventKind::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Completed,
        },
    ];
    store.append(&kinds).unwrap();

    let (_header, rows) = log_rows(&store);
    assert_eq!(rows.len(), kinds.len());
    for row in &rows {
        assert_envelope(row);
    }
    drop(store);

    let reopened = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(reopened.events().len(), kinds.len());
    for (event, kind) in reopened.events().iter().zip(&kinds) {
        assert_eq!(&event.kind, kind, "信封改形后事件仍能原样读回");
    }
}
