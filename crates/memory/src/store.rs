//! 记忆库：一个 SQLite 连接放在锁后；修订只追加，FTS5 trigram 做全文搜索。
//!
//! 对外三个入口：open、view、search、write。schema、FTS 同步、actor 校验都藏在这里。

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::MemoryError;
use crate::types::{
    Actor, Binding, DiaryEntry, DreamAction, DreamChangeInput, Layer, MemoryItem, MemoryView,
    RevisionItem, Scope, SearchHit, WriteContext, WriteOp, WriteOutcome,
};

pub const SCHEMA_VERSION: u64 = 1;
const SCHEMA_KEY: &str = "schema_version";
const VERSION_KEY: &str = "version";
const QUARANTINE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

const CREATE_TABLES: &str = "
    CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS memory (
        id TEXT PRIMARY KEY,
        layer TEXT NOT NULL,
        binding TEXT NOT NULL,
        scope TEXT NOT NULL,
        project_id TEXT,
        role_id TEXT,
        title TEXT NOT NULL,
        summary TEXT NOT NULL,
        body TEXT NOT NULL,
        source_kind TEXT NOT NULL,
        source_ref TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        deleted_at INTEGER,
        purge_after INTEGER,
        status TEXT NOT NULL,
        current_revision INTEGER NOT NULL,
        pinned INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS memory_layer ON memory(layer, status);
    CREATE TABLE IF NOT EXISTS memory_revision (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        memory_id TEXT NOT NULL,
        revision INTEGER NOT NULL,
        title TEXT NOT NULL,
        summary TEXT NOT NULL,
        body TEXT NOT NULL,
        source_kind TEXT NOT NULL,
        source_ref TEXT NOT NULL,
        reason TEXT NOT NULL,
        dream_run_id TEXT,
        undo_of INTEGER,
        created_at INTEGER NOT NULL,
        UNIQUE(memory_id, revision)
    );
    CREATE TABLE IF NOT EXISTS diary_entry (
        id TEXT PRIMARY KEY,
        day TEXT NOT NULL,
        seq INTEGER NOT NULL,
        text TEXT NOT NULL,
        source_ref TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS dream_run (
        id TEXT PRIMARY KEY,
        started_at INTEGER NOT NULL,
        ended_at INTEGER NOT NULL,
        outcome TEXT NOT NULL,
        model TEXT NOT NULL,
        usage_json TEXT NOT NULL,
        report_json TEXT NOT NULL,
        error TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE IF NOT EXISTS dream_change (
        run_id TEXT NOT NULL,
        memory_id TEXT NOT NULL,
        before_revision INTEGER NOT NULL,
        after_revision INTEGER NOT NULL,
        action TEXT NOT NULL,
        reason TEXT NOT NULL,
        PRIMARY KEY (run_id, memory_id)
    );
    CREATE TABLE IF NOT EXISTS quarantine (
        memory_id TEXT PRIMARY KEY,
        deleted_at INTEGER NOT NULL,
        purge_after INTEGER NOT NULL,
        reason TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS tombstone (
        source_hash TEXT PRIMARY KEY,
        at INTEGER NOT NULL
    );
    CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
        kind, memory_id UNINDEXED, title, body, source_json UNINDEXED,
        tokenize='trigram'
    );
    CREATE VIRTUAL TABLE IF NOT EXISTS conversation_fts USING fts5(
        conversation_id UNINDEXED, seq UNINDEXED, turn UNINDEXED, text, source_json UNINDEXED,
        tokenize='trigram'
    );
";

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// 记忆库。一个连接放在锁后；同步接口，事务里的写要么全成要么全不成。
pub struct MemoryStore {
    conn: Mutex<Connection>,
    clock: Clock,
}

impl MemoryStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        Self::open_with_clock(path, Arc::new(now_millis))
    }

    /// 测试用的假时钟入口；生产走 open。
    pub fn open_with_clock(path: impl AsRef<Path>, clock: Clock) -> Result<Self, MemoryError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let version = stored_schema_version(&conn)?;
        if let Some(version) = version
            && version > SCHEMA_VERSION
        {
            return Err(MemoryError::SchemaTooNew {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if let Some(version) = version
            && version < 1
        {
            return Err(MemoryError::SchemaUnsupported {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(MemoryError::Sqlite)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(MemoryError::StorageConfig(format!(
                "journal_mode={mode}，期望 wal"
            )));
        }
        conn.execute_batch("PRAGMA synchronous=FULL;")?;
        conn.execute_batch(CREATE_TABLES)?;
        if version.is_none() {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)",
                params![SCHEMA_KEY, SCHEMA_VERSION.to_string()],
            )?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO kv (key, value) VALUES (?1, '0')",
            params![VERSION_KEY],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            clock,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("记忆库锁被毒化")
    }

    pub fn version(&self) -> Result<u64, MemoryError> {
        let conn = self.lock();
        let value: String = conn.query_row(
            "SELECT value FROM kv WHERE key = ?1",
            params![VERSION_KEY],
            |row| row.get(0),
        )?;
        Ok(value.parse().unwrap_or(0))
    }

    /// 全量视图：关于你 / 条目 / 笔记 / 日记 / 隔离区 / 设置。
    pub fn view(&self) -> Result<MemoryView, MemoryError> {
        let conn = self.lock();
        let version = read_version(&conn)?;
        let about_you = items(
            &conn,
            "where layer = 'about_you' and status = 'active' order by updated_at desc",
        )?;
        let entries = items(
            &conn,
            "where layer = 'entry' and status = 'active' order by updated_at desc",
        )?;
        let notes = items(
            &conn,
            "where layer = 'note' and status = 'active' order by updated_at desc",
        )?;
        let quarantine = items(
            &conn,
            "where status = 'quarantine' order by deleted_at desc",
        )?;
        let diary = diary_entries(&conn)?;
        let settings = settings(&conn)?;
        Ok(MemoryView {
            version,
            about_you,
            entries,
            notes,
            diary,
            quarantine,
            settings,
        })
    }

    /// 搜索记忆、日记和 Kind 对话。不到 3 个字走 LIKE（trigram 不索引短词）。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MemoryError> {
        let query = query.trim();
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let limit = i64::try_from(limit).unwrap_or(50);
        let short = query.chars().count() < 3;
        if short {
            return search_like(&conn, query, limit);
        }
        search_fts(&conn, query, limit)
    }

    /// 读一条记忆（页面/工具按 id 读）。
    pub fn item(&self, id: &str) -> Result<Option<MemoryItem>, MemoryError> {
        let conn = self.lock();
        item_by_id(&conn, id)
    }

    /// 一条记忆的全部修订（从第 1 版起，只增不改）。
    pub fn revisions(&self, id: &str) -> Result<Vec<RevisionItem>, MemoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT memory_id, revision, title, summary, body, reason, dream_run_id, undo_of, created_at FROM memory_revision WHERE memory_id = ?1 ORDER BY revision",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(RevisionItem {
                memory_id: row.get(0)?,
                revision: row.get::<_, i64>(1)? as u64,
                title: row.get(2)?,
                summary: row.get(3)?,
                body: row.get(4)?,
                reason: row.get(5)?,
                dream_run_id: row.get(6)?,
                undo_of: row.get::<_, Option<i64>>(7)?.map(|value| value as u64),
                created_at: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::Sqlite)
    }

    /// 写一条：身份校验、修订追加、FTS 同步、版本递增都在一个事务里。
    pub fn write(&self, op: WriteOp, ctx: &WriteContext) -> Result<WriteOutcome, MemoryError> {
        check_actor(&op, ctx)?;
        let now = (self.clock)();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut bump = true;
        let outcome = match op {
            WriteOp::Remember {
                title,
                summary,
                body,
                source_kind,
                source_ref,
            } => {
                if tombstone_exists(&tx, &source_kind, &source_ref)? {
                    return Err(MemoryError::Refused("这个来源已经忘记过，不能再记"));
                }
                let item = create_item(
                    &tx,
                    Layer::AboutYou,
                    Binding::Binding,
                    Scope::Personal,
                    &title,
                    &summary,
                    &body,
                    &source_kind,
                    &source_ref,
                    now,
                    None,
                )?;
                WriteOutcome {
                    item: Some(item),
                    ..WriteOutcome::default()
                }
            }
            WriteOp::Note {
                title,
                summary,
                body,
                source_kind,
                source_ref,
            } => {
                if tombstone_exists(&tx, &source_kind, &source_ref)? {
                    return Err(MemoryError::Refused("这个来源已经忘记过，不能再记"));
                }
                let item = create_item(
                    &tx,
                    Layer::Note,
                    Binding::Fallible,
                    Scope::Personal,
                    &title,
                    &summary,
                    &body,
                    &source_kind,
                    &source_ref,
                    now,
                    None,
                )?;
                WriteOutcome {
                    item: Some(item),
                    ..WriteOutcome::default()
                }
            }
            WriteOp::Diary { text, source_ref } => {
                let diary = create_diary(&tx, &text, &source_ref, now)?;
                WriteOutcome {
                    diary: Some(diary),
                    ..WriteOutcome::default()
                }
            }
            WriteOp::Correct {
                id,
                title,
                summary,
                body,
                reason,
            } => {
                let item = correct_item(&tx, &id, &title, &summary, &body, &reason, now)?;
                WriteOutcome {
                    item: Some(item),
                    ..WriteOutcome::default()
                }
            }
            WriteOp::Delete { id, reason } => {
                delete_to_quarantine(&tx, &id, &reason, now)?;
                WriteOutcome::default()
            }
            WriteOp::Restore { id } => {
                let item = restore_item(&tx, &id, now)?;
                WriteOutcome {
                    item: Some(item),
                    ..WriteOutcome::default()
                }
            }
            WriteOp::Forget { id } => {
                forget_item(&tx, &id, now)?;
                WriteOutcome::default()
            }
            WriteOp::PurgeExpired => {
                let purged = purge_expired(&tx, now)?;
                bump = false;
                WriteOutcome {
                    purged,
                    ..WriteOutcome::default()
                }
            }
            WriteOp::Pin { id } => {
                let item = pin_item(&tx, &id, now)?;
                WriteOutcome {
                    item: Some(item),
                    ..WriteOutcome::default()
                }
            }
            WriteOp::DreamApply { run, changes } => dream_apply(&tx, &run, &changes, now)?,
            WriteOp::DreamUndo { run_id, only } => dream_undo(&tx, &run_id, only.as_deref(), now)?,
            WriteOp::IndexConversation {
                conversation_id,
                seq,
                turn,
                text,
            } => {
                index_conversation(&tx, &conversation_id, seq, turn, &text)?;
                bump = false;
                WriteOutcome::default()
            }
        };
        if bump {
            bump_version(&tx)?;
        }
        tx.commit()?;
        Ok(outcome)
    }

    /// 隔离区到期清理；启动/页面打开/做梦前顺手调。
    pub fn purge_expired(&self) -> Result<usize, MemoryError> {
        let now = (self.clock)();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let purged = purge_expired(&tx, now)?;
        tx.commit()?;
        Ok(purged)
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn stored_schema_version(conn: &Connection) -> Result<Option<u64>, MemoryError> {
    let table: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'kv'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if table.is_none() {
        return Ok(None);
    }
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE key = ?1",
            params![SCHEMA_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.and_then(|value| value.parse().ok()))
}

fn check_actor(op: &WriteOp, ctx: &WriteContext) -> Result<(), MemoryError> {
    let allowed = match ctx.actor {
        Actor::User => true,
        Actor::Kind => matches!(
            op,
            WriteOp::Note { .. } | WriteOp::Diary { .. } | WriteOp::IndexConversation { .. }
        ),
        Actor::Dream => matches!(
            op,
            WriteOp::DreamApply { .. } | WriteOp::DreamUndo { .. } | WriteOp::PurgeExpired
        ),
    };
    if !allowed {
        return Err(MemoryError::Refused(match ctx.actor {
            Actor::Kind => "Kind 只能记笔记和日记",
            Actor::Dream => "做梦只能写笔记",
            Actor::User => "用户身份被拒",
        }));
    }
    if matches!(op, WriteOp::DreamApply { .. } | WriteOp::DreamUndo { .. })
        && ctx.actor == Actor::Dream
        && ctx.dream_run.is_none()
    {
        return Err(MemoryError::Refused("做梦写入必须带 dream_run"));
    }
    Ok(())
}

fn read_version(conn: &Connection) -> Result<u64, MemoryError> {
    let value: String = conn.query_row(
        "SELECT value FROM kv WHERE key = ?1",
        params![VERSION_KEY],
        |row| row.get(0),
    )?;
    Ok(value.parse().unwrap_or(0))
}

fn bump_version(tx: &rusqlite::Transaction<'_>) -> Result<(), MemoryError> {
    let value: String = tx.query_row(
        "SELECT value FROM kv WHERE key = ?1",
        params![VERSION_KEY],
        |row| row.get(0),
    )?;
    let next = value.parse::<u64>().unwrap_or(0) + 1;
    tx.execute(
        "UPDATE kv SET value = ?1 WHERE key = ?2",
        params![next.to_string(), VERSION_KEY],
    )?;
    Ok(())
}

fn items(conn: &Connection, where_clause: &str) -> Result<Vec<MemoryItem>, MemoryError> {
    let sql = format!(
        "SELECT id, layer, binding, scope, title, summary, body, source_kind, source_ref, status, current_revision, updated_at, pinned FROM memory {where_clause}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], item_from_row)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(MemoryError::Sqlite)
}

fn item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryItem> {
    let layer: String = row.get(1)?;
    let binding: String = row.get(2)?;
    let scope: String = row.get(3)?;
    Ok(MemoryItem {
        id: row.get(0)?,
        layer: Layer::parse(&layer).unwrap_or(Layer::Note),
        binding: if binding == "binding" {
            Binding::Binding
        } else {
            Binding::Fallible
        },
        scope: match scope.as_str() {
            "project" => Scope::Project,
            "role" => Scope::Role,
            _ => Scope::Personal,
        },
        title: row.get(4)?,
        summary: row.get(5)?,
        body: row.get(6)?,
        source_kind: row.get(7)?,
        source_ref: row.get(8)?,
        status: row.get(9)?,
        current_revision: row.get::<_, i64>(10)? as u64,
        updated_at: row.get(11)?,
        pinned: row.get::<_, i64>(12)? != 0,
    })
}

fn item_by_id(conn: &Connection, id: &str) -> Result<Option<MemoryItem>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, layer, binding, scope, title, summary, body, source_kind, source_ref, status, current_revision, updated_at, pinned FROM memory WHERE id = ?1",
    )?;
    stmt.query_row(params![id], item_from_row)
        .optional()
        .map_err(MemoryError::Sqlite)
}

fn diary_entries(conn: &Connection) -> Result<Vec<DiaryEntry>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, day, seq, text, source_ref, created_at FROM diary_entry ORDER BY created_at asc",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DiaryEntry {
            id: row.get(0)?,
            day: row.get(1)?,
            seq: row.get::<_, i64>(2)? as u64,
            text: row.get(3)?,
            source_ref: row.get(4)?,
            created_at: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(MemoryError::Sqlite)
}

fn settings(conn: &Connection) -> Result<Vec<(String, String)>, MemoryError> {
    let mut stmt = conn.prepare("SELECT key, value FROM kv WHERE key != ?1 ORDER BY key")?;
    let rows = stmt.query_map(params![VERSION_KEY], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(MemoryError::Sqlite)
}

#[allow(clippy::too_many_arguments)]
fn create_item(
    tx: &rusqlite::Transaction<'_>,
    layer: Layer,
    binding: Binding,
    scope: Scope,
    title: &str,
    summary: &str,
    body: &str,
    source_kind: &str,
    source_ref: &str,
    now: i64,
    dream_run: Option<&str>,
) -> Result<MemoryItem, MemoryError> {
    let id = next_id("mem");
    tx.execute(
        "INSERT INTO memory (id, layer, binding, scope, title, summary, body, source_kind, source_ref, created_at, updated_at, status, current_revision) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, 'active', 1)",
        params![id, layer.as_str(), binding.as_str(), scope.as_str(), title, summary, body, source_kind, source_ref, now],
    )?;
    tx.execute(
        "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, dream_run_id, created_at) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, 'created', ?7, ?8)",
        params![id, title, summary, body, source_kind, source_ref, dream_run, now],
    )?;
    fts_put(tx, &id, layer, title, body)?;
    item_by_id(tx, &id)?.ok_or_else(|| MemoryError::NotFound(id))
}

fn fts_put(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    layer: Layer,
    title: &str,
    body: &str,
) -> Result<(), MemoryError> {
    tx.execute("DELETE FROM memory_fts WHERE memory_id = ?1", params![id])?;
    tx.execute(
        "INSERT INTO memory_fts (kind, memory_id, title, body, source_json) VALUES (?1, ?2, ?3, ?4, '{}')",
        params![layer.as_str(), id, title, body],
    )?;
    Ok(())
}

fn fts_delete(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<(), MemoryError> {
    tx.execute("DELETE FROM memory_fts WHERE memory_id = ?1", params![id])?;
    Ok(())
}

fn create_diary(
    tx: &rusqlite::Transaction<'_>,
    text: &str,
    source_ref: &str,
    now: i64,
) -> Result<DiaryEntry, MemoryError> {
    let id = next_id("diary");
    let seq: i64 = tx.query_row(
        "SELECT IFNULL(MAX(seq), 0) + 1 FROM diary_entry",
        [],
        |row| row.get(0),
    )?;
    let day = day_of(now);
    tx.execute(
        "INSERT INTO diary_entry (id, day, seq, text, source_ref, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, day, seq, text, source_ref, now],
    )?;
    tx.execute("DELETE FROM memory_fts WHERE memory_id = ?1", params![id])?;
    tx.execute(
        "INSERT INTO memory_fts (kind, memory_id, title, body, source_json) VALUES ('diary', ?1, '', ?2, '{}')",
        params![id, text],
    )?;
    Ok(DiaryEntry {
        id,
        day,
        seq: seq as u64,
        text: text.to_owned(),
        source_ref: source_ref.to_owned(),
        created_at: now,
    })
}

fn correct_item(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    title: &str,
    summary: &str,
    body: &str,
    reason: &str,
    now: i64,
) -> Result<MemoryItem, MemoryError> {
    let current = item_by_id(tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()))?;
    if current.status != "active" {
        return Err(MemoryError::Refused("只有 active 的记忆能更正"));
    }
    let revision = current.current_revision + 1;
    tx.execute(
        "UPDATE memory SET title = ?1, summary = ?2, body = ?3, updated_at = ?4, current_revision = ?5 WHERE id = ?6",
        params![title, summary, body, now, revision as i64, id],
    )?;
    tx.execute(
        "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![id, revision as i64, title, summary, body, current.source_kind, current.source_ref, reason, now],
    )?;
    fts_put(tx, id, current.layer, title, body)?;
    item_by_id(tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()))
}

fn delete_to_quarantine(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    reason: &str,
    now: i64,
) -> Result<(), MemoryError> {
    if item_by_id(tx, id)?.is_none() {
        return Err(MemoryError::NotFound(id.to_owned()));
    }
    let purge_after = now + QUARANTINE_MS;
    tx.execute(
        "UPDATE memory SET status = 'quarantine', deleted_at = ?1, purge_after = ?2 WHERE id = ?3",
        params![now, purge_after, id],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO quarantine (memory_id, deleted_at, purge_after, reason) VALUES (?1, ?2, ?3, ?4)",
        params![id, now, purge_after, reason],
    )?;
    fts_delete(tx, id)?;
    Ok(())
}

fn restore_item(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    now: i64,
) -> Result<MemoryItem, MemoryError> {
    let item = item_by_id(tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()))?;
    if item.status != "quarantine" {
        return Err(MemoryError::Refused("只有隔离区的记忆能恢复"));
    }
    tx.execute(
        "UPDATE memory SET status = 'active', deleted_at = NULL, purge_after = NULL, updated_at = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    tx.execute("DELETE FROM quarantine WHERE memory_id = ?1", params![id])?;
    fts_put(tx, id, item.layer, &item.title, &item.body)?;
    item_by_id(tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()))
}

fn forget_item(tx: &rusqlite::Transaction<'_>, id: &str, now: i64) -> Result<(), MemoryError> {
    let item = item_by_id(tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()))?;
    let source_hash = hash_source(&item.source_kind, &item.source_ref);
    tx.execute(
        "UPDATE memory SET title = '', summary = '', body = '', status = 'forgotten', deleted_at = ?1, purge_after = NULL WHERE id = ?2",
        params![now, id],
    )?;
    tx.execute(
        "UPDATE memory_revision SET title = '', summary = '', body = '' WHERE memory_id = ?1",
        params![id],
    )?;
    fts_delete(tx, id)?;
    tx.execute("DELETE FROM quarantine WHERE memory_id = ?1", params![id])?;
    tx.execute(
        "INSERT OR REPLACE INTO tombstone (source_hash, at) VALUES (?1, ?2)",
        params![source_hash, now],
    )?;
    Ok(())
}

fn purge_expired(tx: &rusqlite::Transaction<'_>, now: i64) -> Result<usize, MemoryError> {
    let ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT memory_id FROM quarantine WHERE purge_after <= ?1")?;
        let rows = stmt.query_map(params![now], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for id in &ids {
        tx.execute("DELETE FROM memory WHERE id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM memory_revision WHERE memory_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM memory_fts WHERE memory_id = ?1", params![id])?;
        tx.execute("DELETE FROM quarantine WHERE memory_id = ?1", params![id])?;
        tx.execute("DELETE FROM dream_change WHERE memory_id = ?1", params![id])?;
    }
    Ok(ids.len())
}

fn pin_item(tx: &rusqlite::Transaction<'_>, id: &str, now: i64) -> Result<MemoryItem, MemoryError> {
    let note = item_by_id(tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()))?;
    if note.layer != Layer::Note {
        return Err(MemoryError::Refused("只有 Kind 的笔记能定下来"));
    }
    tx.execute(
        "UPDATE memory SET status = 'pinned', pinned = 1 WHERE id = ?1",
        params![id],
    )?;
    create_item(
        tx,
        Layer::Entry,
        Binding::Binding,
        Scope::Personal,
        &note.title,
        &note.summary,
        &note.body,
        "note_pin",
        id,
        now,
        None,
    )
}

fn dream_apply(
    tx: &rusqlite::Transaction<'_>,
    run: &crate::types::DreamRunInput,
    changes: &[DreamChangeInput],
    now: i64,
) -> Result<WriteOutcome, MemoryError> {
    tx.execute(
        "INSERT OR REPLACE INTO dream_run (id, started_at, ended_at, outcome, model, usage_json, report_json, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '')",
        params![run.id, run.started_at, run.ended_at, run.outcome, run.model, run.usage_json, run.report_json],
    )?;
    let mut applied = 0;
    let mut last = None;
    for change in changes {
        let memory_id = match &change.memory_id {
            Some(id) => id.clone(),
            None => {
                let item = create_item(
                    tx,
                    Layer::Note,
                    Binding::Fallible,
                    Scope::Personal,
                    &change.title,
                    &change.summary,
                    &change.body,
                    "dream",
                    &run.id,
                    now,
                    Some(&run.id),
                )?;
                last = Some(item.clone());
                tx.execute(
                    "INSERT OR REPLACE INTO dream_change (run_id, memory_id, before_revision, after_revision, action, reason) VALUES (?1, ?2, 0, 1, ?3, ?4)",
                    params![run.id, item.id, action_str(&change.action), change.reason],
                )?;
                applied += 1;
                continue;
            }
        };
        let item =
            item_by_id(tx, &memory_id)?.ok_or_else(|| MemoryError::NotFound(memory_id.clone()))?;
        let before = item.current_revision;
        let after = before + 1;
        let retired = matches!(change.action, DreamAction::Retire);
        let title = if retired { "" } else { &change.title };
        let summary = if retired { "" } else { &change.summary };
        let body = if retired { "" } else { &change.body };
        tx.execute(
            "UPDATE memory SET title = ?1, summary = ?2, body = ?3, updated_at = ?4, current_revision = ?5, status = ?6 WHERE id = ?7",
            params![title, summary, body, now, after as i64, if retired { "retired" } else { "active" }, memory_id],
        )?;
        tx.execute(
            "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, dream_run_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![memory_id, after as i64, title, summary, body, item.source_kind, item.source_ref, change.reason, run.id, now],
        )?;
        fts_put(tx, &memory_id, item.layer, title, body)?;
        tx.execute(
            "INSERT OR REPLACE INTO dream_change (run_id, memory_id, before_revision, after_revision, action, reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![run.id, memory_id, before as i64, after as i64, action_str(&change.action), change.reason],
        )?;
        last = item_by_id(tx, &memory_id)?;
        applied += 1;
    }
    Ok(WriteOutcome {
        item: last,
        dream_applied: applied,
        ..WriteOutcome::default()
    })
}

fn dream_undo(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    only: Option<&str>,
    now: i64,
) -> Result<WriteOutcome, MemoryError> {
    let changes: Vec<(String, i64, i64)> = {
        let mut stmt = tx.prepare(
            "SELECT memory_id, before_revision, after_revision FROM dream_change WHERE run_id = ?1 ORDER BY memory_id",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut applied = 0;
    let mut skipped = Vec::new();
    for (id, before, after) in changes {
        if only.is_some_and(|only| only != id) {
            continue;
        }
        let Some(item) = item_by_id(tx, &id)? else {
            skipped.push(id);
            continue;
        };
        if item.current_revision as i64 != after {
            skipped.push(id);
            continue;
        }
        if before == 0 {
            tx.execute("DELETE FROM memory WHERE id = ?1", params![id])?;
            tx.execute(
                "DELETE FROM memory_revision WHERE memory_id = ?1",
                params![id],
            )?;
            tx.execute("DELETE FROM memory_fts WHERE memory_id = ?1", params![id])?;
            tx.execute(
                "DELETE FROM dream_change WHERE run_id = ?1 AND memory_id = ?2",
                params![run_id, id],
            )?;
            applied += 1;
            continue;
        }
        let (title, summary, body) = revision_content(tx, &id, before)?;
        let next = item.current_revision + 1;
        tx.execute(
            "UPDATE memory SET title = ?1, summary = ?2, body = ?3, updated_at = ?4, current_revision = ?5, status = 'active' WHERE id = ?6",
            params![title, summary, body, now, next as i64, id],
        )?;
        tx.execute(
            "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, undo_of, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'dream_undo', ?8, ?9)",
            params![id, next as i64, title, summary, body, item.source_kind, item.source_ref, after, now],
        )?;
        fts_put(tx, &id, item.layer, &title, &body)?;
        applied += 1;
    }
    Ok(WriteOutcome {
        dream_applied: applied,
        dream_skipped: skipped,
        ..WriteOutcome::default()
    })
}

fn revision_content(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    revision: i64,
) -> Result<(String, String, String), MemoryError> {
    tx.query_row(
        "SELECT title, summary, body FROM memory_revision WHERE memory_id = ?1 AND revision = ?2",
        params![id, revision],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .map_err(MemoryError::Sqlite)
}

fn index_conversation(
    tx: &rusqlite::Transaction<'_>,
    conversation_id: &str,
    seq: u64,
    turn: u64,
    text: &str,
) -> Result<(), MemoryError> {
    tx.execute(
        "DELETE FROM conversation_fts WHERE conversation_id = ?1 AND seq = ?2",
        params![conversation_id, seq as i64],
    )?;
    tx.execute(
        "INSERT INTO conversation_fts (conversation_id, seq, turn, text, source_json) VALUES (?1, ?2, ?3, ?4, '{}')",
        params![conversation_id, seq as i64, turn as i64, text],
    )?;
    Ok(())
}

fn search_like(conn: &Connection, query: &str, limit: i64) -> Result<Vec<SearchHit>, MemoryError> {
    let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let mut hits = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id FROM memory WHERE status = 'active' AND (title LIKE ?1 ESCAPE '\\' OR body LIKE ?1 ESCAPE '\\') ORDER BY updated_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit], |row| row.get::<_, String>(0))?;
        for id in rows {
            if let Some(item) = item_by_id(conn, &id?)? {
                hits.push(SearchHit::Memory(item));
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT conversation_id, seq, turn, text FROM conversation_fts WHERE text LIKE ?1 ESCAPE '\\' ORDER BY seq DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit], |row| {
            Ok(SearchHit::Conversation {
                conversation_id: row.get(0)?,
                seq: row.get::<_, i64>(1)? as u64,
                turn: row.get::<_, i64>(2)? as u64,
                text: row.get(3)?,
            })
        })?;
        for hit in rows {
            hits.push(hit?);
        }
    }
    Ok(hits)
}

fn search_fts(conn: &Connection, query: &str, limit: i64) -> Result<Vec<SearchHit>, MemoryError> {
    let escaped = query.replace('"', "\"\"");
    let match_query = format!("\"{escaped}\"");
    let mut hits = Vec::new();
    {
        let mut stmt =
            conn.prepare("SELECT memory_id FROM memory_fts WHERE memory_fts MATCH ?1 LIMIT ?2")?;
        let rows = stmt.query_map(params![match_query, limit], |row| row.get::<_, String>(0))?;
        for id in rows {
            if let Some(item) = item_by_id(conn, &id?)? {
                hits.push(SearchHit::Memory(item));
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT conversation_id, seq, turn, text FROM conversation_fts WHERE conversation_fts MATCH ?1 LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![match_query, limit], |row| {
            Ok(SearchHit::Conversation {
                conversation_id: row.get(0)?,
                seq: row.get::<_, i64>(1)? as u64,
                turn: row.get::<_, i64>(2)? as u64,
                text: row.get(3)?,
            })
        })?;
        for hit in rows {
            hits.push(hit?);
        }
    }
    Ok(hits)
}

fn tombstone_exists(
    tx: &rusqlite::Transaction<'_>,
    kind: &str,
    source_ref: &str,
) -> Result<bool, MemoryError> {
    let hash = hash_source(kind, source_ref);
    let found: Option<i64> = tx
        .query_row(
            "SELECT at FROM tombstone WHERE source_hash = ?1",
            params![hash],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn hash_source(kind: &str, source_ref: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in kind
        .as_bytes()
        .iter()
        .chain([0].iter())
        .chain(source_ref.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn action_str(action: &DreamAction) -> &'static str {
    match action {
        DreamAction::Merge => "merge",
        DreamAction::Correct => "correct",
        DreamAction::Retire => "retire",
        DreamAction::NewNote => "new_note",
    }
}

fn day_of(at: i64) -> String {
    let days = at.div_euclid(86_400_000);
    let (_, month, day) = civil_from_days(days);
    format!("{month:02}-{day:02}")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn next_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{prefix}-{nanos:x}-{counter:x}")
}
