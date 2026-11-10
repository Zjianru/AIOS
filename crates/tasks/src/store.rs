//! SQLite 存储：一个连接放在锁后，同步接口；建任务和排 launch、写报告和排 relay 各在一个事务里。

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

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
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
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

    fn next_seq(&self) -> i64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 建任务并在同一个事务里排 launch。
    pub fn create_task(&self, draft: NewTask, now: i64) -> Result<Task, TaskError> {
        draft.validate()?;
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = Task {
            id: TaskId::from(generate_id(&transaction)?),
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
        transaction.execute(
            "INSERT INTO task (id, value) VALUES (?1, ?2)",
            params![task.id.as_str(), serde_json::to_string(&task)?],
        )?;
        self.enqueue_effect_in(
            &transaction,
            &EffectPayload::Launch {
                task_id: task.id.clone(),
            },
            now,
        )?;
        transaction.commit()?;
        Ok(task)
    }

    /// 读一个任务。
    pub fn task(&self, id: &TaskId) -> Result<Option<Task>, TaskError> {
        let conn = self.lock()?;
        load_task(&conn, id)
    }

    /// 排一条效果；同一个键还在等的时候合并，保留原来的 seq。
    pub fn enqueue_effect(&self, payload: EffectPayload, now: i64) -> Result<(), TaskError> {
        let conn = self.lock()?;
        self.enqueue_effect_in(&conn, &payload, now)
    }

    fn enqueue_effect_in(
        &self,
        conn: &Connection,
        payload: &EffectPayload,
        now: i64,
    ) -> Result<(), TaskError> {
        let seq = self.next_seq();
        conn.execute(
            "INSERT INTO effect (kind, key, value, seq, due, since) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(kind, key) DO UPDATE SET
                 value = excluded.value,
                 generation = generation + 1,
                 attempts = 0,
                 error = '',
                 due = excluded.due",
            params![
                payload.kind().as_str(),
                payload.key(),
                serde_json::to_string(payload)?,
                seq,
                now,
                now
            ],
        )?;
        Ok(())
    }

    /// 领一条到期的效果。
    pub fn claim_effect(
        &self,
        kind: EffectKind,
        now: i64,
    ) -> Result<Option<EffectClaim>, TaskError> {
        let conn = self.lock()?;
        claim_effect_in(&conn, kind, now)
    }

    /// 这个键的效果还在不在排队。
    pub fn effect_pending(&self, kind: EffectKind, key: &str) -> Result<bool, TaskError> {
        let conn = self.lock()?;
        effect_pending_in(&conn, kind, key)
    }
}

fn generate_id(connection: &Connection) -> Result<String, TaskError> {
    let id = connection.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
    Ok(id)
}

fn load_task(connection: &Connection, id: &TaskId) -> Result<Option<Task>, TaskError> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM task WHERE id = ?1",
            params![id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    match value {
        Some(value) => Ok(Some(serde_json::from_str(&value)?)),
        None => Ok(None),
    }
}

fn claim_effect_in(
    connection: &Connection,
    kind: EffectKind,
    now: i64,
) -> Result<Option<EffectClaim>, TaskError> {
    let row = connection
        .query_row(
            "SELECT key, value, seq, generation, attempts, due, since
             FROM effect WHERE kind = ?1 AND due <= ?2 ORDER BY seq LIMIT 1",
            params![kind.as_str(), now],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((key, value, seq, generation, attempts, due, since)) = row else {
        return Ok(None);
    };
    let payload: EffectPayload = serde_json::from_str(&value)?;
    if payload.kind() != kind || payload.key() != key {
        return Err(TaskError::CorruptEffect {
            kind: kind.as_str(),
            key,
        });
    }
    Ok(Some(EffectClaim {
        payload,
        key,
        seq,
        generation,
        attempts: attempts as u32,
        due,
        since,
    }))
}

fn effect_pending_in(
    connection: &Connection,
    kind: EffectKind,
    key: &str,
) -> Result<bool, TaskError> {
    let pending: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM effect WHERE kind = ?1 AND key = ?2)",
        params![kind.as_str(), key],
        |row| row.get(0),
    )?;
    Ok(pending)
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
