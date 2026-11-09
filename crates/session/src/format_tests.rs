//! 事件记录照 DSH 的 SessionEvent：`{type, seq, time, data}`，data 放该事件字段。
//!
//! DSH 依据：`packages/core/session/src/index.ts:826-836`（事件的 type/seq/time/data）、
//! `packages/session/session-format-v1-to-v2/src/codec.ts:21`（物理行的必填键）、
//! `packages/core/session/src/types.ts:287-436`（逐事件的 data 形状）。

use std::fs;

use model::{AssistantStreamRecord, ContentBlock, Message, Role, StreamChunk, ToolSchema, Usage};
use serde_json::{Value, json};

use crate::{
    AbortCause, EventKind, InboxOutcome, InboxTarget, JobToolCallData, LlmFailure, RetryId,
    RetryMode, SessionStore, ShadowedRange, TodoItem, TodoStatus, ToolError, TurnEndReason,
};

/// session/model 事件的日志形状（type/data/effortWanted）与最近选择折叠。
#[test]
fn session_model_event_shape_and_last_selection() {
    let events = vec![
        crate::Event::new(
            1,
            1,
            EventKind::SessionModel {
                connection: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                effort_wanted: "high".to_owned(),
            },
        ),
        crate::Event::new(
            2,
            2,
            EventKind::SessionModel {
                connection: "minimax-cn".to_owned(),
                model: "MiniMax-M3.1-Flash".to_owned(),
                effort_wanted: "low".to_owned(),
            },
        ),
    ];
    let json = serde_json::to_value(&events[1]).unwrap();
    assert_eq!(json["type"], "session/model");
    assert_eq!(json["data"]["connection"], "minimax-cn");
    assert_eq!(json["data"]["effortWanted"], "low");
    let last = crate::last_model_selection(&events).expect("最近选择");
    assert_eq!(last.connection, "minimax-cn");
    assert_eq!(last.model, "MiniMax-M3.1-Flash");
    assert_eq!(last.effort_wanted, "low");
    // 日志行读回（反序列化）后折叠结果不变。
    let parsed: crate::Event = serde_json::from_value(json).unwrap();
    assert_eq!(crate::last_model_selection(&[parsed]).unwrap(), last);
}

/// 设置里屏蔽了正在跑的一轮时，turn/end 以 hook 原因收尾；日志往返后原因不丢。
#[test]
fn hook_abort_cause_roundtrips_through_the_log() {
    let event = crate::Event::new(
        1,
        1,
        EventKind::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Aborted {
                reason: AbortCause::Hook {
                    reason: "model-disabled".to_owned(),
                },
            },
        },
    );
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "turn/end");
    assert_eq!(
        json["data"]["reason"],
        json!({
            "kind": "aborted",
            "reason": {"kind": "hook", "reason": "model-disabled"},
        })
    );
    let parsed: crate::Event = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.kind, event.kind, "hook 原因往返序列化");
}

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
            meta: None,
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
                effort: None,
                effort_wanted: None,
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
                effort: None,
                effort_wanted: None,
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
/// question/answered 的 data 是一条 DSH 答案项（tool-ask-user 的
/// AskUserQuestionAnswerItem：id + selected[] + 可选 custom），落日志时再带问题定位。
#[test]
fn question_answered_matches_dsh_answer_item() {
    let (_data_dir, _cwd, mut store) = create_store();
    store
        .append(&[EventKind::QuestionAnswered {
            turn: 1,
            step: 2,
            call_id: "call_ask".to_owned(),
            question_id: "q2".to_owned(),
            selected: vec!["No".to_owned()],
            custom: Some("我自己的答案".to_owned()),
        }])
        .unwrap();
    let (_header, rows) = log_rows(&store);
    assert_eq!(rows[0]["type"], "question/answered");
    assert_eq!(
        rows[0]["data"],
        json!({
            "turn": 1,
            "step": 2,
            "call_id": "call_ask",
            "question_id": "q2",
            "selected": ["No"],
            "custom": "我自己的答案"
        }),
        "答案项字段照 DSH（selected 是数组、custom 可选）"
    );
}

/// 全部 EventKind 变体各造一份，信封回归与 wire 名一致性共用。
fn all_event_kinds() -> Vec<EventKind> {
    vec![
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
            call_id: "call_ask".to_owned(),
            question_id: "q1".to_owned(),
            question: "继续？".to_owned(),
            header: Some("确认".to_owned()),
            options: vec!["是".to_owned()],
            multi_select: false,
        },
        EventKind::QuestionAnswered {
            turn: 1,
            step: 1,
            call_id: "call_ask".to_owned(),
            question_id: "q1".to_owned(),
            selected: vec!["是".to_owned()],
            custom: None,
        },
        EventKind::QuestionCancelled {
            turn: 1,
            step: 1,
            call_id: "call_ask".to_owned(),
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
            effort: None,
            effort_wanted: None,
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
            meta: None,
        },
        EventKind::ToolRegistry {
            turn: 1,
            step: 1,
            additions: vec![ToolSchema {
                name: "read".to_owned(),
                description: "读文件".to_owned(),
                parameters: json!({"type": "object"}),
                defer_loading: false,
            }],
        },
        EventKind::CompactionPrune {
            shadowed_range: ShadowedRange { start: 2, end: 2 },
            shadowed_seqs: vec![2],
            shadowed_token_count: 5,
        },
        EventKind::TodoWrite {
            todos: vec![TodoItem {
                content: "收尾".to_owned(),
                status: TodoStatus::Completed,
            }],
        },
        EventKind::CompactionStart {
            compaction_id: "c1".to_owned(),
            source_command_id: None,
            turn: Some(1),
        },
        EventKind::CompactionSummary {
            compaction_id: "c1".to_owned(),
            source_command_id: None,
            summary: vec![ContentBlock::text("摘要")],
            raw_output: vec![ContentBlock::text("摘要")],
            llm_stream_call: true,
            shadowed_range: crate::ShadowedRange { start: 1, end: 2 },
            shadowed_seqs: vec![1, 2],
            shadowed_token_count: 3,
            provider: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: None,
            usage: None,
        },
        EventKind::CompactionEnd {
            compaction_id: "c1".to_owned(),
            source_command_id: None,
            turn: Some(1),
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
        EventKind::JobRegistered {
            job_id: "job-1".to_owned(),
            owner: None,
            kind: "bash".to_owned(),
            label: "后台任务".to_owned(),
            started_at: 100,
            tool_call: None,
        },
        EventKind::JobSettled {
            job_id: "job-1".to_owned(),
            owner: None,
            kind: "bash".to_owned(),
            label: "后台任务".to_owned(),
            started_at: 100,
            tool_call: None,
            finished_at: 200,
            status: "completed".to_owned(),
            cause: "exit".to_owned(),
            producer_reported: true,
            detail: None,
        },
        EventKind::SessionModel {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        EventKind::CardDecision {
            call_id: "call_card".to_owned(),
            option: Some("b".to_owned()),
            action: Some("some_action".to_owned()),
            phase: crate::CardDecisionPhase::Confirmed,
            data: Some(json!({"summary": "已执行"})),
        },
    ]
}

/// 编译期穷举：EventKind 新增变体而这里没写，本文件直接编不过。
///
/// 运行时只验证「清单长度/内容由人补」，所以再加一条 len 断言；两处一起漏才会失守。
fn every_variant_is_named(kind: &EventKind) {
    match kind {
        EventKind::TurnStart { .. }
        | EventKind::TurnEnd { .. }
        | EventKind::StepStart { .. }
        | EventKind::StepEnd { .. }
        | EventKind::SystemMessage { .. }
        | EventKind::UserMessage { .. }
        | EventKind::QuestionAsked { .. }
        | EventKind::QuestionAnswered { .. }
        | EventKind::QuestionCancelled { .. }
        | EventKind::InboxSpliced { .. }
        | EventKind::JobRegistered { .. }
        | EventKind::JobSettled { .. }
        | EventKind::AssistantMessage { .. }
        | EventKind::AssistantAttempt { .. }
        | EventKind::LlmRetry { .. }
        | EventKind::LlmRetryStarted { .. }
        | EventKind::ToolCall { .. }
        | EventKind::ToolResult { .. }
        | EventKind::ToolRegistry { .. }
        | EventKind::CompactionPrune { .. }
        | EventKind::TodoWrite { .. }
        | EventKind::CompactionStart { .. }
        | EventKind::CompactionSummary { .. }
        | EventKind::CompactionEnd { .. }
        | EventKind::SessionModel { .. }
        | EventKind::CardDecision { .. } => {}
    }
}

/// 防未来漂移：EventKind 的 serde `type` 必须等于 `type_name`。
///
/// 首跑即绿，用来在新增/改名事件时抓住两处各写一份的漂移；新增变体要补进 all_event_kinds。
#[test]
fn event_kind_serde_tag_matches_type_name() {
    let kinds = all_event_kinds();
    for kind in &kinds {
        every_variant_is_named(kind);
    }
    assert_eq!(kinds.len(), 26, "all_event_kinds 要覆盖全部变体");
    for kind in kinds {
        let value = serde_json::to_value(&kind).expect("事件序列化");
        assert_eq!(
            value["type"].as_str(),
            Some(kind.type_name()),
            "serde tag 与 type_name 漂移：{value}"
        );
    }
}

#[test]
fn all_event_kinds_round_trip_under_dsh_envelope() {
    let (data_dir, _cwd, mut store) = create_store();
    let id = store.id().clone();
    let kinds = all_event_kinds();
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
    let assistant = &rows[8]["data"]["message"];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["source"]["kind"], "model");
    drop(store);

    let reopened = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(reopened.events().len(), kinds.len());
    for (event, kind) in reopened.events().iter().zip(&kinds) {
        assert_eq!(&event.kind, kind, "信封改形后事件仍能原样读回");
    }
}

/// DSH 的 compaction/* 事件（packages/compaction/compaction/src/types.ts:24-70）：
/// start/end 带 sourceCommandId?、turn 可为 null（两次 turn 之间的手动压缩）；summary 带
/// 摘要内容块、调用事实（provider/model/maxTokens/usage/rawOutput）与 shadowed 区间/seqs。
#[test]
fn compaction_events_match_dsh() {
    let (_data_dir, _cwd, mut store) = create_store();
    store
        .append(&[
            EventKind::CompactionStart {
                compaction_id: "c1".to_owned(),
                source_command_id: Some("cmd-1".to_owned()),
                turn: None,
            },
            EventKind::CompactionSummary {
                compaction_id: "c1".to_owned(),
                source_command_id: Some("cmd-1".to_owned()),
                summary: vec![ContentBlock::text("摘要")],
                raw_output: vec![ContentBlock::text("摘要")],
                llm_stream_call: true,
                shadowed_range: ShadowedRange { start: 2, end: 3 },
                shadowed_seqs: vec![2, 3],
                shadowed_token_count: 5,
                provider: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                max_tokens: Some(321),
                usage: Some(Usage {
                    input_tokens: Some(7),
                    output_tokens: Some(2),
                    ..Usage::default()
                }),
            },
            EventKind::CompactionEnd {
                compaction_id: "c1".to_owned(),
                source_command_id: Some("cmd-1".to_owned()),
                turn: None,
                error: None,
            },
        ])
        .unwrap();

    let (_header, rows) = log_rows(&store);
    assert_eq!(
        rows[0]["data"],
        json!({
            "compactionId": "c1",
            "sourceCommandId": "cmd-1",
            "turn": null,
        }),
        "compaction/start 照 DSH"
    );
    assert_eq!(
        rows[1]["data"],
        json!({
            "compactionId": "c1",
            "sourceCommandId": "cmd-1",
            "summary": [{"type": "text", "text": "摘要"}],
            "rawOutput": [{"type": "text", "text": "摘要"}],
            "llmStreamCall": true,
            "shadowedRange": {"start": 2, "end": 3},
            "shadowedSeqs": [2, 3],
            "shadowedTokenCount": 5,
            "provider": "deepseek",
            "model": "deepseek-flash",
            "maxTokens": 321,
            "usage": {"inputTokens": 7, "outputTokens": 2},
        }),
        "compaction/summary 照 DSH"
    );
    assert_eq!(
        rows[2]["data"],
        json!({
            "compactionId": "c1",
            "sourceCommandId": "cmd-1",
            "turn": null,
        }),
        "compaction/end 照 DSH（成功的 end 省略 error）"
    );

    // 自动压缩（turn 号、无 sourceCommandId、失败 end 带 error）。
    store
        .append(&[
            EventKind::CompactionStart {
                compaction_id: "c2".to_owned(),
                source_command_id: None,
                turn: Some(1),
            },
            EventKind::CompactionEnd {
                compaction_id: "c2".to_owned(),
                source_command_id: None,
                turn: Some(1),
                error: Some("boom".to_owned()),
            },
        ])
        .unwrap();
    let (_header, rows) = log_rows(&store);
    assert_eq!(
        rows[3]["data"],
        json!({"compactionId": "c2", "turn": 1}),
        "自动压缩的 start 省略 sourceCommandId"
    );
    assert_eq!(
        rows[4]["data"],
        json!({"compactionId": "c2", "turn": 1, "error": "boom"}),
        "失败的 end 带 error"
    );
}

/// 压缩替换写成日志里的 user/message + envelope 的 surfaceOp/sourceEventSeqs（DSH
/// compaction/region.ts:491-509）：派生视图读这条消息，不再自己合成 checkpoint。
#[test]
fn compaction_replacement_is_logged_message_with_surface_op() {
    let (_data_dir, _cwd, mut store) = create_store();
    let early = Message::text(Role::User, "早先的话");
    let checkpoint = Message::with_source(
        model::MessageId::parse("checkpoint-1"),
        Role::User,
        vec![ContentBlock::text("checkpoint 内容")],
        model::MessageSource::CompactCheckpoint {
            compaction_id: "c1".to_owned(),
            source_command_id: None,
        },
    );
    store
        .append(&[
            EventKind::TurnStart { turn: 1 },
            EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: early.clone(),
            },
            EventKind::CompactionStart {
                compaction_id: "c1".to_owned(),
                source_command_id: None,
                turn: Some(1),
            },
            EventKind::CompactionSummary {
                compaction_id: "c1".to_owned(),
                source_command_id: None,
                summary: vec![ContentBlock::text("摘要")],
                raw_output: vec![ContentBlock::text("摘要")],
                llm_stream_call: true,
                shadowed_range: ShadowedRange { start: 2, end: 2 },
                shadowed_seqs: vec![2],
                shadowed_token_count: 3,
                provider: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                max_tokens: None,
                usage: None,
            },
        ])
        .unwrap();
    let start_seq = store.events()[2].seq;
    let summary_seq = store.events()[3].seq;
    store
        .append_surface(
            EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: checkpoint.clone(),
            },
            crate::SurfaceOp::Replace {
                start_seq: 2,
                end_seq: 2,
            },
            vec![start_seq, summary_seq, 2],
        )
        .unwrap();

    let (_header, rows) = log_rows(&store);
    let replacement = rows
        .iter()
        .find(|row| row["data"]["message"]["id"] == "checkpoint-1")
        .expect("替换消息在日志里");
    assert_eq!(replacement["type"], "user/message");
    assert_eq!(
        replacement["surfaceOp"],
        json!({"op": "replace", "startSeq": 2, "endSeq": 2}),
        "surfaceOp 在事件信封上"
    );
    assert_eq!(
        replacement["sourceEventSeqs"],
        json!([start_seq, summary_seq, 2]),
        "sourceEventSeqs 列出被替换与引用的事件"
    );

    // 派生视图：被替换区间隐藏，替换消息本身照常派生（且就是日志里的同一条，id 不变）。
    let derived = crate::derive::derive_messages(store.events());
    assert_eq!(derived.len(), 1, "只剩一条 checkpoint：{derived:?}");
    assert_eq!(
        derived[0].id, checkpoint.id,
        "checkpoint 用日志消息的稳定 id"
    );
    assert_eq!(derived[0].role, Role::User);
    assert!(
        !derived
            .iter()
            .any(|message| format!("{message:?}").contains("早先的话")),
        "被替换的早期消息不能出现在派生视图里"
    );
}

/// compaction/prune 照 DSH shadow-price 形状（区间/seqs/估算 token）；修剪替换是
/// 一条带 surfaceOp replace 的 tool/result，派生只读替换消息。
#[test]
fn compaction_prune_matches_dsh() {
    let (_data_dir, _cwd, mut store) = create_store();
    let original = Message::tool_result("call_0", "原始内容", false);
    store
        .append(&[
            EventKind::TurnStart { turn: 1 },
            EventKind::ToolResult {
                turn: 1,
                step: 1,
                message: original.clone(),
                error: None,
                meta: None,
            },
        ])
        .unwrap();
    let original_seq = store.events()[1].seq;
    store
        .append(&[EventKind::CompactionPrune {
            shadowed_range: ShadowedRange {
                start: original_seq,
                end: original_seq,
            },
            shadowed_seqs: vec![original_seq],
            shadowed_token_count: 5,
        }])
        .unwrap();
    let mut replacement = original;
    replacement.content = vec![ContentBlock::text("修剪后")];
    store
        .append_surface(
            EventKind::ToolResult {
                turn: 1,
                step: 1,
                message: replacement,
                error: None,
                meta: None,
            },
            crate::SurfaceOp::Replace {
                start_seq: original_seq,
                end_seq: original_seq,
            },
            vec![original_seq],
        )
        .unwrap();

    let (_header, rows) = log_rows(&store);
    assert_eq!(
        rows[2]["data"],
        json!({
            "shadowedRange": {"start": original_seq, "end": original_seq},
            "shadowedSeqs": [original_seq],
            "shadowedTokenCount": 5,
        }),
        "compaction/prune data 照 DSH"
    );
    assert_eq!(rows[3]["type"], "tool/result");
    assert_eq!(
        rows[3]["surfaceOp"],
        json!({"op": "replace", "startSeq": original_seq, "endSeq": original_seq})
    );
    assert_eq!(rows[3]["sourceEventSeqs"], json!([original_seq]));

    let derived = crate::derive::derive_messages(store.events());
    assert_eq!(derived.len(), 1, "原始 tool/result 被 surface replace 藏掉");
    assert!(
        derived[0]
            .content
            .iter()
            .filter_map(ContentBlock::as_text)
            .any(|text| text == "修剪后"),
        "派生读替换消息"
    );
}

/// job/registered、job/settled 照 DSH SessionEventMap 的 JobRegisteredData /
/// JobSettledData（docs/subsystems/jobs.zh.md），FORMAT_VERSION 升到 7；派生视图忽略它们。
#[test]
fn job_lifecycle_events_match_dsh() {
    let (_data_dir, _cwd, mut store) = create_store();
    let tool_call = JobToolCallData {
        call_id: "call_0".to_owned(),
        root_call_id: "call_0".to_owned(),
    };
    store
        .append(&[
            EventKind::JobRegistered {
                job_id: "bash-0123456789abcdef".to_owned(),
                owner: Some("sess-1".to_owned()),
                kind: "bash".to_owned(),
                label: "echo hi".to_owned(),
                started_at: 100,
                tool_call: Some(tool_call.clone()),
            },
            EventKind::JobSettled {
                job_id: "bash-0123456789abcdef".to_owned(),
                owner: Some("sess-1".to_owned()),
                kind: "bash".to_owned(),
                label: "echo hi".to_owned(),
                started_at: 100,
                tool_call: Some(tool_call),
                finished_at: 200,
                status: "completed".to_owned(),
                cause: "producer".to_owned(),
                producer_reported: true,
                detail: Some("exit code: 0".to_owned()),
            },
        ])
        .unwrap();

    let (header, rows) = log_rows(&store);
    assert_eq!(
        header["version"],
        crate::FORMAT_VERSION,
        "版本断言跟 FORMAT_VERSION 常量"
    );
    assert_eq!(rows.len(), 2);
    assert_envelope(&rows[0]);
    assert_envelope(&rows[1]);
    assert_eq!(rows[0]["type"], "job/registered");
    assert_eq!(
        rows[0]["data"],
        json!({
            "jobId": "bash-0123456789abcdef",
            "owner": "sess-1",
            "kind": "bash",
            "label": "echo hi",
            "startedAt": 100,
            "toolCall": {"callId": "call_0", "rootCallId": "call_0"},
        }),
        "job/registered data 照 DSH JobRegisteredData"
    );
    assert_eq!(rows[1]["type"], "job/settled");
    assert_eq!(
        rows[1]["data"],
        json!({
            "jobId": "bash-0123456789abcdef",
            "owner": "sess-1",
            "kind": "bash",
            "label": "echo hi",
            "startedAt": 100,
            "toolCall": {"callId": "call_0", "rootCallId": "call_0"},
            "finishedAt": 200,
            "status": "completed",
            "cause": "producer",
            "producerReported": true,
            "detail": "exit code: 0",
        }),
        "job/settled data = 注册字段 + 结算字段（DSH JobSettledData）"
    );

    // 生命周期事件只是日志记录，不进派生消息（DSH：recordSessionEvent 不改对话）。
    assert!(
        crate::derive::derive_messages(store.events()).is_empty(),
        "job 生命周期事件不进派生视图"
    );
    // 重新打开校验：新版本事件可读回。
    let reopened = SessionStore::open_read_only(
        store
            .path()
            .parent()
            .and_then(|path| path.parent())
            .unwrap(),
        store.id(),
    )
    .unwrap();
    assert_eq!(
        reopened
            .events()
            .iter()
            .filter(|event| matches!(
                event.kind,
                EventKind::JobRegistered { .. } | EventKind::JobSettled { .. }
            ))
            .count(),
        2,
        "重开日志能读回 job 生命周期事件"
    );
}

/// assistant stream 的 tool-call-delta 帧与 tool-call-chunks 记录照 DSH
/// （StreamChunk types.ts:500；assistant-stream.ts:35-43）。
#[test]
fn tool_call_stream_records_match_dsh() {
    let (data_dir, _cwd, mut store) = create_store();
    store
        .append(&[EventKind::AssistantAttempt {
            turn: 1,
            step: 1,
            stream: vec![
                AssistantStreamRecord::Chunk {
                    time: 10,
                    chunk: StreamChunk::ToolCallDelta {
                        index: 0,
                        id: "call_0".to_owned(),
                        name: Some("read".to_owned()),
                        arguments_delta: String::new(),
                    },
                },
                AssistantStreamRecord::ToolCallChunks {
                    time0: 20,
                    index: 0,
                    dt: vec![3],
                    id: "call_0".to_owned(),
                    name: None,
                    args: vec!["{\"a\":".to_owned(), "1}".to_owned()],
                },
            ],
        }])
        .unwrap();
    let (_header, rows) = log_rows(&store);
    let stream = rows[0]["data"]["stream"].as_array().expect("stream 数组");
    assert_eq!(
        stream[0],
        json!({
            "type": "chunk",
            "time": 10,
            "chunk": {
                "type": "tool-call-delta",
                "index": 0,
                "id": "call_0",
                "name": "read",
                "argumentsDelta": ""
            }
        })
    );
    assert_eq!(
        stream[1],
        json!({
            "type": "tool-call-chunks",
            "time0": 20,
            "index": 0,
            "dt": [3],
            "id": "call_0",
            "args": ["{\"a\":", "1}"]
        }),
        "记录形状照 DSH（没有 name 时不写 name 键）"
    );
    // 重新打开能读回同样的帧记录。
    let reopened = SessionStore::open_read_only(data_dir.path(), store.id()).expect("日志可读回");
    assert_eq!(reopened.events().len(), 1);
    assert!(matches!(
        &reopened.events()[0].kind,
        EventKind::AssistantAttempt { stream, .. }
            if matches!(stream[1], AssistantStreamRecord::ToolCallChunks { .. })
    ));
}
