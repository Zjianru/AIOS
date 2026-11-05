//! T006.5c1：会话日志里的消息带 DSH 形状的 id/source；派生消息的 id 稳定。
//!
//! DSH 依据：core/session/src/types.ts:315（user/message data）、:383（tool/result 消息）；
//! llm/llm/src/message.ts:139-180（MessageBase/五角色）、:206-260（构造器生成 id）。

use std::fs;

use model::{ContentBlock, Message, MessageId, MessageSource, Role};
use serde_json::{Value, json};

use crate::{Event, EventKind, SessionStore};

fn log_rows(store: &SessionStore) -> Vec<Value> {
    let text = fs::read_to_string(store.path()).unwrap();
    text.lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn uuid_shaped(raw: &str) -> bool {
    let parts: Vec<&str> = raw.split('-').collect();
    parts.iter().map(|part| part.len()).collect::<Vec<_>>() == vec![8, 4, 4, 4, 12]
        && parts[2].as_bytes().first() == Some(&b'4')
}

#[test]
fn user_and_tool_result_json_carry_id_source() {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let user = Message::user_text("你好");
    let tool = Message::tool_result("call_0", "结果文本", true);
    let user_id = user.id.clone();
    let tool_id = tool.id.clone();
    store
        .append(&[
            EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: user,
            },
            EventKind::ToolResult {
                turn: 1,
                step: 1,
                message: tool,
                error: None,
                meta: None,
            },
        ])
        .unwrap();

    let rows = log_rows(&store);
    let user_json = &rows[0]["data"]["message"];
    assert_eq!(user_json["id"], user_id.as_str(), "user/message 带稳定 id");
    assert_eq!(user_json["role"], "user");
    assert_eq!(user_json["source"], json!({"kind": "user"}));
    assert!(
        uuid_shaped(user_json["id"].as_str().unwrap()),
        "id 是 uuid v4：{user_json}"
    );

    let tool_json = &rows[1]["data"]["message"];
    assert_eq!(tool_json["id"], tool_id.as_str(), "tool/result 带稳定 id");
    assert_eq!(tool_json["role"], "tool");
    assert_eq!(
        tool_json["source"],
        json!({"kind": "tool", "callId": "call_0"}),
        "tool 消息 source 带 callId（DSH types.ts:383）"
    );
    assert_eq!(tool_json["toolCallId"], "call_0");
    assert_eq!(tool_json["isError"], true);
    assert_eq!(
        tool_json["content"],
        json!([{"type": "text", "text": "结果文本"}]),
        "tool 消息 content 是结果内容块，不是 tool_result 块"
    );
    drop(store);

    // 续接读回：事件里的消息 id 不变。
    let reopened = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let ids: Vec<&str> = reopened
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::UserMessage { message, .. } | EventKind::ToolResult { message, .. } => {
                Some(message.id.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        vec![user_id.as_str(), tool_id.as_str()],
        "续接后消息 id 保持"
    );
}

#[test]
fn compaction_checkpoint_message_id_comes_from_log() {
    // T006.3：压缩替换是日志里的 user/message（envelope 带 surfaceOp replace）；派生只读这条
    // 消息，不再自己合成 checkpoint，id 也保持日志里的稳定 id。
    let early = Message::user_text("早先的话");
    let checkpoint = Message::with_source(
        MessageId::parse("checkpoint-1"),
        Role::User,
        vec![ContentBlock::text("checkpoint 内容")],
        MessageSource::CompactCheckpoint {
            compaction_id: "c1".to_owned(),
            source_command_id: None,
        },
    );
    let mut events = vec![
        Event::new(1, 1, EventKind::TurnStart { turn: 1 }),
        Event::new(
            2,
            2,
            EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: early,
            },
        ),
        Event::new(
            3,
            3,
            EventKind::CompactionStart {
                compaction_id: "c1".to_owned(),
                source_command_id: None,
                turn: Some(1),
            },
        ),
        Event::new(
            4,
            4,
            EventKind::CompactionSummary {
                compaction_id: "c1".to_owned(),
                source_command_id: None,
                summary: vec![ContentBlock::text("摘要")],
                raw_output: vec![ContentBlock::text("摘要")],
                llm_stream_call: true,
                shadowed_range: crate::ShadowedRange { start: 2, end: 2 },
                shadowed_seqs: vec![2],
                shadowed_token_count: 3,
                provider: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                max_tokens: None,
                usage: None,
            },
        ),
        Event::new(
            5,
            5,
            EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: checkpoint.clone(),
            },
        ),
        Event::new(
            6,
            6,
            EventKind::CompactionEnd {
                compaction_id: "c1".to_owned(),
                source_command_id: None,
                turn: Some(1),
                error: None,
            },
        ),
    ];
    // 替换消息的 envelope：surfaceOp replace + 引用压缩事件与被替换节点。
    events[4].surface_op = Some(crate::SurfaceOp::Replace {
        start_seq: 2,
        end_seq: 2,
    });
    events[4].source_event_seqs = Some(vec![3, 4, 2]);

    let derived = crate::derive::derive_messages(&events);
    assert_eq!(derived.len(), 1, "只留日志里的 checkpoint：{derived:?}");
    assert_eq!(derived[0].id, checkpoint.id, "checkpoint 用日志消息的 id");
    assert!(
        derived[0]
            .content
            .iter()
            .filter_map(ContentBlock::as_text)
            .any(|text| text.contains("checkpoint 内容")),
        "checkpoint 内容来自日志消息"
    );
    assert_eq!(
        crate::derive::derive_messages(&events)[0].id,
        derived[0].id,
        "多次派生 checkpoint id 不变"
    );
}
