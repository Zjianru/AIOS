//! T011.3 清单 5、6：ledger 副本脱敏（推理/凭证），会话日志不动。

use model::{AssistantStreamRecord, ContentBlock, Message};
use monitor::{
    Channel, Monitor, Record, RedactError, Redactor, Severity, install_ledger_with_redactor,
};
use serde_json::{Map, json};
use session::{EventKind, SessionStore};

fn monitor_records(data: &std::path::Path) -> Vec<serde_json::Value> {
    let mut files: Vec<_> = std::fs::read_dir(data.join("monitor"))
        .expect("monitor 目录")
        .map(|entry| entry.expect("目录项").path())
        .collect();
    files.sort();
    let mut records = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(file).expect("读监控文件");
        for line in text.lines() {
            records.push(serde_json::from_str(line).expect("JSON"));
        }
    }
    records
}

fn session_text(data: &std::path::Path) -> String {
    let mut files: Vec<_> = std::fs::read_dir(data.join("sessions"))
        .expect("sessions 目录")
        .map(|entry| entry.expect("目录项").path())
        .collect();
    files.sort();
    files
        .iter()
        .map(|file| std::fs::read_to_string(file).expect("读会话日志"))
        .collect()
}

#[test]
fn reasoning_text_is_replaced_with_character_counts() {
    let data = tempfile::tempdir().expect("数据目录");
    let cwd = tempfile::tempdir().expect("工作目录");
    let mut store = SessionStore::create(data.path(), cwd.path()).expect("建会话");
    let monitor = Monitor::open_with_capacity(data.path(), 128).expect("监控");
    install_ledger_with_redactor(&monitor, &mut store, Redactor::new(Vec::new()));
    store
        .append(&[EventKind::AssistantMessage {
            turn: 1,
            step: 1,
            message: Message::assistant(
                vec![
                    ContentBlock::Reasoning {
                        text: "先想一下".to_owned(),
                        signature: None,
                    },
                    ContentBlock::text("回答"),
                ],
                "deepseek",
                "deepseek-flash",
            ),
            stream: vec![AssistantStreamRecord::ReasoningChunks {
                time0: 1,
                index: 0,
                dt: Vec::new(),
                texts: vec!["很长的思考".to_owned(), "继续".to_owned()],
            }],
            usage: None,
            interrupted: None,
        }])
        .expect("追加");
    drop(store);
    monitor.shutdown();

    let records = monitor_records(data.path());
    let ledger = records
        .iter()
        .find(|record| record["channel"] == "ledger")
        .expect("一条 ledger");
    let body = ledger["body"].to_string();
    assert!(body.contains("[推理已略去，4 字]"), "思考块 N=4：{body}");
    assert!(
        body.contains("[推理已略去，7 字]"),
        "reasoning-chunks N=7（很长的思考 5 + 继续 2）：{body}"
    );
    assert!(!body.contains("先想一下"), "推理原文不在副本里：{body}");
    assert!(!body.contains("很长的思考"), "推理帧原文不在副本里：{body}");
    assert!(!body.contains("继续"), "推理帧原文不在副本里：{body}");
    // 只改副本：会话日志里原文还在。
    let log = session_text(data.path());
    assert!(
        log.contains("先想一下") && log.contains("很长的思考"),
        "会话日志不动"
    );
}

#[test]
fn held_key_is_redacted_from_any_string() {
    let data = tempfile::tempdir().expect("数据目录");
    let cwd = tempfile::tempdir().expect("工作目录");
    let mut store = SessionStore::create(data.path(), cwd.path()).expect("建会话");
    let monitor = Monitor::open_with_capacity(data.path(), 128).expect("监控");
    install_ledger_with_redactor(
        &monitor,
        &mut store,
        Redactor::new(vec!["sk-secret-123".to_owned()]),
    );
    store
        .append(&[EventKind::ToolResult {
            turn: 1,
            step: 1,
            message: Message::tool_result("call_1", "输出 sk-secret-123 结束", false),
            error: None,
        }])
        .expect("追加");
    drop(store);
    monitor.shutdown();

    let records = monitor_records(data.path());
    let ledger = records
        .iter()
        .find(|record| record["channel"] == "ledger")
        .expect("一条 ledger");
    let body = ledger["body"].to_string();
    assert!(!body.contains("sk-secret-123"), "key 不在副本里：{body}");
    assert!(body.contains("[已脱敏]"), "有脱敏标记：{body}");
    let log = session_text(data.path());
    assert!(
        log.contains("sk-secret-123"),
        "会话日志里 key 原文不动（只改副本）"
    );
}

#[test]
fn too_deep_record_is_rejected_and_can_be_dropped() {
    let mut nested = json!("底");
    for _ in 0..100 {
        nested = json!({"x": nested});
    }
    let mut record = Record::new(Channel::Ops, Severity::Info, Map::new(), nested);
    let redactor = Redactor::new(Vec::new());
    assert!(matches!(
        redactor.redact(&mut record),
        Err(RedactError::TooDeep)
    ));
}
