//! 会话日志的 JSONL 存储：一个会话一个只追加的文件。
//!
//! 机制照 DSH 的 session-persistence-jsonl（strategy 第 1 节），格式是本仓库
//! 自己的 v2：首行 header；事件记录是 `{type, seq, time, data}`；每批事件先写入再
//! fsync，写失败回滚到写之前的长度。尾行不完整时只从读到的视图里丢掉：只读打开
//! 不改文件，写句柄在第一次 append 之前才把未提交的尾部截掉（照 DSH）。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use model::Message;

use crate::derive::{derive_messages, derive_messages_with_seqs};
use crate::error::SessionError;
use crate::event::{Event, EventKind, FORMAT_VERSION, SessionHeader, SurfaceOp};
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
    /// 文件里还有未提交的尾部；第一次 append 之前先截到 bytes（照 DSH 的写句柄语义）。
    pending_truncate: bool,
    writer: Box<dyn LogWriter>,
    /// 单写者锁（T006.5b）：跟随本 store 的生命周期；drop 即释放。
    _writer_lock: crate::lock::WriterLock,
}

/// 只读打开得到的会话日志视图：读到最后一个完整事件为止，不改文件。
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    id: crate::SessionId,
    path: PathBuf,
    events: Vec<Event>,
}

impl SessionSnapshot {
    pub fn id(&self) -> &crate::SessionId {
        &self.id
    }

    /// 这个会话的 JSONL 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }
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
        // T006.5b：创建会话就等于物化写句柄，先取单写者锁再落文件。
        let writer_lock = crate::lock::WriterLock::acquire(data_dir, &id)?;
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
            pending_truncate: false,
            writer: Box::new(file),
            _writer_lock: writer_lock,
        })
    }

    /// 打开已有会话准备写。尾行不完整时只从读到的视图里丢掉，文件要等到第一次
    /// append 之前才截（照 DSH：读打开不改文件，写句柄在首次追加前修掉 torn tail）。
    pub fn open(data_dir: &Path, id: &SessionId) -> Result<Self, SessionError> {
        // T006.5b：写打开先取非阻塞排他锁；被活写者占用立即报 AlreadyOwned（照 DSH）。
        let writer_lock = crate::lock::WriterLock::acquire(data_dir, id)?;
        let path = log_path(data_dir, id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SessionError::NotFound(path.display().to_string())
            } else {
                SessionError::Io(error)
            }
        })?;
        let (header, events, committed) = parse_log(&bytes)?;
        let pending_truncate = committed < bytes.len() as u64;
        let file = OpenOptions::new().append(true).open(&path)?;
        Ok(Self {
            header,
            path,
            events,
            bytes: committed,
            pending_truncate,
            writer: Box::new(file),
            _writer_lock: writer_lock,
        })
    }

    /// 只读打开一个已有会话：读到最后一个完整事件为止，不改文件。
    ///
    /// 尾行不完整时只从视图里丢掉，不截文件；要写的话另开 write 句柄（照 DSH
    /// 的 open(id, 'read'|'write')，persistence-jsonl README:77,87）。
    pub fn open_read_only(
        data_dir: &Path,
        id: &SessionId,
    ) -> Result<SessionSnapshot, SessionError> {
        let path = log_path(data_dir, id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SessionError::NotFound(path.display().to_string())
            } else {
                SessionError::Io(error)
            }
        })?;
        let (header, events, _committed) = parse_log(&bytes)?;
        Ok(SessionSnapshot {
            id: header.id,
            path,
            events,
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
        self.append_with_time(kinds, now_ms())
    }

    /// 按给定的 time 追加一批事件（seq 仍由 store 连续分配）。
    ///
    /// 中断修复的事件复用最后一个真实事件的时间（照 DSH repair.ts:88-96）。
    pub fn append_with_time(
        &mut self,
        kinds: &[EventKind],
        time: u64,
    ) -> Result<Vec<u64>, SessionError> {
        let entries: Vec<(EventKind, Option<SurfaceOp>, Option<Vec<u64>>)> = kinds
            .iter()
            .map(|kind| (kind.clone(), None, None))
            .collect();
        self.append_entries(entries, time)
    }

    /// 追加一条 message-producing 事件并带上 surface 放置信息（T006.3 压缩 checkpoint 的
    /// `surfaceOp: replace` + `sourceEventSeqs`，照 DSH SessionEvent）。
    pub fn append_surface(
        &mut self,
        kind: EventKind,
        surface_op: SurfaceOp,
        source_event_seqs: Vec<u64>,
    ) -> Result<Vec<u64>, SessionError> {
        self.append_entries(
            vec![(kind, Some(surface_op), Some(source_event_seqs))],
            now_ms(),
        )
    }

    fn append_entries(
        &mut self,
        entries: Vec<(EventKind, Option<SurfaceOp>, Option<Vec<u64>>)>,
        time: u64,
    ) -> Result<Vec<u64>, SessionError> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        // 写句柄的第一次 append 之前，先把上次没提交的尾部截掉（DSH storage.ts:317-334）。
        if self.pending_truncate {
            self.writer
                .set_len(self.bytes)
                .map_err(|source| SessionError::Truncate { source })?;
            self.pending_truncate = false;
        }
        let first_seq = self.events.len() as u64 + 1;
        let mut batch = Vec::with_capacity(entries.len());
        let mut buffer = String::new();
        for (offset, (kind, surface_op, source_event_seqs)) in entries.into_iter().enumerate() {
            let event = Event {
                seq: first_seq + offset as u64,
                time,
                kind,
                surface_op,
                source_event_seqs,
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
    use model::Role;
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

    #[test]
    fn question_events_are_ignored_by_derive_and_repair() {
        let at = |seq: u64, kind: EventKind| Event::new(seq, 0, kind);
        let message = Message::text(Role::User, "go");
        let events = vec![
            at(1, EventKind::TurnStart { turn: 1 }),
            at(
                2,
                EventKind::UserMessage {
                    turn: 1,
                    step: 1,
                    message: message.clone(),
                },
            ),
            at(
                3,
                EventKind::QuestionAsked {
                    turn: 1,
                    step: 1,
                    question_id: "q1".to_owned(),
                    question: "Proceed?".to_owned(),
                    header: Some("Confirm".to_owned()),
                    options: vec!["Yes".to_owned()],
                    multi_select: false,
                },
            ),
            at(
                4,
                EventKind::QuestionCancelled {
                    turn: 1,
                    step: 1,
                    question_id: "q1".to_owned(),
                },
            ),
        ];
        // derive：question 事件不进派生请求（只有那条 user/message）。
        let derived = crate::derive::derive_messages(&events);
        assert_eq!(derived.len(), 1, "question 事件不能变成请求消息");
        // repair：忽略 question 事件，收尾 closers 与没有它们时完全一致。
        let base = vec![
            at(1, EventKind::TurnStart { turn: 1 }),
            at(2, EventKind::StepStart { turn: 1, step: 1 }),
        ];
        let with_questions = vec![
            at(1, EventKind::TurnStart { turn: 1 }),
            at(2, EventKind::StepStart { turn: 1, step: 1 }),
            at(
                3,
                EventKind::QuestionAsked {
                    turn: 1,
                    step: 1,
                    question_id: "q1".to_owned(),
                    question: "Proceed?".to_owned(),
                    header: None,
                    options: Vec::new(),
                    multi_select: false,
                },
            ),
            at(
                4,
                EventKind::QuestionCancelled {
                    turn: 1,
                    step: 1,
                    question_id: "q1".to_owned(),
                },
            ),
        ];
        assert_eq!(
            crate::repair::interrupted_turn_closers(&with_questions).map(|closers| closers.events),
            crate::repair::interrupted_turn_closers(&base).map(|closers| closers.events),
            "repair 必须忽略 question 事件"
        );
    }
}
