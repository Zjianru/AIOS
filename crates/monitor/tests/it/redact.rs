//! T011.3/F1 清单 5、6：所有记录在写线程出口统一脱敏（ledger/ops/推理/凭证）。

use model::{AssistantStreamRecord, ContentBlock, Message};
use monitor::{Channel, Monitor, Record, Severity, SourceEvent};
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

fn wire_ledger(monitor: &Monitor, store: &mut SessionStore) {
    let factory = monitor.append_hook_factory();
    store.set_append_hook(Some(factory(store.id())));
}

#[test]
fn reasoning_text_is_replaced_with_character_counts() {
    let data = tempfile::tempdir().expect("数据目录");
    let cwd = tempfile::tempdir().expect("工作目录");
    let mut store = SessionStore::create(data.path(), cwd.path()).expect("建会话");
    let monitor = Monitor::open_with_capacity(data.path(), 128).expect("监控");
    monitor.set_secrets(Vec::new());
    wire_ledger(&monitor, &mut store);
    store
        .append(&[EventKind::AssistantMessage {
            effort: None,
            effort_wanted: None,
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
    let log = session_text(data.path());
    assert!(
        log.contains("先想一下") && log.contains("很长的思考"),
        "会话日志不动"
    );
}

#[test]
fn ledger_and_ops_keys_are_redacted_from_any_string() {
    let data = tempfile::tempdir().expect("数据目录");
    let cwd = tempfile::tempdir().expect("工作目录");
    let mut store = SessionStore::create(data.path(), cwd.path()).expect("建会话");
    let monitor = Monitor::open_with_capacity(data.path(), 128).expect("监控");
    monitor.set_secrets(vec!["sk-secret-123".to_owned()]);
    wire_ledger(&monitor, &mut store);
    store
        .append(&[EventKind::ToolResult {
            turn: 1,
            step: 1,
            message: Message::tool_result("call_1", "输出 sk-secret-123 结束", false),
            error: None,
            meta: None,
        }])
        .expect("追加");
    let mut attributes = Map::new();
    attributes.insert("telemetry.op".to_owned(), json!("api-call"));
    attributes.insert("echo".to_owned(), json!("sk-secret-123"));
    assert!(monitor.record(Record::new(
        Channel::Ops,
        Severity::Info,
        attributes,
        json!({"nested": ["sk-secret-123"]}),
    )));
    drop(store);
    monitor.shutdown();

    let records = monitor_records(data.path());
    let ledger = records
        .iter()
        .find(|record| record["channel"] == "ledger")
        .expect("一条 ledger");
    let ledger_text = ledger.to_string();
    assert!(
        !ledger_text.contains("sk-secret-123"),
        "ledger key 原文：{ledger_text}"
    );
    assert!(
        ledger_text.contains("[已脱敏]"),
        "ledger 有脱敏标记：{ledger_text}"
    );
    let ops = records
        .iter()
        .find(|record| {
            record["channel"] == "ops" && record["attributes"]["telemetry.op"] == "api-call"
        })
        .expect("ops 记录");
    let ops_text = ops.to_string();
    assert!(
        !ops_text.contains("sk-secret-123"),
        "ops key 原文：{ops_text}"
    );
    assert!(ops_text.contains("[已脱敏]"), "ops 有脱敏标记：{ops_text}");
    let log = session_text(data.path());
    assert!(
        log.contains("sk-secret-123"),
        "会话日志里 key 原文不动（只改副本）"
    );
}

#[test]
fn secrets_added_after_start_apply_to_later_records() {
    let data = tempfile::tempdir().expect("数据目录");
    let monitor = Monitor::open_with_capacity(data.path(), 128).expect("监控");
    monitor.set_secrets(Vec::new());
    let mut map = Map::new();
    map.insert("telemetry.op".to_owned(), json!("api-call"));
    monitor.record(Record::new(
        Channel::Ops,
        Severity::Info,
        map,
        json!({"key": "sk-late-456"}),
    ));
    // 等第一条真的写盘（写线程异步），再模拟运行中保存后的 secrets 刷新。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while monitor_records(data.path()).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "第一条没有在 5 秒内写盘"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    monitor.set_secrets(vec!["sk-late-456".to_owned()]);
    let mut map = Map::new();
    map.insert("telemetry.op".to_owned(), json!("api-call"));
    map.insert("echo".to_owned(), json!("sk-late-456"));
    monitor.record(Record::new(
        Channel::Ops,
        Severity::Info,
        map,
        json!({"key": "sk-late-456"}),
    ));
    monitor.shutdown();
    let records = monitor_records(data.path());
    let texts: Vec<String> = records
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert!(
        texts.iter().any(|text| text.contains("[已脱敏]")),
        "后加的 key 被脱敏：{texts:?}"
    );
    assert_eq!(
        texts
            .iter()
            .filter(|text| text.contains("sk-late-456"))
            .count(),
        1,
        "只有加 key 之前那条保留原文（历史记录不追溯）"
    );
}

#[test]
fn too_deep_record_is_dropped_and_redact_failed_is_recorded() {
    let data = tempfile::tempdir().expect("数据目录");
    let monitor = Monitor::open_with_capacity(data.path(), 128).expect("监控");
    monitor.set_secrets(Vec::new());
    let mut nested = json!("底");
    for _ in 0..100 {
        nested = json!({"x": nested});
    }
    monitor.record(Record::new(
        Channel::Ops,
        Severity::Info,
        Map::new(),
        nested,
    ));
    monitor.shutdown();
    let records = monitor_records(data.path());
    assert!(
        records
            .iter()
            .any(|record| record["attributes"]["telemetry.op"] == "redact-failed"),
        "过深记录扣下并写 redact-failed：{records:?}"
    );
}

/// T012.8 K5：redact-failed 记事件类型和序号（不是 channel/time），供定位是哪条日志。
#[test]
fn redact_failed_records_event_type_and_seq() {
    let data = tempfile::tempdir().expect("数据目录");
    let monitor = Monitor::open_with_capacity(data.path(), 128).expect("监控");
    monitor.set_secrets(Vec::new());
    let mut nested = json!("底");
    for _ in 0..100 {
        nested = json!({"x": nested});
    }
    let mut record = Record::new(Channel::Ops, Severity::Info, Map::new(), nested);
    record.source_event = Some(SourceEvent {
        session_id: "s-1".to_owned(),
        event: json!({"type": "tool/result", "seq": 42, "data": {}}),
    });
    monitor.record(record);
    monitor.shutdown();
    let records = monitor_records(data.path());
    let failed = records
        .iter()
        .find(|record| record["attributes"]["telemetry.op"] == "redact-failed")
        .expect("redact-failed 记录");
    assert_eq!(
        failed["body"]["eventType"], "tool/result",
        "redact-failed：{failed}"
    );
    assert_eq!(failed["body"]["eventSeq"], 42, "redact-failed：{failed}");
}
