//! 会话日志的 JSONL 存储：一个会话一个只追加的文件。
//!
//! 机制照 DSH 的 session-persistence-jsonl（strategy 第 1 节），格式是本仓库
//! 自己的 v1：首行 header；每批事件先写入再 fsync，写失败回滚到写之前的长度；
//! 读的时候丢掉不完整的最后一行。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use model::Message;

use crate::derive::{derive_messages, derive_messages_with_seqs};
use crate::error::SessionError;
use crate::event::{Event, EventKind, FORMAT_VERSION, SessionHeader};
use crate::id::SessionId;

/// 追加写日志的底层文件接缝。生产实现是 std::fs::File；单元测试用第二个
/// 实现注入写入/fsync 失败，验证「写失败回滚到写之前的长度」。
trait LogWriter: Send {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn sync_data(&mut self) -> std::io::Result<()>;
    fn set_len(&mut self, len: u64) -> std::io::Result<()>;
}

impl LogWriter for File {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        Write::write_all(self, bytes)
    }

    fn sync_data(&mut self) -> std::io::Result<()> {
        File::sync_data(self)
    }

    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        File::set_len(self, len)
    }
}

/// 一个会话的只追加 JSONL 事件日志。
pub struct SessionStore {
    header: SessionHeader,
    path: PathBuf,
    events: Vec<Event>,
    bytes: u64,
    writer: Box<dyn LogWriter>,
}

impl SessionStore {
    /// 新建会话：生成 id、写 header（首行）并 fsync。
    pub fn create(data_dir: &Path, cwd: &Path) -> Result<Self, SessionError> {
        let cwd = fs::canonicalize(cwd)?;
        let cwd_text = cwd.to_str().ok_or_else(|| {
            SessionError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "工作目录不是合法 UTF-8",
            ))
        })?;
        let id = SessionId::generate();
        let path = log_path(data_dir, &id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let header = SessionHeader::new(id, cwd_text.to_owned(), now_ms());
        let header_line = format!("{}\n", serde_json::to_string(&header)?);
        let mut file = OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(&path)?;
        Write::write_all(&mut file, header_line.as_bytes())?;
        file.sync_data()?;
        Ok(Self {
            header,
            path,
            events: Vec::new(),
            bytes: header_line.len() as u64,
            writer: Box::new(file),
        })
    }

    /// 打开已有会话；最后一行不完整就丢掉（并把文件截回完整前缀）。
    pub fn open(data_dir: &Path, id: &SessionId) -> Result<Self, SessionError> {
        let path = log_path(data_dir, id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SessionError::NotFound(path.display().to_string())
            } else {
                SessionError::Io(error)
            }
        })?;
        let (header, events, committed) = parse_log(&bytes)?;
        if committed < bytes.len() as u64 {
            let file = OpenOptions::new().write(true).open(&path)?;
            file.set_len(committed)?;
            file.sync_data()?;
        }
        let file = OpenOptions::new().append(true).open(&path)?;
        Ok(Self {
            header,
            path,
            events,
            bytes: committed,
            writer: Box::new(file),
        })
    }

    pub fn id(&self) -> &SessionId {
        &self.header.id
    }

    /// header 里的工作目录；续接时用它（spec 实现决定）。
    pub fn cwd(&self) -> &str {
        &self.header.cwd
    }

    /// 这个会话的 JSONL 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// 从日志派生一次请求要带的消息。
    pub fn derive_messages(&self) -> Vec<Message> {
        derive_messages(&self.events)
    }

    /// 派生消息并带来源事件 seq（T005.8a 压缩选区间用）。
    pub fn derive_messages_with_seqs(&self) -> Vec<(u64, Message)> {
        derive_messages_with_seqs(&self.events)
    }

    /// 日志里是否已经有系统提示（一个会话只写一次）。
    pub fn has_system_message(&self) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event.kind, EventKind::SystemMessage { .. }))
    }

    /// 追加一批事件：先写入、再 fsync；任何一步失败都回滚到写之前的长度。
    pub fn append(&mut self, kinds: &[EventKind]) -> Result<Vec<u64>, SessionError> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let first_seq = self.events.len() as u64 + 1;
        let time = now_ms();
        let mut batch = Vec::with_capacity(kinds.len());
        let mut buffer = String::new();
        for (offset, kind) in kinds.iter().enumerate() {
            let event = Event {
                seq: first_seq + offset as u64,
                time,
                kind: kind.clone(),
            };
            buffer.push_str(&serde_json::to_string(&event)?);
            buffer.push('\n');
            batch.push(event);
        }
        let prior = self.bytes;
        let outcome = self
            .writer
            .write_all(buffer.as_bytes())
            .and_then(|()| self.writer.sync_data());
        if let Err(write_error) = outcome {
            return Err(match self.writer.set_len(prior) {
                Ok(()) => SessionError::Write {
                    source: write_error,
                },
                Err(rollback_error) => SessionError::WriteRollback {
                    write: write_error.to_string(),
                    rollback: rollback_error.to_string(),
                },
            });
        }
        self.bytes = prior + buffer.len() as u64;
        let seqs = batch.iter().map(|event| event.seq).collect();
        self.events.extend(batch);
        Ok(seqs)
    }
}

fn log_path(data_dir: &Path, id: &SessionId) -> PathBuf {
    data_dir.join("sessions").join(format!("{id}.jsonl"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 解析一份 JSONL 日志，返回 header、事件，以及完整前缀的字节长度。
fn parse_log(bytes: &[u8]) -> Result<(SessionHeader, Vec<Event>, u64), SessionError> {
    let Some(header_end) = bytes.iter().position(|byte| *byte == b'\n') else {
        return Err(SessionError::MissingHeader(
            "没有换行，读不到 header".to_owned(),
        ));
    };
    let header_text = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| SessionError::CorruptHeader("header 不是合法 UTF-8".to_owned()))?;
    let header: SessionHeader = serde_json::from_str(header_text)
        .map_err(|error| SessionError::CorruptHeader(error.to_string()))?;
    if header.version != FORMAT_VERSION {
        return Err(SessionError::UnsupportedVersion {
            version: header.version,
            supported: FORMAT_VERSION,
        });
    }
    header.validate()?;

    let rest = &bytes[header_end + 1..];
    let complete_len = if rest.is_empty() {
        0
    } else if rest.last() == Some(&b'\n') {
        rest.len()
    } else {
        rest.iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |position| position + 1)
    };
    let mut events = Vec::new();
    for (line_index, line) in rest[..complete_len]
        .split(|byte| *byte == b'\n')
        .enumerate()
    {
        if line.is_empty() {
            continue;
        }
        let line_number = line_index + 1;
        let text = std::str::from_utf8(line).map_err(|_| SessionError::CorruptLine {
            line: line_number,
            detail: "不是合法 UTF-8".to_owned(),
        })?;
        let event: Event =
            serde_json::from_str(text).map_err(|error| SessionError::CorruptLine {
                line: line_number,
                detail: error.to_string(),
            })?;
        let expected = events.len() as u64 + 1;
        if event.seq != expected {
            return Err(SessionError::SeqDiscontinuity {
                expected,
                actual: event.seq,
            });
        }
        events.push(event);
    }
    Ok((header, events, (header_end + 1 + complete_len) as u64))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// 生产实现之外的第二个 LogWriter：让 fsync 失败，验证回滚。
    struct FailingWriter {
        file: File,
        fail_sync: Arc<AtomicBool>,
    }

    impl LogWriter for FailingWriter {
        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            Write::write_all(&mut self.file, bytes)
        }

        fn sync_data(&mut self) -> std::io::Result<()> {
            if self.fail_sync.load(Ordering::SeqCst) {
                Err(std::io::Error::other("注入的 fsync 失败"))
            } else {
                File::sync_data(&self.file)
            }
        }

        fn set_len(&mut self, len: u64) -> std::io::Result<()> {
            File::set_len(&self.file, len)
        }
    }

    #[test]
    fn failed_append_rolls_back_to_previous_length() {
        let data_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
        let id = store.id().clone();
        store.append(&[EventKind::TurnStart { turn: 1 }]).unwrap();
        let path = store.path().to_path_buf();
        let length_after_first = fs::metadata(&path).unwrap().len();

        let fail_sync = Arc::new(AtomicBool::new(true));
        let file = OpenOptions::new().append(true).open(&path).unwrap();
        store.writer = Box::new(FailingWriter {
            file,
            fail_sync: Arc::clone(&fail_sync),
        });

        let error = store
            .append(&[EventKind::StepStart { turn: 1, step: 1 }])
            .unwrap_err();
        assert!(
            matches!(error, SessionError::Write { .. }),
            "期望写入错误，实际 {error}"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            length_after_first,
            "写失败后文件长度要回滚到写之前"
        );
        assert_eq!(store.events().len(), 1, "失败的事件不能进内存");

        fail_sync.store(false, Ordering::SeqCst);
        let seqs = store
            .append(&[EventKind::StepStart { turn: 1, step: 1 }])
            .unwrap();
        assert_eq!(seqs, vec![2]);
        assert_eq!(store.events().len(), 2);
        drop(store);

        let reopened = SessionStore::open(data_dir.path(), &id).unwrap();
        assert_eq!(reopened.events().len(), 2);
        assert_eq!(reopened.events()[1].seq, 2);
    }
}
