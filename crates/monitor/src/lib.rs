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
use std::sync::Arc;
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

// T011.1 骨架：写线程行为待实现（红测试后补）。
fn write_record(_sink: &mut dyn LineSink, _record: &Record) -> Result<(), MonitorError> {
    Ok(())
}

fn write_pending_gap(
    _sink: &mut dyn LineSink,
    _dropped: &AtomicU64,
) -> Result<(), MonitorError> {
    Ok(())
}

fn run_writer(_sink: Box<dyn LineSink>, _rx: Receiver<Message>, _dropped: Arc<AtomicU64>) {}

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
