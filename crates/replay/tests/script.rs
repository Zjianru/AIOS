//! T010.1：脚本推导与覆盖文件（清单 1 的输入侧 + 清单 2 的覆盖语义）。

use model::{AssistantStreamRecord, Message, Role, Usage};
use replay::{ReplayCall, ReplayScript};
use session::{Event, EventKind};

fn chunk(text: &str) -> AssistantStreamRecord {
    AssistantStreamRecord::TextChunks {
        time0: 1,
        index: 0,
        dt: Vec::new(),
        texts: vec![text.to_owned()],
    }
}

fn usage() -> Usage {
    Usage {
        input_tokens: Some(5),
        output_tokens: Some(2),
        cache_read_input_tokens: Some(1),
        cache_creation_input_tokens: None,
    }
}

fn message_event(seq: u64, text: &str) -> Event {
    Event::new(
        seq,
        seq * 10,
        EventKind::AssistantMessage {
            turn: 1,
            step: 1,
            message: Message::text(Role::Assistant, text),
            stream: vec![chunk(text)],
            usage: Some(usage()),
            interrupted: None,
        },
    )
}

fn attempt_event(seq: u64, text: &str) -> Event {
    Event::new(
        seq,
        seq * 10,
        EventKind::AssistantAttempt {
            turn: 1,
            step: 1,
            stream: vec![chunk(text)],
        },
    )
}

fn write_log(events: &[Event]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("临时目录");
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).expect("sessions");
    let mut text = String::from(
        "{\"type\":\"session\",\"version\":2,\"id\":\"00000000-0000-4000-8000-000000000000\",\"createdAt\":0,\"cwd\":\"/tmp\"}\n",
    );
    for event in events {
        text.push_str(&serde_json::to_string(event).expect("序列化事件"));
        text.push('\n');
    }
    std::fs::write(sessions.join("s.jsonl"), text).expect("写日志");
    dir
}

#[test]
fn derives_one_call_per_assistant_settlement_in_order() {
    let dir = write_log(&[message_event(1, "一"), attempt_event(2, "二")]);
    let log = dir.path().join("sessions").join("s.jsonl");
    let script = ReplayScript::from_log(&log).expect("推脚本");
    assert_eq!(script.len(), 2);
    match &script.calls[0] {
        ReplayCall::Stream(records) => assert_eq!(records, &vec![chunk("一")]),
        other => panic!("第一条不是流：{other:?}"),
    }
    match &script.calls[1] {
        ReplayCall::Stream(records) => assert_eq!(records, &vec![chunk("二")]),
        other => panic!("第二条不是流：{other:?}"),
    }
}

#[test]
fn bare_override_replaces_the_whole_script() {
    let dir = write_log(&[message_event(1, "一")]);
    let log = dir.path().join("sessions").join("s.jsonl");
    let override_path = dir.path().join("replay.override.json");
    std::fs::write(
        &override_path,
        serde_json::to_string(&serde_json::json!([
            {"kind": "stream", "records": [{"type": "text-chunks", "time0": 1, "index": 0, "dt": [], "texts": ["替换"]}]},
            {"kind": "throw", "status": 429, "body": {"error": {"message": "太频繁"}}},
            {"kind": "hang"}
        ]))
        .expect("覆盖文件"),
    )
    .expect("写覆盖");
    let script = ReplayScript::from_log_with_override(&log, Some(&override_path)).expect("读覆盖");
    assert_eq!(script.len(), 3);
    assert!(matches!(&script.calls[1], ReplayCall::Throw { status: 429, .. }));
    assert!(matches!(script.calls[2], ReplayCall::Hang));
}

#[test]
fn patches_replace_and_append() {
    let dir = write_log(&[message_event(1, "一"), message_event(2, "二")]);
    let log = dir.path().join("sessions").join("s.jsonl");
    let override_path = dir.path().join("replay.override.json");
    std::fs::write(
        &override_path,
        serde_json::to_string(&serde_json::json!({
            "patches": [
                {"at": 1, "entry": {"kind": "hang"}},
                {"at": 2, "entry": {"kind": "throw", "status": 500}}
            ]
        }))
        .expect("覆盖文件"),
    )
    .expect("写覆盖");
    let script = ReplayScript::from_log_with_override(&log, Some(&override_path)).expect("读覆盖");
    assert_eq!(script.len(), 3, "替换 + 追加");
    assert!(matches!(script.calls[1], ReplayCall::Hang));
    assert!(matches!(&script.calls[2], ReplayCall::Throw { status: 500, .. }));
}

#[test]
fn patch_beyond_length_is_rejected() {
    let dir = write_log(&[message_event(1, "一")]);
    let log = dir.path().join("sessions").join("s.jsonl");
    let override_path = dir.path().join("replay.override.json");
    std::fs::write(
        &override_path,
        serde_json::to_string(&serde_json::json!({"patches": [{"at": 3, "entry": {"kind": "hang"}}]}))
            .expect("覆盖文件"),
    )
    .expect("写覆盖");
    assert!(ReplayScript::from_log_with_override(&log, Some(&override_path)).is_err());
}
