//! T006.5a：事件记录照 DSH 的 SessionEvent：`{type, seq, time, data}`，data 放该事件字段。
//!
//! DSH 依据：`packages/core/session/src/index.ts:826-836`（事件的 type/seq/time/data）、
//! `packages/session/session-format-v1-to-v2/src/codec.ts:21`（物理行的必填键）、
//! `packages/core/session/src/types.ts:287-436`（逐事件的 data 形状）。

use std::fs;

use model::{AssistantStreamRecord, ContentBlock, Message, Role, StreamChunk, Usage};
use serde_json::{Value, json};

use crate::{
    AbortCause, EventKind, InboxOutcome, InboxTarget, LlmFailure, RetryId, RetryMode, SessionStore,
    TodoItem, TodoStatus, ToolError, TurnEndReason,
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
    // DSH 的 tool/result.error：{name, code}（packages/core/session/src/types.ts:383-393）；
    // 消息本身照 DSH ToolResultMessage（id/source/toolCallId/isError 在消息级）。
    let message = &rows[0]["data"]["message"];
    assert_eq!(
        message["source"],
        json!({"kind": "tool", "callId": "call_0"}),
        "tool 消息 source 带 callId"
    );
    assert_eq!(message["toolCallId"], "call_0");
    assert_eq!(message["isError"], true);
    assert_eq!(
        message["content"],
        json!([{"type": "text", "text": "Error: tool failed"}]),
        "消息 content 是结果内容块"
    );
    assert!(
        message["id"]
            .as_str()
            .is_some_and(|id| id.split('-').count() == 5),
        "tool 消息带 uuid 形状的 id：{message}"
    );
    assert_eq!(
        rows[0]["data"]["error"],
        json!({"name": "ToolOutcomeUnknownError", "code": "TOOL_OUTCOME_UNKNOWN"})
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

/// DSH 的 assistant 结算事件（types.ts:349-363）：message 带必填 stream；usage 是
/// TokenUsage camelCase；interrupted 只在取消收尾时写 true；assistant/attempt 只带流。
#[test]
fn assistant_settlement_shapes_match_dsh() {
    let (_data_dir, _cwd, mut store) = create_store();
    store
        .append(&[
            EventKind::AssistantMessage {
                turn: 1,
                step: 1,
                message: Message::assistant(
                    vec![ContentBlock::text("回答")],
                    "deepseek",
                    "deepseek-flash",
                ),
                // 合成事件：即使 stream 为空也要序列化成数组（DSH 的 data 必填 stream）。
                stream: Vec::new(),
                usage: Some(Usage {
                    input_tokens: Some(7),
                    output_tokens: Some(2),
                    ..Usage::default()
                }),
                interrupted: None,
            },
            EventKind::AssistantAttempt {
                turn: 1,
                step: 1,
                stream: vec![AssistantStreamRecord::Chunk {
                    time: 90,
                    chunk: StreamChunk::BlockStart {
                        index: 0,
                        block_type: model::BlockType::Text,
                    },
                }],
            },
            EventKind::AssistantMessage {
                turn: 1,
                step: 1,
                message: Message::assistant(
                    vec![ContentBlock::text("半截")],
                    "deepseek",
                    "deepseek-flash",
                ),
                stream: Vec::new(),
                usage: None,
                interrupted: Some(true),
            },
        ])
        .unwrap();

    let (_header, rows) = log_rows(&store);
    let message = &rows[0]["data"];
    assert_eq!(rows[0]["type"], "assistant/message");
    assert_eq!(message["turn"], 1);
    assert_eq!(message["step"], 1);
    assert_eq!(message["stream"], json!([]), "stream 是必填数组，空也要写");
    assert_eq!(
        message["usage"],
        json!({"inputTokens": 7, "outputTokens": 2}),
        "usage 字段照 DSH TokenUsage"
    );
    assert!(
        message.get("interrupted").is_none(),
        "正常完成省略 interrupted"
    );
    assert_eq!(
        message["message"]["content"],
        json!([{"type": "text", "text": "回答"}])
    );

    let attempt = &rows[1]["data"];
    assert_eq!(rows[1]["type"], "assistant/attempt");
    assert_eq!(
        attempt,
        &json!({
            "turn": 1,
            "step": 1,
            "stream": [{
                "type": "chunk",
                "time": 90,
                "chunk": {"type": "block-start", "index": 0, "blockType": "text"}
            }]
        }),
        "assistant/attempt 只带 turn/step/stream"
    );

    let interrupted = &rows[2]["data"];
    assert_eq!(
        interrupted["interrupted"], true,
        "取消收尾写 interrupted=true"
    );
    assert_eq!(
        interrupted["message"]["content"],
        json!([{"type": "text", "text": "半截"}])
    );
}
/// DSH 的 llm/retry 与 llm/retry-started（llm-retry/src/types.ts:9-40）：前者在等待前落盘、
/// 带 retryId/turn/step/provider/mode/policyKey/retry/maxRetries/delayMs/failure；后者只带
/// retryId/turn/step/retry。两者都是 log-only（派生历史只认消息事件）。
#[test]
fn retry_events_match_dsh() {
    let (_data_dir, _cwd, mut store) = create_store();
    store
        .append(&[
            EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: Message::text(Role::User, "重试"),
            },
            EventKind::LlmRetry {
                retry_id: RetryId::from("retry-1"),
                turn: 1,
                step: 1,
                provider: "deepseek".to_owned(),
                mode: RetryMode::Normal,
                policy_key: "[\"normal\"]".to_owned(),
                retry: 1,
                max_retries: Some(5),
                delay_ms: 500,
                failure: LlmFailure {
                    message: "boom".to_owned(),
                    code: "SERVER".to_owned(),
                },
            },
            EventKind::LlmRetryStarted {
                retry_id: RetryId::from("retry-1"),
                turn: 1,
                step: 1,
                retry: 1,
            },
        ])
        .unwrap();

    let (_header, rows) = log_rows(&store);
    assert_eq!(rows[1]["type"], "llm/retry");
    assert_eq!(
        rows[1]["data"],
        json!({
            "retryId": "retry-1",
            "turn": 1,
            "step": 1,
            "provider": "deepseek",
            "mode": "normal",
            "policyKey": "[\"normal\"]",
            "retry": 1,
            "maxRetries": 5,
            "delayMs": 500,
            "failure": {"message": "boom", "code": "SERVER"},
        }),
        "llm/retry 字段照 DSH"
    );
    assert_eq!(rows[2]["type"], "llm/retry-started");
    assert_eq!(
        rows[2]["data"],
        json!({"retryId": "retry-1", "turn": 1, "step": 1, "retry": 1}),
        "llm/retry-started 字段照 DSH"
    );

    // log-only：派生历史只看到 user 消息，不把重试记录当消息。
    let derived = crate::derive::derive_messages(store.events());
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0].role, Role::User);
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
        EventKind::InboxSpliced {
            target: InboxTarget::NextTurn,
            start: 0,
            removed_count: None,
            inserted: vec![Message::text(Role::User, "排队")],
            outcome: Some(InboxOutcome::Canceled),
        },
        EventKind::AssistantMessage {
            turn: 1,
            step: 1,
            message: Message::text(Role::Assistant, "好的"),
            stream: vec![AssistantStreamRecord::TextChunks {
                time0: 100,
                index: 0,
                dt: Vec::new(),
                texts: vec!["好的".to_owned()],
            }],
            usage: Some(Usage {
                input_tokens: Some(3),
                output_tokens: Some(2),
                ..Usage::default()
            }),
            interrupted: Some(true),
        },
        EventKind::AssistantAttempt {
            turn: 1,
            step: 1,
            stream: vec![AssistantStreamRecord::Chunk {
                time: 90,
                chunk: StreamChunk::BlockStart {
                    index: 0,
                    block_type: model::BlockType::Text,
                },
            }],
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
        EventKind::LlmRetry {
            retry_id: RetryId::from("retry-1"),
            turn: 1,
            step: 1,
            provider: "deepseek".to_owned(),
            mode: RetryMode::Normal,
            policy_key: "policy".to_owned(),
            retry: 1,
            max_retries: Some(5),
            delay_ms: 500,
            failure: LlmFailure {
                message: "boom".to_owned(),
                code: "SERVER".to_owned(),
            },
        },
        EventKind::LlmRetryStarted {
            retry_id: RetryId::from("retry-1"),
            turn: 1,
            step: 1,
            retry: 1,
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
    // 消息 id/source 跟着事件过 JSON：派生/续接看到的是同一个 id。
    let user = &rows[3]["data"]["message"];
    assert_eq!(user["role"], "user");
    assert_eq!(user["source"], json!({"kind": "user"}));
    assert!(
        user["id"]
            .as_str()
            .is_some_and(|id| id.split('-').collect::<Vec<_>>().len() == 5),
        "user/message 的 JSON 带 uuid 形状 id：{user}"
    );
    let assistant = &rows[7]["data"]["message"];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["source"]["kind"], "model");
    drop(store);

    let reopened = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(reopened.events().len(), kinds.len());
    for (event, kind) in reopened.events().iter().zip(&kinds) {
        assert_eq!(&event.kind, kind, "信封改形后事件仍能原样读回");
    }
}
