//! T011.1：本机监控记录的形状与写线程（照 DSH session-telemetry 的 record 形状）。
//!
//! 每个进程一个 JSONL 文件：<数据目录>/monitor/<启动时间>-<pid>.jsonl。
//! 入队不阻塞；队列满丢新来的并计数，下一条写入前先写一条 gap；写线程每条 flush。
//! 写失败时写线程停掉、stderr 报一次；业务不受影响。退出时写 shutdown 并排空。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// 队列上限（T011.7 写进开发规范）。
pub const DEFAULT_QUEUE_CAPACITY: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// 会话日志的副本。
    Ledger,
    /// 日志之外的运行信号。
    Ops,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warn,
    Error,
}

/// ledger 记录里的源事件信封：会话 id + 去掉 data 的事件外壳。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceEvent {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub event: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub channel: Channel,
    /// unix 毫秒。
    pub time: u64,
    pub severity: Severity,
    pub attributes: Map<String, Value>,
    pub body: Value,
    #[serde(
        rename = "sourceEvent",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub source_event: Option<SourceEvent>,
}

impl Record {
    pub fn new(
        channel: Channel,
        severity: Severity,
        attributes: Map<String, Value>,
        body: Value,
    ) -> Self {
        Self {
            channel,
            time: now_ms(),
            severity,
            attributes,
            body,
            source_event: None,
        }
    }

    /// 一条 ops 记录：attributes 里带 telemetry.op。
    pub fn ops(op: &str, severity: Severity, body: Value) -> Self {
        let mut attributes = Map::new();
        attributes.insert("telemetry.op".to_owned(), Value::String(op.to_owned()));
        Self::new(Channel::Ops, severity, attributes, body)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    #[error("监控文件不可用：{0}")]
    Io(#[from] std::io::Error),
    #[error("监控记录序列化失败：{0}")]
    Json(#[from] serde_json::Error),
}

enum Message {
    Record(Record),
    Shutdown,
}

trait LineSink: Send {
    fn write_line(&mut self, line: &str) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
}

struct FileSink {
    file: File,
}

impl LineSink for FileSink {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 写一条记录：序列化成一行 JSON，写完立即 flush。
fn write_record(sink: &mut dyn LineSink, record: &Record) -> Result<(), MonitorError> {
    let line = serde_json::to_string(record)?;
    sink.write_line(&line)?;
    sink.flush()?;
    Ok(())
}

/// 队列满丢掉的条数：下一条记录之前先写一条 gap，然后清零。
fn pending_gap(dropped: &AtomicU64) -> Option<Record> {
    let dropped_count = dropped.swap(0, Ordering::SeqCst);
    (dropped_count > 0)
        .then(|| Record::ops("gap", Severity::Warn, json!({ "dropped": dropped_count })))
}

/// 写线程：records 逐条写；所有记录在写入前统一脱敏（ledger/ops/gap/shutdown），
/// 写失败停掉并在标准错误报一次（业务不受影响）；退出时写 shutdown 记录并排空队列。
fn run_writer(
    mut sink: Box<dyn LineSink>,
    rx: Receiver<Message>,
    dropped: Arc<AtomicU64>,
    redactor: Redactor,
) {
    // 仅测试注入用：AIOS_MONITOR_FAIL_AFTER=N 时第 N 条写入之后模拟写失败（DoD4 不可写场景）。
    let fail_after = std::env::var("AIOS_MONITOR_FAIL_AFTER")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    let mut written = 0_u64;
    loop {
        match rx.recv() {
            Ok(Message::Record(record)) => {
                if let Some(gap) = pending_gap(&dropped)
                    && let Err(error) =
                        write_emitted(&mut *sink, gap, &redactor, fail_after, &mut written)
                {
                    eprintln!("monitor: 写监控失败，停止记录：{error}");
                    break;
                }
                if let Err(error) =
                    write_emitted(&mut *sink, record, &redactor, fail_after, &mut written)
                {
                    eprintln!("monitor: 写监控失败，停止记录：{error}");
                    break;
                }
            }
            Ok(Message::Shutdown) => {
                if let Some(gap) = pending_gap(&dropped)
                    && let Err(error) =
                        write_emitted(&mut *sink, gap, &redactor, fail_after, &mut written)
                {
                    eprintln!("monitor: 写监控失败，停止记录：{error}");
                    break;
                }
                let shutdown = Record::ops("shutdown", Severity::Info, json!({}));
                if let Err(error) =
                    write_emitted(&mut *sink, shutdown, &redactor, fail_after, &mut written)
                {
                    eprintln!("monitor: 写监控失败，停止记录：{error}");
                }
                break;
            }
            Err(_) => {
                if let Some(gap) = pending_gap(&dropped) {
                    let _ = write_emitted(&mut *sink, gap, &redactor, fail_after, &mut written);
                }
                break;
            }
        }
    }
}

/// 所有记录交给后端前的唯一出口：先脱敏，再写（脱敏失败的原记录扣下，
/// 只写一条不含原文的 redact-failed ops）。
fn write_emitted(
    sink: &mut dyn LineSink,
    record: Record,
    redactor: &Redactor,
    fail_after: Option<u64>,
    written: &mut u64,
) -> Result<(), MonitorError> {
    let mut record = record;
    match redactor.redact(&mut record) {
        Ok(()) => write_with_injection(sink, &record, fail_after, written),
        Err(_) => {
            let mut body = json!({
                "channel": channel_label(record.channel),
                "time": record.time,
            });
            // T012.8 K5：记事件类型和序号（定位是哪条日志），不记正文。
            if let Some(source) = &record.source_event
                && let Some(event) = source.event.as_object()
            {
                if let Some(kind) = event.get("type").and_then(serde_json::Value::as_str) {
                    body["eventType"] = serde_json::json!(kind);
                }
                if let Some(seq) = event.get("seq").and_then(serde_json::Value::as_u64) {
                    body["eventSeq"] = serde_json::json!(seq);
                }
            }
            let failed = Record::ops("redact-failed", Severity::Warn, body);
            match redactor.redact(&mut failed.clone()) {
                Ok(()) => write_with_injection(sink, &failed, fail_after, written),
                Err(_) => Ok(()),
            }
        }
    }
}

fn channel_label(channel: Channel) -> &'static str {
    match channel {
        Channel::Ledger => "ledger",
        Channel::Ops => "ops",
    }
}

/// 测试注入：到了注入阈值之后返回写失败；未设置时永远写。
fn write_with_injection(
    sink: &mut dyn LineSink,
    record: &Record,
    fail_after: Option<u64>,
    written: &mut u64,
) -> Result<(), MonitorError> {
    if let Some(limit) = fail_after
        && *written >= limit
    {
        return Err(MonitorError::Io(std::io::Error::other(
            "测试注入：监控写失败",
        )));
    }
    *written += 1;
    write_record(sink, record)
}

/// 一个进程的监控记录器（Clone 共享同一个队列；只有持 join 的那一份负责退出排空）。
struct Shared {
    tx: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
    path: PathBuf,
    redactor: Redactor,
}

pub struct Monitor {
    shared: Arc<Shared>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl Clone for Monitor {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            join: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Monitor")
            .field("path", &self.shared.path)
            .finish_non_exhaustive()
    }
}

impl Monitor {
    pub fn open(data_dir: &Path) -> Result<Self, MonitorError> {
        Self::open_with_capacity(data_dir, DEFAULT_QUEUE_CAPACITY)
    }

    pub fn open_with_capacity(data_dir: &Path, capacity: usize) -> Result<Self, MonitorError> {
        let dir = data_dir.join("monitor");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}-{}.jsonl", now_ms(), std::process::id()));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let (tx, rx) = sync_channel(capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let redactor = Redactor::new(Vec::new());
        let join = std::thread::Builder::new()
            .name("aios-monitor".to_owned())
            .spawn({
                let dropped = Arc::clone(&dropped);
                let redactor = redactor.clone();
                move || run_writer(Box::new(FileSink { file }), rx, dropped, redactor)
            })
            .map_err(MonitorError::Io)?;
        Ok(Self {
            shared: Arc::new(Shared {
                tx,
                dropped,
                path,
                redactor,
            }),
            join: Mutex::new(Some(join)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.shared.path
    }

    /// F1：更新进程当前持有的 key 列表（来自 Keyring；由 cli 装配在启动/设置保存/recheck 时刷新）。
    pub fn set_secrets(&self, secrets: Vec<String>) {
        self.shared.redactor.set_secrets(secrets);
    }

    /// F1：会话「追加之后」的钩子工厂。生产（cli serve）和测试用同一个装配入口。
    pub fn append_hook_factory(&self) -> session::AppendHookFactory {
        let monitor = self.clone();
        Arc::new(move |id: &session::SessionId| {
            let monitor = monitor.clone();
            let session_id = id.as_str().to_owned();
            Arc::new(move |events: &[session::Event]| {
                for event in events {
                    let _ = monitor.record(ledger_record(&session_id, event));
                }
            }) as session::AppendHook
        })
    }

    /// 入队；队列满时丢新来的、计数并返回 false（业务不受影响）。
    pub fn record(&self, record: Record) -> bool {
        match self.shared.tx.try_send(Message::Record(record)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.shared.dropped.fetch_add(1, Ordering::SeqCst);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// 退出：写 shutdown 并排空队列。
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let Ok(mut guard) = self.join.lock() else {
            return;
        };
        if let Some(join) = guard.take() {
            let _ = self.shared.tx.send(Message::Shutdown);
            let _ = join.join();
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// T011.2：把一条会话事件写成 ledger 记录。
///
/// body 是事件副本（脱敏在 T011.3 做）；sourceEvent 是会话 id + 去掉 data 的事件信封。
pub fn ledger_record(session_id: &str, event: &session::Event) -> Record {
    let body = serde_json::to_value(event).unwrap_or(Value::Null);
    let mut envelope = body.clone();
    if let Some(object) = envelope.as_object_mut() {
        object.remove("data");
    }
    let mut attributes = Map::new();
    attributes.insert(
        "session.id".to_owned(),
        Value::String(session_id.to_owned()),
    );
    attributes.insert(
        "event.type".to_owned(),
        envelope
            .get("type")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
    );
    attributes.insert("event.seq".to_owned(), json!(event.seq));
    Record {
        channel: Channel::Ledger,
        time: event.time,
        severity: severity_of(&event.kind),
        attributes,
        body,
        source_event: Some(SourceEvent {
            session_id: session_id.to_owned(),
            event: envelope,
        }),
    }
}

/// T011.2（照 DSH severityOf）：工具结果出错、一轮以出错结束为 error，其余 info。
fn severity_of(kind: &session::EventKind) -> Severity {
    match kind {
        session::EventKind::ToolResult { error: Some(_), .. } => Severity::Error,
        session::EventKind::TurnEnd {
            reason: session::TurnEndReason::Error { .. },
            ..
        } => Severity::Error,
        _ => Severity::Info,
    }
}

/// T011.3/F1：脱敏规则（只改副本；key 不落盘，只在内存里匹配）。
#[derive(Debug, Clone)]
struct Redactor {
    secrets: Arc<RwLock<Vec<String>>>,
}

#[derive(Debug, thiserror::Error)]
enum RedactError {
    #[error("监控记录嵌套过深，脱敏放弃")]
    TooDeep,
}

impl Redactor {
    fn new(secrets: Vec<String>) -> Self {
        Self {
            secrets: Arc::new(RwLock::new(secrets)),
        }
    }

    /// F1：Keyring 的 key 在进程生命期内会变；这里原地替换，写线程下一条起生效。
    fn set_secrets(&self, secrets: Vec<String>) {
        if let Ok(mut guard) = self.secrets.write() {
            *guard = secrets;
        }
    }

    /// 推理帧/思考块文本 → [推理已略去，N 字]；任意字符串里的 key → [已脱敏]。
    /// 只改副本；出错（嵌套过深）时由调用方扣下该条。
    fn redact(&self, record: &mut Record) -> Result<(), RedactError> {
        let secrets = self
            .secrets
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        redact_value(&mut record.body, &secrets, 0)?;
        for value in record.attributes.values_mut() {
            redact_value(value, &secrets, 0)?;
        }
        if let Some(source) = &mut record.source_event {
            redact_value(&mut source.event, &secrets, 0)?;
        }
        Ok(())
    }
}

/// 记录嵌套上限：超了就不脱敏、扣下该条。
const MAX_REDACT_DEPTH: usize = 64;

const REASONING_PLACEHOLDER: &str = "[推理已略去，{n} 字]";
const SECRET_PLACEHOLDER: &str = "[已脱敏]";

fn reasoning_placeholder(characters: usize) -> String {
    REASONING_PLACEHOLDER.replace("{n}", &characters.to_string())
}

fn redact_value(value: &mut Value, secrets: &[String], depth: usize) -> Result<(), RedactError> {
    if depth > MAX_REDACT_DEPTH {
        return Err(RedactError::TooDeep);
    }
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("reasoning-chunks")
                && let Some(Value::Array(texts)) = map.get_mut("texts")
            {
                let characters: usize = texts
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|text| text.chars().count())
                    .sum();
                *texts = vec![Value::String(reasoning_placeholder(characters))];
            }
            if map.get("type").and_then(Value::as_str) == Some("reasoning")
                && let Some(Value::String(text)) = map.get_mut("text")
            {
                let characters = text.chars().count();
                *text = reasoning_placeholder(characters);
            }
            for item in map.values_mut() {
                redact_value(item, secrets, depth + 1)?;
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                redact_value(item, secrets, depth + 1)?;
            }
        }
        Value::String(text) => {
            for secret in secrets {
                if !secret.is_empty() && text.contains(secret.as_str()) {
                    *text = text.replace(secret.as_str(), SECRET_PLACEHOLDER);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn read_lines(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .expect("读监控文件")
            .lines()
            .map(|line| serde_json::from_str(line).expect("监控行是 JSON"))
            .collect()
    }

    fn file_sink(path: &Path) -> FileSink {
        FileSink {
            file: OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("打开测试文件"),
        }
    }

    struct FailingSink {
        attempts: Arc<AtomicUsize>,
    }

    impl LineSink for FailingSink {
        fn write_line(&mut self, _line: &str) -> std::io::Result<()> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::other("磁盘满了"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn record_shape_file_location_and_shutdown() {
        let dir = tempfile::tempdir().expect("临时目录");
        let monitor = Monitor::open(dir.path()).expect("打开监控");
        let path = monitor.path().to_path_buf();
        assert!(
            path.starts_with(dir.path().join("monitor")),
            "文件在 <数据目录>/monitor 下"
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".jsonl"),
            "文件是 jsonl：{}",
            path.display()
        );

        let mut attributes = Map::new();
        attributes.insert("session.id".to_owned(), json!("s1"));
        attributes.insert("event.type".to_owned(), json!("turn/start"));
        let mut record = Record::new(
            Channel::Ledger,
            Severity::Info,
            attributes,
            json!({"turn": 1}),
        );
        record.source_event = Some(SourceEvent {
            session_id: "s1".to_owned(),
            event: json!({"type": "turn/start", "seq": 1, "time": 2}),
        });
        assert!(monitor.record(record));
        assert!(monitor.record(Record::ops(
            "api-call",
            Severity::Warn,
            json!({"method": "send"})
        )));
        for index in 0..5 {
            assert!(monitor.record(Record::ops(
                "stream-open",
                Severity::Info,
                json!({"index": index})
            )));
        }
        monitor.shutdown();

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 8, "2 条 + 5 条 + shutdown");
        let first = &lines[0];
        assert_eq!(first["channel"], "ledger");
        assert!(first["time"].as_u64().unwrap() > 0);
        assert_eq!(first["severity"], "info");
        assert_eq!(first["attributes"]["session.id"], "s1");
        assert_eq!(first["sourceEvent"]["sessionId"], "s1");
        assert_eq!(first["sourceEvent"]["event"]["type"], "turn/start");
        assert_eq!(lines[1]["channel"], "ops");
        assert_eq!(lines[1]["attributes"]["telemetry.op"], "api-call");
        assert_eq!(lines[1]["severity"], "warn");
        for (index, line) in lines.iter().enumerate().take(7).skip(2) {
            assert_eq!(line["body"]["index"], index - 2, "记录顺序保持");
        }
        assert_eq!(
            lines[7]["attributes"]["telemetry.op"], "shutdown",
            "退出写 shutdown"
        );
    }

    #[test]
    fn gap_records_dropped_count_before_next_record() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("monitor.jsonl");
        let mut sink = file_sink(&path);
        let dropped = AtomicU64::new(3);
        let gap = pending_gap(&dropped).expect("gap");
        write_record(&mut sink, &gap).expect("写 gap");
        write_record(
            &mut sink,
            &Record::ops("stream-open", Severity::Info, json!({"ok": true})),
        )
        .expect("写下一条");
        sink.flush().expect("flush");
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2, "gap 在下一条之前先写");
        assert_eq!(lines[0]["attributes"]["telemetry.op"], "gap");
        assert_eq!(lines[0]["severity"], "warn");
        assert_eq!(lines[0]["body"]["dropped"], 3);
        assert_eq!(lines[1]["attributes"]["telemetry.op"], "stream-open");
        assert_eq!(dropped.load(Ordering::SeqCst), 0, "gap 写完后计数清零");
        // 守恒：写下的 1 条 + 丢掉的 3 条 = 产生的 4 条。
        assert_eq!(lines.len() - 1 + 3, 4);
    }

    #[test]
    fn write_failure_stops_writer_after_one_attempt() {
        let (tx, rx) = sync_channel(8);
        tx.send(Message::Record(Record::ops(
            "process-start",
            Severity::Info,
            json!({}),
        )))
        .unwrap();
        tx.send(Message::Record(Record::ops(
            "shutdown",
            Severity::Info,
            json!({}),
        )))
        .unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let sink = FailingSink {
            attempts: Arc::clone(&attempts),
        };
        let handle = std::thread::spawn(move || {
            run_writer(
                Box::new(sink),
                rx,
                Arc::new(AtomicU64::new(0)),
                Redactor::new(Vec::new()),
            );
        });
        handle.join().expect("写线程应停下而不是 panic");
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "写失败后不再尝试第二条");
    }
}
