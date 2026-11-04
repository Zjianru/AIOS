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
use std::sync::{Arc, Mutex};
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
fn write_pending_gap(sink: &mut dyn LineSink, dropped: &AtomicU64) -> Result<(), MonitorError> {
    let dropped_count = dropped.swap(0, Ordering::SeqCst);
    if dropped_count == 0 {
        return Ok(());
    }
    write_record(
        sink,
        &Record::ops("gap", Severity::Warn, json!({ "dropped": dropped_count })),
    )
}

/// 写线程：records 逐条写；写失败停掉并在标准错误报一次（业务不受影响）；
/// 退出时写 shutdown 记录并排空队列。
fn run_writer(mut sink: Box<dyn LineSink>, rx: Receiver<Message>, dropped: Arc<AtomicU64>) {
    loop {
        match rx.recv() {
            Ok(Message::Record(record)) => {
                if let Err(error) = write_pending_gap(&mut *sink, &dropped) {
                    eprintln!("monitor: 写监控失败，停止记录：{error}");
                    break;
                }
                if let Err(error) = write_record(&mut *sink, &record) {
                    eprintln!("monitor: 写监控失败，停止记录：{error}");
                    break;
                }
            }
            Ok(Message::Shutdown) => {
                if let Err(error) = write_pending_gap(&mut *sink, &dropped) {
                    eprintln!("monitor: 写监控失败，停止记录：{error}");
                    break;
                }
                let shutdown = Record::ops("shutdown", Severity::Info, json!({}));
                if let Err(error) = write_record(&mut *sink, &shutdown) {
                    eprintln!("monitor: 写监控失败，停止记录：{error}");
                }
                break;
            }
            Err(_) => {
                let _ = write_pending_gap(&mut *sink, &dropped);
                break;
            }
        }
    }
}

/// 一个进程的监控记录器（Clone 共享同一个队列；只有持 join 的那一份负责退出排空）。
struct Shared {
    tx: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
    path: PathBuf,
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
        let join = std::thread::Builder::new()
            .name("aios-monitor".to_owned())
            .spawn({
                let dropped = Arc::clone(&dropped);
                move || run_writer(Box::new(FileSink { file }), rx, dropped)
            })
            .map_err(MonitorError::Io)?;
        Ok(Self {
            shared: Arc::new(Shared { tx, dropped, path }),
            join: Mutex::new(Some(join)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.shared.path
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

/// T011.3：脱敏规则（只改副本；key 不保存，只在匹配时用）。
#[derive(Debug, Clone)]
pub struct Redactor {
    secrets: Arc<Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum RedactError {
    #[error("监控记录嵌套过深，脱敏放弃")]
    TooDeep,
}

impl Redactor {
    /// secrets：进程持有的 key（运行时从 Keyring 取，监控不落盘）。
    pub fn new(secrets: Vec<String>) -> Self {
        Self {
            secrets: Arc::new(secrets),
        }
    }

    /// 推理帧/思考块文本 → [推理已略去，N 字]；任意字符串里的 key → [已脱敏]。
    /// 只改副本；出错（嵌套过深）时由调用方扣下该条。
    pub fn redact(&self, record: &mut Record) -> Result<(), RedactError> {
        redact_value(&mut record.body, &self.secrets, 0)?;
        for value in record.attributes.values_mut() {
            redact_value(value, &self.secrets, 0)?;
        }
        if let Some(source) = &mut record.source_event {
            redact_value(&mut source.event, &self.secrets, 0)?;
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

/// T011.3：装 ledger 时同时带脱敏；脱敏出错的记录扣下，另记一条 redact-failed。
pub fn install_ledger_with_redactor(
    monitor: &Monitor,
    store: &mut session::SessionStore,
    redactor: Redactor,
) {
    let session_id = store.id().as_str().to_owned();
    let monitor = monitor.clone();
    let hook: session::AppendHook = Arc::new(move |events: &[session::Event]| {
        for event in events {
            let mut record = ledger_record(&session_id, event);
            match redactor.redact(&mut record) {
                Ok(()) => {
                    let _ = monitor.record(record);
                }
                Err(_) => {
                    let event_type = serde_json::to_value(event)
                        .ok()
                        .and_then(|value| value.get("type").cloned())
                        .unwrap_or(Value::Null);
                    let mut attributes = Map::new();
                    attributes.insert("event.type".to_owned(), event_type);
                    attributes.insert("event.seq".to_owned(), json!(event.seq));
                    let _ = monitor.record(Record {
                        channel: Channel::Ops,
                        time: event.time,
                        severity: Severity::Warn,
                        attributes,
                        body: json!({}),
                        source_event: None,
                    });
                }
            }
        }
    });
    store.set_append_hook(Some(hook));
}

/// T011.2：把监控装到会话的「追加之后」钩子上。
pub fn install_ledger(monitor: &Monitor, store: &mut session::SessionStore) {
    let session_id = store.id().as_str().to_owned();
    let monitor = monitor.clone();
    let hook: session::AppendHook = Arc::new(move |events: &[session::Event]| {
        for event in events {
            let _ = monitor.record(ledger_record(&session_id, event));
        }
    });
    store.set_append_hook(Some(hook));
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
        write_pending_gap(&mut sink, &dropped).expect("写 gap");
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
            run_writer(Box::new(sink), rx, Arc::new(AtomicU64::new(0)));
        });
        handle.join().expect("写线程应停下而不是 panic");
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "写失败后不再尝试第二条");
    }
}
