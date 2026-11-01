//! 会话单写者锁（T006.5b，照 DSH session-persistence-jsonl 的 session.lock）。
//!
//! DSH 依据：packages/session/session-persistence-jsonl/README.md「One live writer per
//! session」：内核非阻塞 flock(2) 排他锁；崩溃的持有者随进程退出自动释放；释放不删锁文件。
//! AIOS 位置：<data-dir>/sessions/<id>.lock。

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use crate::{SessionError, SessionId};

/// 写会话持有的排他锁；随 File 句柄释放（drop / panic / 进程退出都由内核释放）。
#[allow(dead_code)] // 骨架期先不接进 SessionStore，实现期接
pub(crate) struct WriterLock {
    /// 锁句柄保持打开才持有锁；drop 即释放。文件名/路径见 path()。
    _file: File,
    path: PathBuf,
}

#[allow(dead_code)]
impl WriterLock {
    /// 非阻塞取锁：被另一个活写者占用时立刻报 AlreadyOwned（服务层据此回报占用）。
    pub(crate) fn acquire(data_dir: &Path, id: &SessionId) -> Result<Self, SessionError> {
        let path = lock_path(data_dir, id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(TryLockError::WouldBlock) => Err(SessionError::AlreadyOwned {
                id: id.to_string(),
                path: path.display().to_string(),
            }),
            Err(TryLockError::Error(error)) => Err(SessionError::Io(error)),
        }
    }

    /// 锁文件路径（<data-dir>/sessions/<id>.lock）。
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// 会话锁文件路径。
pub(crate) fn lock_path(data_dir: &Path, id: &SessionId) -> PathBuf {
    data_dir.join("sessions").join(format!("{id}.lock"))
}
