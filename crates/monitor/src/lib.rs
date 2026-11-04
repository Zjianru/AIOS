//! T011.1：本机监控记录的形状与写线程（照 DSH session-telemetry 的 record 形状）。
//!
//! 每个进程一个 JSONL 文件：<数据目录>/monitor/<启动时间>-<pid>.jsonl。
//! 入队不阻塞；队列满丢新来的并计数，下一条写入前先写一条 gap；写线程每条 flush。
//! 写失败时写线程停掉、stderr 报一次；业务不受影响。退出时写 shutdown 并排空。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
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

/// 一个进程的监控记录器。
pub struct Monitor {
    tx: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
    join: Option<JoinHandle<()>>,
    path: PathBuf,
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
            tx,
            dropped,
            join: Some(join),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 入队；队列满时丢新来的、计数并返回 false（业务不受影响）。
    pub fn record(&self, record: Record) -> bool {
        match self.tx.try_send(Message::Record(record)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::SeqCst);
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
        if let Some(join) = self.join.take() {
            let _ = self.tx.send(Message::Shutdown);
            let _ = join.join();
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop();
    }
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
