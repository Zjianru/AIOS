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
use crate::error::WriteError;
use crate::types::{
    AboutDraftPayload, AboutEdit, Actor, Binding, DiaryEntry, DreamAction, DreamChange,
    DreamChangeInput, DreamRun, DreamSettings, DreamSettingsPatch, Layer, MemoryCardRecord,
    MemoryItem, MemoryKey, MemoryKeyKind, MemoryMigrationReport, MemoryRelation, MemorySource,
    MemoryView, MemoryWrite, RelationCandidate, RelationDecision, RevisionItem, Scope, SearchGap,
    SearchHit, SearchRequest, SearchResponse, WriteContext, WriteOp, WriteOutcome,
};

pub const SCHEMA_VERSION: u64 = 3;
const V2_BACKUP_FILE: &str = "memory.db.v2.bak";
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
        pinned INTEGER NOT NULL DEFAULT 0,
        valid_from INTEGER NOT NULL DEFAULT 0,
        valid_until INTEGER,
        invalid_reason TEXT CHECK (invalid_reason IS NULL OR invalid_reason IN ('replaced','user_deadline','deleted')),
        superseded_by TEXT,
        possibly_stale INTEGER NOT NULL DEFAULT 0
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
        actor TEXT NOT NULL DEFAULT 'system',
        created_at INTEGER NOT NULL,
        valid_from INTEGER NOT NULL DEFAULT 0,
        valid_until INTEGER,
        invalid_reason TEXT CHECK (invalid_reason IS NULL OR invalid_reason IN ('replaced','user_deadline','deleted')),
        superseded_by TEXT,
        possibly_stale INTEGER NOT NULL DEFAULT 0,
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
    CREATE TABLE IF NOT EXISTS memory_key (
        memory_id TEXT NOT NULL,
        key_no INTEGER NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('keyword','path','command','event','entity')),
        value TEXT NOT NULL,
        PRIMARY KEY (memory_id, key_no)
    );
    CREATE TABLE IF NOT EXISTS memory_source (
        memory_id TEXT NOT NULL,
        source_kind TEXT NOT NULL,
        source_ref TEXT NOT NULL,
        quote TEXT,
        added_at INTEGER NOT NULL,
        PRIMARY KEY (memory_id, source_kind, source_ref)
    );
    CREATE TABLE IF NOT EXISTS memory_relation (
        memory_id TEXT NOT NULL,
        related_id TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('supplement','replaces')),
        PRIMARY KEY (memory_id, related_id, kind)
    );
    CREATE TABLE IF NOT EXISTS memory_card (
        card_ref TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        status TEXT NOT NULL,
        payload_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS memory_delivery (
        memory_id TEXT NOT NULL,
        conversation_id TEXT NOT NULL,
        turn INTEGER NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('matched','sent','suppressed','used')),
        at INTEGER NOT NULL,
        PRIMARY KEY (memory_id, conversation_id, turn, kind)
    );
    CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
        kind, memory_id UNINDEXED, title, body, source_json UNINDEXED, keys,
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
    migration_report: Option<MemoryMigrationReport>,
}

impl MemoryStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        Self::open_with_clock(path, Arc::new(now_millis))
    }

    /// 测试用的假时钟入口；生产走 open。
    pub fn open_with_clock(path: impl AsRef<Path>, clock: Clock) -> Result<Self, MemoryError> {
        Self::open_inner(path.as_ref(), clock, false)
    }

    /// 迁移失败注入：只给回滚测试用，生产不会走到这条路。
    #[doc(hidden)]
    pub fn open_with_failed_migration_for_test(
        path: impl AsRef<Path>,
    ) -> Result<Self, MemoryError> {
        Self::open_inner(path.as_ref(), Arc::new(now_millis), true)
    }

    fn open_inner(path: &Path, clock: Clock, fail_migration: bool) -> Result<Self, MemoryError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)?;
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
        let mut migration_report = None;
        match version {
            None => {
                conn.execute_batch(CREATE_TABLES)?;
                conn.execute(
                    "INSERT INTO kv (key, value) VALUES (?1, ?2)",
                    params![SCHEMA_KEY, SCHEMA_VERSION.to_string()],
                )?;
                conn.pragma_update(
                    None,
                    "user_version",
                    i64::try_from(SCHEMA_VERSION).unwrap_or(3),
                )?;
            }
            Some(old) if old < SCHEMA_VERSION => {
                migration_report = Some(migrate_to_v3(&mut conn, path, old, fail_migration)?);
            }
            Some(_) => {
                conn.execute_batch(CREATE_TABLES)?;
                conn.pragma_update(
                    None,
                    "user_version",
                    i64::try_from(SCHEMA_VERSION).unwrap_or(3),
                )?;
            }
        }
        conn.execute(
            "INSERT OR IGNORE INTO kv (key, value) VALUES (?1, '0')",
            params![VERSION_KEY],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            clock,
            migration_report,
        })
    }

    /// 从旧库迁上来时的报告；没有迁移过就是 None。
    pub fn migration_report(&self) -> Option<&MemoryMigrationReport> {
        self.migration_report.as_ref()
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
            "where layer = 'note' and status in ('active','pinned') order by updated_at desc",
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
    /// 旧单 query 入口保留薄包装；新调用方用 search_request。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MemoryError> {
        let response = self.search_request(SearchRequest {
            query: query.to_owned(),
            alternates: Vec::new(),
            limit,
            conversation_id: None,
            conversation_last_seq: None,
        })?;
        let mut hits: Vec<SearchHit> = response.originals;
        hits.extend(response.derived);
        Ok(hits)
    }

    /// 一个 alternate 一路 FTS5/LIKE，按 RRF(k=60) 合并后分成 originals/derived 两张表。
    pub fn search_request(&self, request: SearchRequest) -> Result<SearchResponse, MemoryError> {
        let conn = self.lock();
        let limit = request.limit;
        let mut queries = Vec::new();
        for query in std::iter::once(&request.query)
            .chain(request.alternates.iter())
            .map(|query| query.trim())
            .filter(|query| !query.is_empty())
        {
            if !queries.iter().any(|seen: &String| seen == query) {
                queries.push(query.to_owned());
            }
        }
        if queries.is_empty() || limit == 0 {
            return Ok(SearchResponse {
                gaps: vec![SearchGap::NoRelevantEvidence],
                ..SearchResponse::default()
            });
        }
        // 工具参数上限就是 6 个别的说法；store 再多拿也只取前 6。
        queries.truncate(7);
        let sql_limit = i64::try_from(limit).unwrap_or(50);
        let mut scores: std::collections::BTreeMap<String, (SearchHit, f64)> =
            std::collections::BTreeMap::new();
        let mut any_ok = false;
        let mut any_truncated = false;
        let mut short_like_truncated = false;
        for query in &queries {
            let short = query.chars().count() < 3;
            let hits = if short {
                search_like(&conn, query, sql_limit)
            } else {
                search_fts(&conn, query, sql_limit)
            };
            let hits = match hits {
                Ok(hits) => hits,
                Err(_) => continue,
            };
            any_ok = true;
            if hits.len() >= limit {
                any_truncated = true;
            }
            if short && hits.len() >= limit {
                short_like_truncated = true;
            }
            let mut seen = std::collections::BTreeSet::new();
            for (index, hit) in hits.into_iter().enumerate() {
                let key = search_hit_key(&hit);
                if !seen.insert(key.clone()) {
                    continue;
                }
                let score = 1.0 / (60.0 + (index + 1) as f64);
                scores
                    .entry(key)
                    .and_modify(|(_, total)| *total += score)
                    .or_insert((hit, score));
            }
        }
        if !any_ok {
            return Ok(SearchResponse {
                gaps: vec![SearchGap::RawSearchUnavailable],
                ..SearchResponse::default()
            });
        }
        let mut ranked: Vec<(SearchHit, f64)> = scores.into_values().collect();
        ranked.sort_by(|(left_hit, left), (right_hit, right)| {
            right
                .partial_cmp(left)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| search_hit_key(left_hit).cmp(&search_hit_key(right_hit)))
        });
        let mut response = SearchResponse::default();
        for (hit, _) in ranked {
            if is_derived_hit(&hit) {
                if response.derived.len() < limit {
                    response.derived.push(hit);
                } else {
                    any_truncated = true;
                }
            } else if response.originals.len() < limit {
                response.originals.push(hit);
            } else {
                any_truncated = true;
            }
        }
        if response.originals.is_empty() && response.derived.is_empty() {
            response.gaps.push(SearchGap::NoRelevantEvidence);
        }
        let conversation_incomplete = match (
            request.conversation_id.as_deref(),
            request.conversation_last_seq,
        ) {
            (Some(conversation_id), Some(last_seq)) => {
                let indexed: Option<i64> = conn
                    .query_row(
                        "SELECT max(seq) FROM conversation_fts WHERE conversation_id = ?1",
                        params![conversation_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
                indexed.is_none_or(|indexed| u64::try_from(indexed).unwrap_or(0) < last_seq)
            }
            _ => false,
        };
        if conversation_incomplete || short_like_truncated {
            response.gaps.push(SearchGap::SourceCoverageIncomplete);
        }
        if any_truncated {
            response.gaps.push(SearchGap::ResultsTruncated);
        }
        Ok(response)
    }

    /// 某个会话已经进索引的最大 seq；测试和覆盖判断用。
    pub fn last_indexed_conversation_seq(
        &self,
        conversation_id: &str,
    ) -> Result<Option<u64>, MemoryError> {
        let conn = self.lock();
        let seq: Option<i64> = conn
            .query_row(
                "SELECT max(seq) FROM conversation_fts WHERE conversation_id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(seq.and_then(|seq| u64::try_from(seq).ok()))
    }

    /// 读一条记忆（页面/工具按 id 读）。
    pub fn item(&self, id: &str) -> Result<Option<MemoryItem>, MemoryError> {
        let conn = self.lock();
        item_by_id(&conn, id)
    }

    /// 一条记忆的钥匙，按 key_no 排。
    pub fn keys(&self, id: &str) -> Result<Vec<MemoryKey>, MemoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT key_no, kind, value FROM memory_key WHERE memory_id = ?1 ORDER BY key_no",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            let kind: String = row.get(1)?;
            Ok(MemoryKey {
                key_no: row.get::<_, i64>(0)? as u32,
                kind: MemoryKeyKind::parse(&kind).unwrap_or(MemoryKeyKind::Keyword),
                value: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::Sqlite)
    }

    /// 一条记忆的出处。
    pub fn sources(&self, id: &str) -> Result<Vec<MemorySource>, MemoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT source_kind, source_ref, quote, added_at FROM memory_source WHERE memory_id = ?1 ORDER BY added_at, source_kind, source_ref",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(MemorySource {
                source_kind: row.get(0)?,
                source_ref: row.get(1)?,
                quote: row.get(2)?,
                added_at: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::Sqlite)
    }

    /// 一条记忆的关系边（supplement / replaces）。
    pub fn relations(&self, id: &str) -> Result<Vec<MemoryRelation>, MemoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT related_id, kind FROM memory_relation WHERE memory_id = ?1 ORDER BY kind, related_id",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(MemoryRelation {
                related_id: row.get(0)?,
                kind: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::Sqlite)
    }

    /// 联想回忆：当前消息命中 entity 钥匙，且这条记忆通过 supplement/replaces
    /// 连到一条更早的旧事，才返回旧事；同话题直接命中不送。
    pub fn flashback_bridges(
        &self,
        message: &str,
    ) -> Result<Vec<crate::types::FlashbackBridge>, MemoryError> {
        let message = message.trim().to_lowercase();
        if message.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT k.memory_id FROM memory_key k JOIN memory m ON m.id = k.memory_id
             WHERE k.kind = 'entity' AND m.status = 'active' AND m.invalid_reason IS NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut hit_ids = Vec::new();
        for id in rows {
            let id = id?;
            if hit_ids.contains(&id) {
                continue;
            }
            let mut keys = conn
                .prepare("SELECT value FROM memory_key WHERE memory_id = ?1 AND kind = 'entity'")?;
            let values = keys.query_map(params![id], |row| row.get::<_, String>(0))?;
            let mut matched = false;
            for value in values {
                let value = value?;
                if !value.trim().is_empty() && message.contains(&value.trim().to_lowercase()) {
                    matched = true;
                    break;
                }
            }
            if matched {
                hit_ids.push(id);
            }
        }
        let mut bridges = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for id in hit_ids {
            let Some(hit) = item_by_id(&conn, &id)? else {
                continue;
            };
            let mut relations = conn.prepare(
                "SELECT related_id, kind FROM memory_relation WHERE memory_id = ?1 ORDER BY kind, related_id",
            )?;
            let rows = relations.query_map(params![id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for relation in rows {
                let (related_id, kind) = relation?;
                if kind != "supplement" && kind != "replaces" {
                    continue;
                }
                if seen.insert((id.clone(), related_id.clone(), kind.clone()))
                    && let Some(older) = item_by_id(&conn, &related_id)?
                    && older.valid_from <= hit.valid_from
                {
                    bridges.push(crate::types::FlashbackBridge {
                        hit: hit.clone(),
                        older,
                        relation: kind,
                    });
                }
            }
        }
        Ok(bridges)
    }

    /// 换一组钥匙：删旧、写新、FTS 的 keys 在同一个事务里更新。
    pub fn set_keys(&self, id: &str, keys: &[(MemoryKeyKind, String)]) -> Result<(), MemoryError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if item_by_id(&tx, id)?.is_none() {
            return Err(MemoryError::NotFound(id.to_owned()));
        }
        tx.execute("DELETE FROM memory_key WHERE memory_id = ?1", params![id])?;
        let mut values = Vec::with_capacity(keys.len());
        for (index, (kind, value)) in keys.iter().enumerate() {
            let value = value.trim();
            if value.is_empty() {
                return Err(MemoryError::Refused("钥匙不能为空"));
            }
            tx.execute(
                "INSERT INTO memory_key (memory_id, key_no, kind, value) VALUES (?1, ?2, ?3, ?4)",
                params![
                    id,
                    i64::try_from(index).unwrap_or(i64::MAX),
                    kind.as_str(),
                    value
                ],
            )?;
            values.push(value.to_owned());
        }
        update_fts_keys(&tx, id, &values.join(" "))?;
        bump_version(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// 追加一条出处；旧库迁移不调用这里，所以不会伪造旧原文。
    pub fn add_source(
        &self,
        id: &str,
        source_kind: &str,
        source_ref: &str,
        quote: Option<&str>,
    ) -> Result<(), MemoryError> {
        let added_at = (self.clock)();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if item_by_id(&tx, id)?.is_none() {
            return Err(MemoryError::NotFound(id.to_owned()));
        }
        tx.execute(
            "INSERT INTO memory_source (memory_id, source_kind, source_ref, quote, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(memory_id, source_kind, source_ref) DO UPDATE SET quote = excluded.quote",
            params![id, source_kind, source_ref, quote, added_at],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 一条记忆的全部修订（从第 1 版起，只增不改）。
    pub fn revisions(&self, id: &str) -> Result<Vec<RevisionItem>, MemoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT memory_id, revision, title, summary, body, reason, dream_run_id, undo_of, actor, created_at FROM memory_revision WHERE memory_id = ?1 ORDER BY revision",
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
                actor: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::Sqlite)
    }

    /// 库自己的时钟（毫秒）；设置/做梦记录与条目用同一个时间来源。
    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    /// 读一条日记（做梦按 id 读、页面按 id 读）。
    pub fn diary(&self, id: &str) -> Result<Option<DiaryEntry>, MemoryError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, day, seq, text, source_ref, created_at FROM diary_entry WHERE id = ?1",
            params![id],
            |row| {
                Ok(DiaryEntry {
                    id: row.get(0)?,
                    day: row.get(1)?,
                    seq: row.get::<_, i64>(2)? as u64,
                    text: row.get(3)?,
                    source_ref: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(MemoryError::Sqlite)
    }

    /// 做梦/观察的设置（缺省值照 spec）。
    pub fn dream_settings(&self) -> Result<DreamSettings, MemoryError> {
        let conn = self.lock();
        dream_settings_from(&conn)
    }

    /// 改设置：只写给出的项；写进版本号（页面按版本推）。
    pub fn update_dream_settings(
        &self,
        patch: &DreamSettingsPatch,
    ) -> Result<DreamSettings, MemoryError> {
        let mut pairs: Vec<(&str, String)> = Vec::new();
        if let Some(value) = patch.enabled {
            pairs.push(("dream.enabled", value.to_string()));
        }
        if let Some(value) = patch.permit {
            pairs.push(("dream.permit", value.to_string()));
        }
        if let Some(value) = patch.interval_hours {
            pairs.push(("dream.interval_hours", value.to_string()));
        }
        if let Some(value) = patch.min_events {
            pairs.push(("dream.min_events", value.to_string()));
        }
        if let Some(value) = patch.last_success_at {
            pairs.push(("dream.last_success_at", value.to_string()));
        }
        if let Some(value) = patch.last_attempt_at {
            pairs.push(("dream.last_attempt_at", value.to_string()));
        }
        if let Some(value) = patch.last_read_seq {
            pairs.push(("dream.last_read_seq", value.to_string()));
        }
        if let Some(value) = patch.permit_card_at {
            pairs.push(("dream.permit_card_at", value.to_string()));
        }
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (key, value) in &pairs {
            tx.execute(
                "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
        if !pairs.is_empty() {
            bump_version(&tx)?;
        }
        let settings = dream_settings_from(&tx)?;
        tx.commit()?;
        Ok(settings)
    }

    /// 一次做梦的记录。
    pub fn dream_run(&self, id: &str) -> Result<Option<DreamRun>, MemoryError> {
        let conn = self.lock();
        dream_run_row(&conn, id)
    }

    /// 最近的一次做梦（页面/撤销默认它）。
    pub fn latest_dream_run(&self) -> Result<Option<DreamRun>, MemoryError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, started_at, ended_at, outcome, model, usage_json, report_json, error FROM dream_run ORDER BY started_at DESC, rowid DESC LIMIT 1",
            [],
            dream_run_from_row,
        )
        .optional()
        .map_err(MemoryError::Sqlite)
    }

    /// 最近的几次做梦，新的在前。
    pub fn dream_runs(&self, limit: usize) -> Result<Vec<DreamRun>, MemoryError> {
        let conn = self.lock();
        let limit = i64::try_from(limit).unwrap_or(20);
        let mut stmt = conn.prepare(
            "SELECT id, started_at, ended_at, outcome, model, usage_json, report_json, error FROM dream_run ORDER BY started_at DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], dream_run_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::Sqlite)
    }

    /// 一次做梦改了哪些处。
    pub fn dream_changes(&self, run_id: &str) -> Result<Vec<DreamChange>, MemoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT run_id, memory_id, before_revision, after_revision, action, reason FROM dream_change WHERE run_id = ?1 ORDER BY memory_id",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok(DreamChange {
                run_id: row.get(0)?,
                memory_id: row.get(1)?,
                before_revision: row.get::<_, i64>(2)? as u64,
                after_revision: row.get::<_, i64>(3)? as u64,
                action: row.get(4)?,
                reason: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::Sqlite)
    }

    /// 做梦跑完后补记录（结束时间/结果/报告/用量/失败原因）；run 行必须已由 DreamApply 建过。
    pub fn finish_dream_run(
        &self,
        id: &str,
        ended_at: i64,
        outcome: &str,
        report_json: &str,
        usage_json: &str,
        error: &str,
    ) -> Result<(), MemoryError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE dream_run SET ended_at = ?2, outcome = ?3, report_json = ?4, usage_json = ?5, error = ?6 WHERE id = ?1",
            params![id, ended_at, outcome, report_json, usage_json, error],
        )?;
        if changed == 0 {
            return Err(MemoryError::NotFound(id.to_owned()));
        }
        bump_version(&tx)?;
        tx.commit()?;
        Ok(())
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
                    actor_str(ctx.actor),
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
                    actor_str(ctx.actor),
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
                let item = correct_item(
                    &tx,
                    &id,
                    &title,
                    &summary,
                    &body,
                    &reason,
                    now,
                    actor_str(ctx.actor),
                )?;
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
            WriteOp::DreamApply { run, changes } => {
                if ctx.dream_run.as_deref() != Some(run.id.as_str()) {
                    return Err(MemoryError::Refused("做梦 run id 和写入上下文不一致"));
                }
                dream_apply(&tx, &run, &changes, now)?
            }
            WriteOp::DreamUndo { run_id, only } => {
                // 做梦自己撤要带对 run；用户从页面撤只给出 run id 就够（identity 不是做梦）。
                if ctx.actor == Actor::Dream && ctx.dream_run.as_deref() != Some(run_id.as_str()) {
                    return Err(MemoryError::Refused("做梦 run id 和撤销上下文不一致"));
                }
                dream_undo(&tx, &run_id, only.as_deref(), now, actor_str(ctx.actor))?
            }
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

    /// 撤销「被替换」：旧条目恢复有效，新条目留着（只适用于 replaced）。
    pub fn restore_invalid(&self, id: &str, ctx: &WriteContext) -> Result<MemoryItem, MemoryError> {
        if ctx.actor != Actor::User {
            return Err(MemoryError::Refused("只有用户能撤销失效"));
        }
        let now = (self.clock)();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if item_by_id(&tx, id)?.is_none() {
            return Err(MemoryError::NotFound(id.to_owned()));
        }
        let new_id: Option<String> = tx
            .query_row(
                "SELECT superseded_by FROM memory WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let Some(new_id) = new_id else {
            return Err(MemoryError::Refused("这一条不是被替换失效"));
        };
        tx.execute(
            "UPDATE memory SET valid_until = NULL, invalid_reason = NULL, superseded_by = NULL, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        tx.execute(
            "DELETE FROM memory_relation WHERE memory_id = ?1 AND related_id = ?2 AND kind = 'replaces'",
            params![new_id, id],
        )?;
        bump_version(&tx)?;
        let updated = item_by_id(&tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()))?;
        tx.commit()?;
        Ok(updated)
    }

    /// 记一条送达/使用日志；调用方只在 TurnEnd succeeded 时记 sent。
    pub fn record_delivery(
        &self,
        ids: &[String],
        conversation_id: &str,
        turn: u64,
        kind: &str,
    ) -> Result<(), MemoryError> {
        let at = (self.clock)();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in ids {
            tx.execute(
                "INSERT OR IGNORE INTO memory_delivery (memory_id, conversation_id, turn, kind, at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    conversation_id,
                    i64::try_from(turn).unwrap_or(0),
                    kind,
                    at
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 一条记忆的送达/使用日志（测试/页面读）。
    pub fn deliveries(&self, id: &str) -> Result<Vec<(String, u64, String, i64)>, MemoryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT conversation_id, turn, kind, at FROM memory_delivery WHERE memory_id = ?1 ORDER BY at",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok((
                row.get(0)?,
                row.get::<_, i64>(1)? as u64,
                row.get(2)?,
                row.get(3)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::Sqlite)
    }

    /// 存一张待决的「关于你」草稿卡；edits 已由服务核过原话/旧条。
    pub fn draft_about(
        &self,
        quote: &str,
        edits: Vec<AboutEdit>,
    ) -> Result<MemoryCardRecord, MemoryError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let payload = AboutDraftPayload {
            quote: quote.to_owned(),
            edits,
        };
        let payload = serde_json::to_value(payload)
            .map_err(|error| MemoryError::StorageConfig(error.to_string()))?;
        let card = put_card_in(&tx, "about_draft", "pending", &payload)?;
        bump_version(&tx)?;
        tx.commit()?;
        Ok(card)
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

    /// 按 scope + 钥匙捞可能相关的旧条目；没有钥匙就没有候选。
    pub fn relation_candidates(
        &self,
        scope: Scope,
        keys: &[MemoryKey],
    ) -> Result<Vec<RelationCandidate>, MemoryError> {
        let conn = self.lock();
        candidates_in(&conn, scope, keys)
    }

    /// 带关系判断的一次写入。candidate 非空而 relation 为空时只回候选，不写。
    pub fn write_with_relation(
        &self,
        write: MemoryWrite,
        ctx: &WriteContext,
    ) -> Result<WriteOutcome, WriteError> {
        check_actor(&write.op, ctx)?;
        let (layer, binding, title, summary, body, source_kind, source_ref) = match &write.op {
            WriteOp::Remember {
                title,
                summary,
                body,
                source_kind,
                source_ref,
            } => (
                Layer::AboutYou,
                Binding::Binding,
                title.clone(),
                summary.clone(),
                body.clone(),
                source_kind.clone(),
                source_ref.clone(),
            ),
            WriteOp::Note {
                title,
                summary,
                body,
                source_kind,
                source_ref,
            } => (
                Layer::Note,
                Binding::Fallible,
                title.clone(),
                summary.clone(),
                body.clone(),
                source_kind.clone(),
                source_ref.clone(),
            ),
            _ => {
                if write.relation.is_some() || !write.keys.is_empty() {
                    return Err(WriteError::InvalidRelation(
                        "只有 remember/note 支持关系判断和钥匙".to_owned(),
                    ));
                }
                return self.write(write.op, ctx).map_err(WriteError::Memory);
            }
        };
        let now = write.valid_from.unwrap_or_else(|| (self.clock)());
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope = if write.project_id.is_some() {
            Scope::Project
        } else {
            scope_of(&write.op)
        };
        let candidates = if write.keys.is_empty() {
            Vec::new()
        } else {
            candidates_in(&tx, scope, &write.keys)?
        };
        let relation = match write.relation.clone() {
            Some(relation) => relation,
            None if candidates.is_empty() => RelationDecision::Independent,
            None => {
                return Err(WriteError::RelationRequired { candidates });
            }
        };
        let (mut outcome, needs_card) = match relation {
            RelationDecision::Independent => {
                let item = create_with_details(
                    &tx,
                    layer,
                    binding,
                    scope,
                    &title,
                    &summary,
                    &body,
                    &source_kind,
                    &source_ref,
                    write.source_quote.as_deref(),
                    &write.keys,
                    now,
                    actor_str(ctx.actor),
                )?;
                (
                    WriteOutcome {
                        item: Some(item),
                        ..WriteOutcome::default()
                    },
                    None,
                )
            }
            RelationDecision::Duplicate { target } => {
                let old = item_by_id(&tx, &target)?
                    .filter(|item| {
                        item.status == "active"
                            && invalid_reason_of(&tx, &item.id).ok().flatten().is_none()
                    })
                    .ok_or_else(|| {
                        WriteError::InvalidRelation(format!("重复目标不存在或已失效：{target}"))
                    })?;
                add_source_row(
                    &tx,
                    &target,
                    &source_kind,
                    &source_ref,
                    write.source_quote.as_deref(),
                    now,
                )?;
                (
                    WriteOutcome {
                        item: Some(old),
                        duplicate_of: Some(target),
                        ..WriteOutcome::default()
                    },
                    None,
                )
            }
            RelationDecision::Supplement { target } => {
                ensure_active(&tx, &target)?;
                let item = create_with_details(
                    &tx,
                    layer,
                    binding,
                    scope,
                    &title,
                    &summary,
                    &body,
                    &source_kind,
                    &source_ref,
                    write.source_quote.as_deref(),
                    &write.keys,
                    now,
                    actor_str(ctx.actor),
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO memory_relation (memory_id, related_id, kind) VALUES (?1, ?2, 'supplement')",
                    params![item.id, target],
                )?;
                (
                    WriteOutcome {
                        item: Some(item),
                        related: Some(target),
                        ..WriteOutcome::default()
                    },
                    None,
                )
            }
            RelationDecision::Contradict { target, reason } => {
                let old = ensure_active(&tx, &target)?;
                if old.binding == Binding::Binding {
                    // 新的先记成 Kind 笔记；用户点替换前旧的一直有效。
                    let item = create_with_details(
                        &tx,
                        Layer::Note,
                        Binding::Fallible,
                        scope,
                        &title,
                        &summary,
                        &body,
                        &source_kind,
                        &source_ref,
                        write.source_quote.as_deref(),
                        &write.keys,
                        now,
                        "kind",
                    )?;
                    let payload = serde_json::json!({
                        "old_id": old.id,
                        "new_id": item.id,
                        "old_title": old.title,
                        "old_text": old.body,
                        "new_text": item.body,
                    });
                    let card = put_card_in(&tx, "replace", "pending", &payload)?;
                    (
                        WriteOutcome {
                            item: Some(item.clone()),
                            related: Some(old.id.clone()),
                            card: Some(card.clone()),
                            ..WriteOutcome::default()
                        },
                        Some(WriteError::NeedsCard {
                            target: old.id,
                            card: Box::new(card),
                            new_id: item.id,
                        }),
                    )
                } else {
                    let new_item = create_with_details(
                        &tx,
                        layer,
                        binding,
                        scope,
                        &title,
                        &summary,
                        &body,
                        &source_kind,
                        &source_ref,
                        write.source_quote.as_deref(),
                        &write.keys,
                        now,
                        actor_str(ctx.actor),
                    )?;
                    invalidate_item(&tx, &old.id, &new_item.id, now)?;
                    tx.execute(
                        "INSERT OR IGNORE INTO memory_relation (memory_id, related_id, kind) VALUES (?1, ?2, 'replaces')",
                        params![new_item.id, old.id],
                    )?;
                    let _ = reason;
                    (
                        WriteOutcome {
                            item: Some(new_item),
                            related: Some(old.id),
                            ..WriteOutcome::default()
                        },
                        None,
                    )
                }
            }
        };
        if let Some(project_id) = write.project_id.as_deref()
            && let Some(item) = outcome.item.as_mut()
        {
            tx.execute(
                "UPDATE memory SET project_id = ?1 WHERE id = ?2",
                params![project_id, item.id],
            )?;
            item.project_id = Some(project_id.to_owned());
            item.scope = Scope::Project;
        }
        bump_version(&tx)?;
        tx.commit()?;
        if let Some(error) = needs_card {
            return Err(error);
        }
        Ok(outcome)
    }

    /// 一条待决卡的状态/内容。
    pub fn card(&self, card_ref: &str) -> Result<Option<MemoryCardRecord>, MemoryError> {
        let conn = self.lock();
        card_in(&conn, card_ref)
    }

    /// 替换卡：替换/两条都留；只能生效一次，再点返回现状态。
    pub fn resolve_replace(
        &self,
        card_ref: &str,
        replace: bool,
        ctx: &WriteContext,
    ) -> Result<MemoryCardRecord, MemoryError> {
        let now = (self.clock)();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut card) = card_in(&tx, card_ref)? else {
            return Err(MemoryError::NotFound(card_ref.to_owned()));
        };
        if card.kind != "replace" {
            return Err(MemoryError::Refused("不是替换卡"));
        }
        if card.status != "pending" {
            return Ok(card);
        }
        if replace {
            let old_id = card.payload["old_id"].as_str().unwrap_or_default();
            let new_id = card.payload["new_id"].as_str().unwrap_or_default();
            invalidate_item(&tx, old_id, new_id, now)?;
            promote_replacement(&tx, new_id, now, actor_str(ctx.actor))?;
            tx.execute(
                "INSERT OR IGNORE INTO memory_relation (memory_id, related_id, kind) VALUES (?1, ?2, 'replaces')",
                params![new_id, old_id],
            )?;
            card.status = "replaced".to_owned();
        } else {
            card.status = "kept".to_owned();
        }
        tx.execute(
            "UPDATE memory_card SET status = ?1 WHERE card_ref = ?2",
            params![card.status, card_ref],
        )?;
        bump_version(&tx)?;
        tx.commit()?;
        Ok(card)
    }

    /// 关于你草稿卡：保存/不用；只能生效一次。
    pub fn resolve_about_draft(
        &self,
        card_ref: &str,
        save: bool,
        ctx: &WriteContext,
    ) -> Result<MemoryCardRecord, MemoryError> {
        if ctx.actor != Actor::User {
            return Err(MemoryError::Refused("关于你只有用户能保存"));
        }
        let now = (self.clock)();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut card) = card_in(&tx, card_ref)? else {
            return Err(MemoryError::NotFound(card_ref.to_owned()));
        };
        if card.kind != "about_draft" {
            return Err(MemoryError::Refused("不是关于你草稿卡"));
        }
        if card.status != "pending" {
            return Ok(card);
        }
        if save {
            let payload: AboutDraftPayload = serde_json::from_value(card.payload.clone())
                .map_err(|error| MemoryError::StorageConfig(error.to_string()))?;
            apply_about_edits(&tx, &payload, card_ref, now, actor_str(ctx.actor))?;
            card.status = "saved".to_owned();
        } else {
            card.status = "dropped".to_owned();
        }
        tx.execute(
            "UPDATE memory_card SET status = ?1 WHERE card_ref = ?2",
            params![card.status, card_ref],
        )?;
        bump_version(&tx)?;
        tx.commit()?;
        Ok(card)
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

fn migrate_to_v3(
    conn: &mut Connection,
    path: &Path,
    from: u64,
    fail: bool,
) -> Result<MemoryMigrationReport, MemoryError> {
    let backup = if from == 2 {
        Some(backup_v2(conn, path)?)
    } else {
        None
    };
    let missing_original: u64 = conn.query_row("SELECT COUNT(*) FROM memory", [], |row| {
        row.get::<_, i64>(0)
    })? as u64;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if from == 1 && !table_has_column(&tx, "memory_revision", "actor")? {
        tx.execute(
            "ALTER TABLE memory_revision ADD COLUMN actor TEXT NOT NULL DEFAULT 'system'",
            [],
        )?;
    }
    add_column_if_missing(&tx, "memory", "valid_from", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(&tx, "memory", "valid_until", "INTEGER")?;
    add_column_if_missing(
        &tx,
        "memory",
        "invalid_reason",
        "TEXT CHECK (invalid_reason IS NULL OR invalid_reason IN ('replaced','user_deadline','deleted'))",
    )?;
    add_column_if_missing(&tx, "memory", "superseded_by", "TEXT")?;
    add_column_if_missing(
        &tx,
        "memory",
        "possibly_stale",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        &tx,
        "memory_revision",
        "valid_from",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(&tx, "memory_revision", "valid_until", "INTEGER")?;
    add_column_if_missing(
        &tx,
        "memory_revision",
        "invalid_reason",
        "TEXT CHECK (invalid_reason IS NULL OR invalid_reason IN ('replaced','user_deadline','deleted'))",
    )?;
    add_column_if_missing(&tx, "memory_revision", "superseded_by", "TEXT")?;
    add_column_if_missing(
        &tx,
        "memory_revision",
        "possibly_stale",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    tx.execute(
        "UPDATE memory SET valid_from = created_at WHERE valid_from = 0",
        [],
    )?;
    tx.execute(
        "UPDATE memory_revision SET valid_from = created_at WHERE valid_from = 0",
        [],
    )?;
    // FTS5 不能加列：旧表整张重建，词表里显式加 keys；旧数据没有钥匙，留空。
    tx.execute("DROP TABLE IF EXISTS memory_fts", [])?;
    tx.execute_batch(CREATE_TABLES)?;
    rebuild_memory_fts(&tx)?;
    if fail {
        return Err(MemoryError::Migration("测试注入：迁移失败".to_owned()));
    }
    tx.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SCHEMA_KEY, SCHEMA_VERSION.to_string()],
    )?;
    tx.commit()?;
    conn.pragma_update(
        None,
        "user_version",
        i64::try_from(SCHEMA_VERSION).unwrap_or(3),
    )?;
    Ok(MemoryMigrationReport {
        from_version: from,
        to_version: SCHEMA_VERSION,
        backup: backup.map(|path| path.display().to_string()),
        missing_original,
    })
}

fn backup_v2(conn: &Connection, path: &Path) -> Result<std::path::PathBuf, MemoryError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let backup = parent.join(V2_BACKUP_FILE);
    if backup.exists() {
        return Ok(backup);
    }
    let temp = parent.join(format!("{V2_BACKUP_FILE}.tmp"));
    drop(fs::remove_file(&temp));
    // VACUUM INTO 读的是同一个一致性快照，WAL 里的尾部也会带上。
    conn.execute(
        "VACUUM INTO ?1",
        params![temp.to_string_lossy().to_string()],
    )?;
    fs::rename(&temp, &backup)?;
    Ok(backup)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, MemoryError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), MemoryError> {
    if table_has_column(conn, table, column)? {
        return Ok(());
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

fn rebuild_memory_fts(conn: &Connection) -> Result<(), MemoryError> {
    conn.execute(
        "INSERT INTO memory_fts (kind, memory_id, title, body, source_json, keys)
         SELECT layer, id, title, body, '{}', '' FROM memory",
        [],
    )?;
    conn.execute(
        "INSERT INTO memory_fts (kind, memory_id, title, body, source_json, keys)
         SELECT 'diary', id, '', text, '{}', '' FROM diary_entry",
        [],
    )?;
    Ok(())
}

/// 修订里的「谁改的」：写入上下文的就是最终身份（Kind 的入口也按 User 记，挡住的是越权层）。
fn actor_str(actor: Actor) -> &'static str {
    match actor {
        Actor::User => "user",
        Actor::Kind => "kind",
        Actor::Dream => "dream",
    }
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
        "SELECT id, layer, binding, scope, title, summary, body, source_kind, source_ref, status, current_revision, updated_at, pinned, valid_from, valid_until, invalid_reason, superseded_by, possibly_stale, project_id FROM memory {where_clause}"
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
        valid_from: row.get(13)?,
        valid_until: row.get(14)?,
        invalid_reason: row.get(15)?,
        superseded_by: row.get(16)?,
        possibly_stale: row.get::<_, i64>(17)? != 0,
        project_id: row.get(18)?,
    })
}

fn diary_by_id(conn: &Connection, id: &str) -> Result<Option<DiaryEntry>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, day, seq, text, source_ref, created_at FROM diary_entry WHERE id = ?1",
    )?;
    stmt.query_row(params![id], |row| {
        Ok(DiaryEntry {
            id: row.get(0)?,
            day: row.get(1)?,
            seq: row.get::<_, i64>(2)? as u64,
            text: row.get(3)?,
            source_ref: row.get(4)?,
            created_at: row.get(5)?,
        })
    })
    .optional()
    .map_err(MemoryError::Sqlite)
}

fn item_by_id(conn: &Connection, id: &str) -> Result<Option<MemoryItem>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, layer, binding, scope, title, summary, body, source_kind, source_ref, status, current_revision, updated_at, pinned, valid_from, valid_until, invalid_reason, superseded_by, possibly_stale, project_id FROM memory WHERE id = ?1",
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

fn dream_settings_from(conn: &Connection) -> Result<DreamSettings, MemoryError> {
    let mut map = std::collections::HashMap::new();
    let mut stmt = conn.prepare("SELECT key, value FROM kv WHERE key LIKE 'dream.%'")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (key, value) = row?;
        map.insert(key, value);
    }
    let mut settings = DreamSettings::default();
    if let Some(value) = map.get("dream.enabled") {
        settings.enabled = value == "true";
    }
    if let Some(value) = map.get("dream.permit") {
        settings.permit = value == "true";
    }
    if let Some(value) = map.get("dream.interval_hours") {
        settings.interval_hours = value.parse().unwrap_or(settings.interval_hours);
    }
    if let Some(value) = map.get("dream.min_events") {
        settings.min_events = value.parse().unwrap_or(settings.min_events);
    }
    if let Some(value) = map.get("dream.last_success_at") {
        settings.last_success_at = value.parse().unwrap_or(settings.last_success_at);
    }
    if let Some(value) = map.get("dream.last_attempt_at") {
        settings.last_attempt_at = value.parse().unwrap_or(settings.last_attempt_at);
    }
    if let Some(value) = map.get("dream.last_read_seq") {
        settings.last_read_seq = value.parse().unwrap_or(settings.last_read_seq);
    }
    if let Some(value) = map.get("dream.permit_card_at") {
        settings.permit_card_at = value.parse().unwrap_or(settings.permit_card_at);
    }
    Ok(settings)
}

fn dream_run_row(conn: &Connection, id: &str) -> Result<Option<DreamRun>, MemoryError> {
    conn.query_row(
        "SELECT id, started_at, ended_at, outcome, model, usage_json, report_json, error FROM dream_run WHERE id = ?1",
        params![id],
        dream_run_from_row,
    )
    .optional()
    .map_err(MemoryError::Sqlite)
}

fn dream_run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DreamRun> {
    Ok(DreamRun {
        id: row.get(0)?,
        started_at: row.get(1)?,
        ended_at: row.get(2)?,
        outcome: row.get(3)?,
        model: row.get(4)?,
        usage_json: row.get(5)?,
        report_json: row.get(6)?,
        error: row.get(7)?,
    })
}

#[allow(clippy::too_many_arguments)]
fn scope_of(op: &WriteOp) -> Scope {
    match op {
        WriteOp::Remember { .. } | WriteOp::Note { .. } => Scope::Personal,
        _ => Scope::Personal,
    }
}

fn candidates_in(
    conn: &Connection,
    scope: Scope,
    keys: &[MemoryKey],
) -> Result<Vec<RelationCandidate>, MemoryError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut clauses = Vec::new();
    let mut values: Vec<String> = Vec::new();
    for key in keys {
        if key.value.trim().is_empty() {
            continue;
        }
        values.push(key.value.trim().to_owned());
        clauses.push("k.value = ?".to_owned());
    }
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT DISTINCT m.id, m.title, m.summary
         FROM memory m JOIN memory_key k ON k.memory_id = m.id
         WHERE m.status = 'active' AND m.invalid_reason IS NULL AND m.scope = ?
           AND ({})
         ORDER BY m.updated_at DESC LIMIT 5",
        clauses.join(" OR ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params = std::iter::once(scope.as_str().to_owned()).chain(values);
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(RelationCandidate {
            id: row.get(0)?,
            title: row.get(1)?,
            summary: row.get(2)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(MemoryError::Sqlite)
}

fn invalid_reason_of(conn: &Connection, id: &str) -> Result<Option<String>, MemoryError> {
    let reason: Option<Option<String>> = conn
        .query_row(
            "SELECT invalid_reason FROM memory WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(reason.flatten())
}

fn ensure_active(conn: &Connection, id: &str) -> Result<MemoryItem, WriteError> {
    let item = item_by_id(conn, id)?.filter(|item| {
        item.status == "active" && invalid_reason_of(conn, &item.id).ok().flatten().is_none()
    });
    item.ok_or_else(|| WriteError::InvalidRelation(format!("关系目标不存在或已失效：{id}")))
}

fn add_source_row(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
    source_kind: &str,
    source_ref: &str,
    quote: Option<&str>,
    now: i64,
) -> Result<(), MemoryError> {
    if source_kind.trim().is_empty() && source_ref.trim().is_empty() {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO memory_source (memory_id, source_kind, source_ref, quote, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(memory_id, source_kind, source_ref)
         DO UPDATE SET quote = excluded.quote, added_at = excluded.added_at",
        params![memory_id, source_kind, source_ref, quote, now],
    )?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "创建条目时字段都是事实，集中一处写"
)]
fn create_with_details(
    tx: &rusqlite::Transaction<'_>,
    layer: Layer,
    binding: Binding,
    scope: Scope,
    title: &str,
    summary: &str,
    body: &str,
    source_kind: &str,
    source_ref: &str,
    quote: Option<&str>,
    keys: &[MemoryKey],
    now: i64,
    actor: &str,
) -> Result<MemoryItem, MemoryError> {
    let item = create_item(
        tx,
        layer,
        binding,
        scope,
        title,
        summary,
        body,
        source_kind,
        source_ref,
        now,
        None,
        actor,
    )?;
    add_source_row(tx, &item.id, source_kind, source_ref, quote, now)?;
    replace_keys_in(tx, &item.id, keys)?;
    item_by_id(tx, &item.id)?.ok_or_else(|| MemoryError::NotFound(item.id))
}

fn replace_keys_in(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
    keys: &[MemoryKey],
) -> Result<(), MemoryError> {
    tx.execute(
        "DELETE FROM memory_key WHERE memory_id = ?1",
        params![memory_id],
    )?;
    let mut values = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        let value = key.value.trim();
        if value.is_empty() {
            continue;
        }
        tx.execute(
            "INSERT INTO memory_key (memory_id, key_no, kind, value) VALUES (?1, ?2, ?3, ?4)",
            params![
                memory_id,
                i64::try_from(index).unwrap_or(i64::MAX),
                key.kind.as_str(),
                value
            ],
        )?;
        values.push(value.to_owned());
    }
    update_fts_keys(tx, memory_id, &values.join(" "))
}

fn put_card_in(
    tx: &rusqlite::Transaction<'_>,
    kind: &str,
    status: &str,
    payload: &serde_json::Value,
) -> Result<MemoryCardRecord, MemoryError> {
    let card_ref = next_id("card");
    tx.execute(
        "INSERT INTO memory_card (card_ref, kind, status, payload_json) VALUES (?1, ?2, ?3, ?4)",
        params![card_ref, kind, status, payload.to_string()],
    )?;
    Ok(MemoryCardRecord {
        card_ref,
        kind: kind.to_owned(),
        status: status.to_owned(),
        payload: payload.clone(),
    })
}

fn card_in(conn: &Connection, card_ref: &str) -> Result<Option<MemoryCardRecord>, MemoryError> {
    conn.query_row(
        "SELECT card_ref, kind, status, payload_json FROM memory_card WHERE card_ref = ?1",
        params![card_ref],
        |row| {
            let payload: String = row.get(3)?;
            Ok(MemoryCardRecord {
                card_ref: row.get(0)?,
                kind: row.get(1)?,
                status: row.get(2)?,
                payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
            })
        },
    )
    .optional()
    .map_err(MemoryError::Sqlite)
}

fn invalidate_item(
    tx: &rusqlite::Transaction<'_>,
    old_id: &str,
    new_id: &str,
    now: i64,
) -> Result<(), MemoryError> {
    let changed = tx.execute(
        "UPDATE memory SET invalid_reason = 'replaced', valid_until = ?1, superseded_by = ?2 WHERE id = ?3",
        params![now, new_id, old_id],
    )?;
    if changed == 0 {
        return Err(MemoryError::NotFound(old_id.to_owned()));
    }
    Ok(())
}

fn promote_replacement(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    now: i64,
    actor: &str,
) -> Result<(), MemoryError> {
    let item = item_by_id(tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()))?;
    let revision = item.current_revision + 1;
    tx.execute(
        "UPDATE memory SET layer = 'entry', binding = 'binding', pinned = 1, updated_at = ?1, current_revision = ?2 WHERE id = ?3",
        params![now, revision as i64, id],
    )?;
    tx.execute(
        "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, actor, created_at, valid_from)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'replaced_by_user', ?8, ?9, ?9)",
        params![id, revision as i64, item.title, item.summary, item.body, item.source_kind, item.source_ref, actor, now],
    )?;
    // 新的成为普通条目：FTS 的 kind 跟着层走。
    fts_put(tx, id, Layer::Entry, &item.title, &item.body)?;
    Ok(())
}

fn apply_about_edits(
    tx: &rusqlite::Transaction<'_>,
    payload: &AboutDraftPayload,
    card_ref: &str,
    now: i64,
    actor: &str,
) -> Result<(), MemoryError> {
    for edit in &payload.edits {
        match edit.op.as_str() {
            "add" => {
                let Some(text) = edit.text.as_deref().filter(|text| !text.trim().is_empty()) else {
                    continue;
                };
                let title: String = text.chars().take(20).collect();
                let item = create_item(
                    tx,
                    Layer::AboutYou,
                    Binding::Binding,
                    Scope::Personal,
                    &title,
                    "",
                    text,
                    "about_draft",
                    card_ref,
                    now,
                    None,
                    actor,
                )?;
                add_source_row(
                    tx,
                    &item.id,
                    "about_draft",
                    card_ref,
                    Some(&payload.quote),
                    now,
                )?;
            }
            "replace" => {
                let Some(id) = edit.about_id.as_deref() else {
                    continue;
                };
                let Some(text) = edit.text.as_deref().filter(|text| !text.trim().is_empty()) else {
                    continue;
                };
                let Some(current) = item_by_id(tx, id)? else {
                    continue;
                };
                let item = correct_item(
                    tx,
                    id,
                    &current.title,
                    &current.summary,
                    text,
                    "about_draft",
                    now,
                    actor,
                )?;
                add_source_row(
                    tx,
                    &item.id,
                    "about_draft",
                    card_ref,
                    Some(&payload.quote),
                    now,
                )?;
            }
            "delete" => {
                if let Some(id) = edit.about_id.as_deref()
                    && item_by_id(tx, id)?.is_some()
                {
                    delete_to_quarantine(tx, id, "about_draft", now)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "创建条目时字段都是事实，集中一处写"
)]
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
    actor: &str,
) -> Result<MemoryItem, MemoryError> {
    let id = next_id("mem");
    tx.execute(
        "INSERT INTO memory (id, layer, binding, scope, title, summary, body, source_kind, source_ref, created_at, updated_at, status, current_revision, valid_from) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, 'active', 1, ?10)",
        params![id, layer.as_str(), binding.as_str(), scope.as_str(), title, summary, body, source_kind, source_ref, now],
    )?;
    tx.execute(
        "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, dream_run_id, actor, created_at, valid_from) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, 'created', ?7, ?8, ?9, ?9)",
        params![id, title, summary, body, source_kind, source_ref, dream_run, actor, now],
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
        "INSERT INTO memory_fts (kind, memory_id, title, body, source_json, keys) VALUES (?1, ?2, ?3, ?4, '{}', '')",
        params![layer.as_str(), id, title, body],
    )?;
    Ok(())
}

fn update_fts_keys(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    keys: &str,
) -> Result<(), MemoryError> {
    tx.execute(
        "UPDATE memory_fts SET keys = ?1 WHERE memory_id = ?2",
        params![keys, id],
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
        "INSERT INTO memory_fts (kind, memory_id, title, body, source_json, keys) VALUES ('diary', ?1, '', ?2, '{}', '')",
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

#[allow(clippy::too_many_arguments)]
fn correct_item(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    title: &str,
    summary: &str,
    body: &str,
    reason: &str,
    now: i64,
    actor: &str,
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
        "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, actor, created_at, valid_from) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![id, revision as i64, title, summary, body, current.source_kind, current.source_ref, reason, actor, now],
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
    if let Some(new_id) = item.superseded_by.clone() {
        // 被替换失效的：恢复旧条，新条留着。
        tx.execute(
            "UPDATE memory SET valid_until = NULL, invalid_reason = NULL, superseded_by = NULL, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        tx.execute(
            "DELETE FROM memory_relation WHERE memory_id = ?1 AND related_id = ?2 AND kind = 'replaces'",
            params![new_id, id],
        )?;
        return item_by_id(tx, id)?.ok_or_else(|| MemoryError::NotFound(id.to_owned()));
    }
    if item.status != "quarantine" {
        return Err(MemoryError::Refused("只有隔离区或被替换的记忆能恢复"));
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
        "user",
    )
}

fn dream_validation_error(code: &'static str, index: usize, detail: String) -> MemoryError {
    MemoryError::DreamValidation(crate::error::DreamValidationError {
        code,
        message: format!("第 {} 处：{detail}", index + 1),
    })
}

type DreamTargetRow = (String, String, i64, String, String, String, Option<String>);

fn normalized_title(value: &str) -> String {
    value.trim().to_lowercase()
}

fn normalized_content(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn validate_dream_changes(
    tx: &rusqlite::Transaction<'_>,
    changes: &[DreamChangeInput],
) -> Result<(), MemoryError> {
    use std::collections::HashSet;
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut seen_new_titles: HashSet<String> = HashSet::new();
    for (index, change) in changes.iter().enumerate() {
        if change.reason.trim().is_empty() {
            return Err(dream_validation_error(
                "missing_reason",
                index,
                "每处改动都要写一句 reason".to_owned(),
            ));
        }
        if change.title.trim().is_empty() && !matches!(change.action, DreamAction::Retire) {
            return Err(dream_validation_error(
                "invalid_op",
                index,
                "除 retire 外 title 不能为空".to_owned(),
            ));
        }
        let Some(memory_id) = change.memory_id.as_deref() else {
            if !matches!(change.action, DreamAction::NewNote) {
                return Err(dream_validation_error(
                    "invalid_op",
                    index,
                    "没有 memory_id 的改动只能是 new_note".to_owned(),
                ));
            }
            // 用户删过/撤销过的内容，不换个新笔记写回来（去空白、忽略大小写）。
            let wanted = normalized_content(&format!("{}{}", change.title, change.body));
            let mut removed = tx.prepare(
                "SELECT title, body, status, invalid_reason FROM memory WHERE layer = 'note'",
            )?;
            let rows = removed.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            for row in rows {
                let (old_title, old_body, status, invalid_reason) = row?;
                let gone = status != "active" || invalid_reason.as_deref() == Some("deleted");
                if gone && normalized_content(&format!("{old_title}{old_body}")) == wanted {
                    return Err(dream_validation_error(
                        "user_removed",
                        index,
                        "这段内容用户删过，不写回".to_owned(),
                    ));
                }
            }
            let title = normalized_title(&change.title);
            if title.is_empty() {
                return Err(dream_validation_error(
                    "title_collision",
                    index,
                    "新笔记标题不能为空".to_owned(),
                ));
            }
            if seen_new_titles.contains(&title) {
                return Err(dream_validation_error(
                    "duplicate_change",
                    index,
                    "同一轮里新笔记标题重复".to_owned(),
                ));
            }
            seen_new_titles.insert(title.clone());
            let collision: Option<String> = tx
                .query_row(
                    "SELECT id FROM memory WHERE layer = 'note' AND status = 'active' AND lower(trim(title)) = ?1 LIMIT 1",
                    params![title],
                    |row| row.get(0),
                )
                .optional()?;
            if collision.is_some() {
                return Err(dream_validation_error(
                    "title_collision",
                    index,
                    format!("已有同名笔记「{}」", change.title.trim()),
                ));
            }
            continue;
        };
        if matches!(change.action, DreamAction::NewNote) {
            return Err(dream_validation_error(
                "invalid_op",
                index,
                "new_note 不能带 memory_id".to_owned(),
            ));
        }
        if !seen_ids.insert(memory_id.to_owned()) {
            return Err(dream_validation_error(
                "duplicate_change",
                index,
                format!("同一轮里 {memory_id} 改了不止一次"),
            ));
        }
        let row: Option<DreamTargetRow> = tx
            .query_row(
                "SELECT layer, status, current_revision, title, summary, body, invalid_reason FROM memory WHERE id = ?1",
                params![memory_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((layer, status, revision, title, summary, body, invalid_reason)) = row else {
            return Err(dream_validation_error(
                "target_missing",
                index,
                format!("没有 {memory_id} 这条笔记"),
            ));
        };
        if status == "quarantine" || invalid_reason.as_deref() == Some("deleted") {
            return Err(dream_validation_error(
                "user_removed",
                index,
                "用户删过/撤销过的内容不写回".to_owned(),
            ));
        }
        if status != "active" {
            return Err(dream_validation_error(
                "target_missing",
                index,
                format!("{memory_id} 不是 active"),
            ));
        }
        if layer != "note" {
            // 让原有 dream_apply 的 Refused 继续负责这条边界。
            continue;
        }
        let latest: Option<(String, String)> = tx
            .query_row(
                "SELECT actor, reason FROM memory_revision WHERE memory_id = ?1 ORDER BY revision DESC LIMIT 1",
                params![memory_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((actor, reason)) = latest
            && actor == "user"
            && reason != "dream_undo"
        {
            return Err(dream_validation_error(
                "user_edited",
                index,
                "用户改过的笔记，做梦不覆盖".to_owned(),
            ));
        }
        if let Some(expected) = change.before_revision
            && expected != u64::try_from(revision).unwrap_or(0)
        {
            return Err(dream_validation_error(
                "revision_mismatch",
                index,
                format!("before_revision={expected}，库里是 {revision}"),
            ));
        }
        let untouched = change.title == title
            && change.summary == summary
            && change.body == body
            && !matches!(change.action, DreamAction::Retire);
        if untouched {
            return Err(dream_validation_error(
                "unchanged",
                index,
                "内容逐字节没变".to_owned(),
            ));
        }
        let next_title = normalized_title(&change.title);
        if !matches!(change.action, DreamAction::Retire) && !next_title.is_empty() {
            let collision: Option<String> = tx
                .query_row(
                    "SELECT id FROM memory WHERE layer = 'note' AND status = 'active' AND id != ?1 AND lower(trim(title)) = ?2 LIMIT 1",
                    params![memory_id, next_title],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(other) = collision {
                return Err(dream_validation_error(
                    "title_collision",
                    index,
                    format!("和已有笔记 {other} 重名"),
                ));
            }
        }
        let source_count: i64 = tx.query_row(
            "SELECT count(*) FROM memory_source WHERE memory_id = ?1",
            params![memory_id],
            |row| row.get(0),
        )?;
        if matches!(change.action, DreamAction::Retire)
            && source_count > 0
            && change.dropped_sources.is_empty()
        {
            return Err(dream_validation_error(
                "dropped_source",
                index,
                "retire 会丢出处，要在 dropped_sources 里列出并写 reason".to_owned(),
            ));
        }
        for ref_name in &change.dropped_sources {
            let exists: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM memory_source WHERE memory_id = ?1 AND source_ref = ?2 LIMIT 1",
                    params![memory_id, ref_name],
                    |row| row.get(0),
                )
                .optional()?;
            if exists.is_none() {
                return Err(dream_validation_error(
                    "dropped_source",
                    index,
                    format!("{memory_id} 没有出处 {ref_name}"),
                ));
            }
        }
    }
    Ok(())
}

fn dream_apply(
    tx: &rusqlite::Transaction<'_>,
    run: &crate::types::DreamRunInput,
    changes: &[DreamChangeInput],
    now: i64,
) -> Result<WriteOutcome, MemoryError> {
    validate_dream_changes(tx, changes)?;
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
                    "dream",
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
        if item.layer != Layer::Note {
            return Err(MemoryError::Refused("做梦只能改笔记"));
        }
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
            "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, dream_run_id, actor, created_at, valid_from) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'dream', ?10, ?10)",
            params![memory_id, after as i64, title, summary, body, item.source_kind, item.source_ref, change.reason, run.id, now],
        )?;
        for ref_name in &change.dropped_sources {
            tx.execute(
                "DELETE FROM memory_source WHERE memory_id = ?1 AND source_ref = ?2",
                params![memory_id, ref_name],
            )?;
        }
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
    actor: &str,
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
        if item.layer != Layer::Note {
            return Err(MemoryError::Refused("做梦只能撤销笔记"));
        }
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
            "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, undo_of, actor, created_at, valid_from) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'dream_undo', ?8, ?9, ?10, ?10)",
            params![id, next as i64, title, summary, body, item.source_kind, item.source_ref, after, actor, now],
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
            "SELECT id FROM memory WHERE status = 'active' AND invalid_reason IS NULL AND (title LIKE ?1 ESCAPE '\\' OR body LIKE ?1 ESCAPE '\\') ORDER BY updated_at DESC LIMIT ?2",
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
            "SELECT id FROM diary_entry WHERE text LIKE ?1 ESCAPE '\\' ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit], |row| row.get::<_, String>(0))?;
        for id in rows {
            if let Some(entry) = diary_by_id(conn, &id?)? {
                hits.push(SearchHit::Diary(entry));
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
            if let Some(item) = item_by_id(conn, &id?)?
                && item.invalid_reason.is_none()
            {
                hits.push(SearchHit::Memory(item));
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT memory_id FROM memory_fts WHERE memory_fts MATCH ?1 AND kind = 'diary' LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![match_query, limit], |row| row.get::<_, String>(0))?;
        for id in rows {
            if let Some(entry) = diary_by_id(conn, &id?)? {
                hits.push(SearchHit::Diary(entry));
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

fn search_hit_key(hit: &SearchHit) -> String {
    match hit {
        SearchHit::Memory(item) => format!("memory:{}", item.id),
        SearchHit::Diary(entry) => format!("diary:{}", entry.id),
        SearchHit::Conversation {
            conversation_id,
            seq,
            ..
        } => format!("conversation:{conversation_id}:{seq}"),
    }
}

fn is_derived_hit(hit: &SearchHit) -> bool {
    match hit {
        SearchHit::Memory(item) => matches!(item.layer, Layer::Note),
        SearchHit::Diary(_) => true,
        SearchHit::Conversation { .. } => false,
    }
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
