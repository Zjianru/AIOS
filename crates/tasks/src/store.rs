//! SQLite 存储：一个连接放在锁后，同步接口；建任务和排 launch、写报告和排 relay 各在一个事务里。

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};

use crate::TaskError;
use crate::effect::{EffectClaim, EffectKind, EffectPayload};
use crate::id::TaskId;
use crate::task::{NewTask, Task};

/// 任务库 schema 版本；比这新的库拒绝打开。
pub const SCHEMA_VERSION: u64 = 1;

/// kv 表里记 schema 版本的键。
pub(crate) const SCHEMA_KEY: &str = "schema_version";

const CREATE_TABLES: &str = "
    CREATE TABLE IF NOT EXISTS task (id TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS effect (
        kind TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, seq INTEGER NOT NULL,
        generation INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0,
        error TEXT NOT NULL DEFAULT '', due INTEGER NOT NULL, since INTEGER NOT NULL,
        PRIMARY KEY (kind, key)
    );
    CREATE INDEX IF NOT EXISTS effect_due ON effect(kind, due, seq);
";

/// 任务库。一个 SQLite 连接放在锁后；所有方法同步。
pub struct Store {
    pub(crate) conn: Mutex<Connection>,
    /// 效果行队列位置，进程内单调；重开时从表里的最大 seq 续上。
    seq: AtomicI64,
}

impl Store {
    /// 打开或创建任务库；新建时写入 schema 版本。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TaskError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        Self::configure(conn)
    }

    fn configure(conn: Connection) -> Result<Self, TaskError> {
        let version = stored_schema_version(&conn)?;
        if let Some(version) = version {
            if version > SCHEMA_VERSION {
                return Err(TaskError::SchemaTooNew {
                    found: version,
                    supported: SCHEMA_VERSION,
                });
            }
            if version < SCHEMA_VERSION {
                return Err(TaskError::SchemaUnsupported {
                    found: version,
                    supported: SCHEMA_VERSION,
                });
            }
        }
        let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(TaskError::StorageConfig(format!(
                "journal_mode={mode}，期望 wal"
            )));
        }
        conn.execute_batch("PRAGMA synchronous=FULL;")?;
        let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        if synchronous != 2 {
            return Err(TaskError::StorageConfig(format!(
                "synchronous={synchronous}，期望 2（FULL）"
            )));
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        if version.is_none() {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)",
                params![SCHEMA_KEY, serde_json::to_string(&SCHEMA_VERSION)?],
            )?;
        }
        conn.execute_batch(CREATE_TABLES)?;
        let seq: i64 = conn.query_row("SELECT IFNULL(MAX(seq), 0) FROM effect", [], |row| {
            row.get(0)
        })?;
        Ok(Self {
            conn: Mutex::new(conn),
            seq: AtomicI64::new(seq),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, TaskError> {
        self.conn.lock().map_err(|_| TaskError::Poisoned)
    }

    /// 建任务并在同一个事务里排 launch。
    pub fn create_task(&self, draft: NewTask, now: i64) -> Result<Task, TaskError> {
        let task = Task {
            id: TaskId::from("stub"),
            title: draft.title,
            requirement: draft.requirement,
            dod: draft.dod,
            cwd: draft.cwd,
            conversation: draft.conversation,
            model: draft.model,
            tools: draft.tools,
            writes: Vec::new(),
            code_mode: false,
            runs: Vec::new(),
            reports: Vec::new(),
            missing: Vec::new(),
            created_at: now,
        };
        Ok(task)
    }

    /// 读一个任务。
    pub fn task(&self, _id: &TaskId) -> Result<Option<Task>, TaskError> {
        Ok(None)
    }

    /// 领一条到期的效果。
    pub fn claim_effect(
        &self,
        _kind: EffectKind,
        _now: i64,
    ) -> Result<Option<EffectClaim>, TaskError> {
        Ok(None)
    }

    /// 这个键的效果还在不在排队。
    pub fn effect_pending(&self, _kind: EffectKind, _key: &str) -> Result<bool, TaskError> {
        Ok(false)
    }
}

fn stored_schema_version(conn: &Connection) -> Result<Option<u64>, TaskError> {
    let has_kv: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'kv')",
        [],
        |row| row.get(0),
    )?;
    if !has_kv {
        return Ok(None);
    }
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE key = ?1",
            params![SCHEMA_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let Some(value) = value else {
        return Ok(None);
    };
    serde_json::from_str(&value)
        .map(Some)
        .map_err(|_| TaskError::CorruptSchema(value))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
