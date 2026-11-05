//! T011.2 清单 1、2、9：ledger 挂在会话「追加之后」的钩子上。

use model::Message;
use monitor::install_ledger;
use session::{EventKind, LlmFailure, SessionStore, ToolError, TurnEndReason};

fn tool_result(message: Message, error: Option<ToolError>) -> EventKind {
    EventKind::ToolResult {
        turn: 1,
        step: 1,
        message,
        error,
    }
}

fn read_ledgers(path: &std::path::Path) -> Vec<serde_json::Value> {
    let monitor_dir = path.join("monitor");
    let mut files: Vec<_> = std::fs::read_dir(monitor_dir)
        .expect("monitor 目录")
        .map(|entry| entry.expect("目录项").path())
        .collect();
    files.sort();
    let mut records = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(file).expect("读监控文件");
        for line in text.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("监控行 JSON");
            if value["channel"] == "ledger" {
                records.push(value);
            }
        }
    }
    records
}

#[test]
fn ledger_records_every_event_in_order_with_severity() {
    let data = tempfile::tempdir().expect("数据目录");
    let cwd = tempfile::tempdir().expect("工作目录");
    let mut store = SessionStore::create(data.path(), cwd.path()).expect("建会话");
    let session_id = store.id().as_str().to_owned();
    let monitor = monitor::Monitor::open_with_capacity(data.path(), 1024).expect("监控");
    install_ledger(&monitor, &mut store);

    store
        .append(&[
            EventKind::TurnStart { turn: 1 },
            tool_result(
                Message::tool_result("call_1", "工具失败了", true),
                Some(ToolError {
                    name: "read".to_owned(),
                    code: "E_READ".to_owned(),
                }),
            ),
            EventKind::TurnEnd {
                turn: 1,
                reason: TurnEndReason::Error {
                    error: LlmFailure {
                        message: "模型失败".to_owned(),
                        code: "SERVER".to_owned(),
                    },
                },
            },
        ])
        .expect("第一批");
    store
        .append(&[
            EventKind::TurnStart { turn: 2 },
            tool_result(Message::tool_result("call_2", "ok", false), None),
            EventKind::TurnEnd {
                turn: 2,
                reason: TurnEndReason::Completed,
            },
        ])
        .expect("第二批");
    drop(store);
    monitor.shutdown();

    let records = read_ledgers(data.path());
    assert_eq!(records.len(), 6, "每条会话事件一条 ledger");
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record["attributes"]["session.id"], session_id);
        assert_eq!(
            record["attributes"]["event.seq"],
            (index + 1) as u64,
            "顺序=seq"
        );
        let source = &record["sourceEvent"];
        assert_eq!(source["sessionId"], session_id);
        assert!(
            source["event"].get("data").is_none(),
            "sourceEvent 是去掉 data 的事件信封：{source}"
        );
        assert!(source["event"]["type"].as_str().is_some());
        assert!(
            record["body"].is_object(),
            "body 是事件副本：{}",
            record["body"]
        );
    }
    assert_eq!(records[1]["severity"], "error", "工具出错 = error");
    assert_eq!(records[2]["severity"], "error", "一轮以出错结束 = error");
    assert_eq!(records[0]["severity"], "info");
    assert_eq!(records[3]["severity"], "info");
    assert_eq!(records[4]["severity"], "info");
    assert_eq!(records[5]["severity"], "info");
    assert_eq!(records[0]["attributes"]["event.type"], "turn/start");
    assert_eq!(records[1]["attributes"]["event.type"], "tool/result");
    assert_eq!(records[2]["attributes"]["event.type"], "turn/end");
}

#[test]
fn reopened_session_only_records_events_after_reopen() {
    let data = tempfile::tempdir().expect("数据目录");
    let cwd = tempfile::tempdir().expect("工作目录");
    let mut store = SessionStore::create(data.path(), cwd.path()).expect("建会话");
    let id = store.id().clone();
    let first_monitor = monitor::Monitor::open_with_capacity(data.path(), 128).expect("第一个监控");
    install_ledger(&first_monitor, &mut store);
    store
        .append(&[
            EventKind::TurnStart { turn: 1 },
            EventKind::TurnEnd {
                turn: 1,
                reason: TurnEndReason::Completed,
            },
        ])
        .expect("追加旧事件");
    drop(store);
    first_monitor.shutdown();

    // T009.H6：等下一次毫秒启动时间，让两个监控文件不同名
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut reopened = SessionStore::open(data.path(), &id).expect("重开会话");
    let second_monitor =
        monitor::Monitor::open_with_capacity(data.path(), 128).expect("第二个监控");
    install_ledger(&second_monitor, &mut reopened);
    reopened
        .append(&[EventKind::TurnStart { turn: 2 }])
        .expect("追加新事件");
    drop(reopened);
    second_monitor.shutdown();

    let monitor_dir = data.path().join("monitor");
    let mut files: Vec<_> = std::fs::read_dir(monitor_dir)
        .expect("monitor 目录")
        .map(|entry| entry.expect("目录项").path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "两个进程记录文件");
    let second = std::fs::read_to_string(&files[1]).expect("读第二个文件");
    let ledger: Vec<serde_json::Value> = second
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON"))
        .filter(|record: &serde_json::Value| record["channel"] == "ledger")
        .collect();
    assert_eq!(ledger.len(), 1, "重开只采重开之后的事件：{second}");
    assert_eq!(ledger[0]["attributes"]["event.seq"], 3, "seq 接着日志继续");
}
