//! 日历库：一个 SQLite 连接放在锁后；约定/版本/运行记录三张表，事务里写要么全成要么全不成。
//!
//! 对外入口：open/open_with_clock、约定的建/改/暂停/恢复/取消/结束、运行记录的排队与结果推进。
//! 规则和复核分别在 schedule/review 模块，这里只负责存与状态迁移。

use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::CalendarError;
use crate::types::{
    Appointment, AppointmentPatch, AppointmentStatus, AppointmentVersion, Binding, Clock,
    CycleSpec, NewAppointment, NewRun, Run, RunResult, Schedule, TriggerKind, WatchSource,
};

/// 日历库 schema 版本；加字段/表时升。
pub const SCHEMA_VERSION: u64 = 2;
const SCHEMA_KEY: &str = "schema_version";

const CREATE_TABLES: &str = "
    CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS appointment (
        id TEXT PRIMARY KEY,
        status TEXT NOT NULL,
        version INTEGER NOT NULL,
        title TEXT NOT NULL,
        condition TEXT NOT NULL,
        rule TEXT NOT NULL,
        timezone TEXT NOT NULL,
        start_at INTEGER NOT NULL,
        binding_kind TEXT NOT NULL,
        binding_ref TEXT NOT NULL,
        watch_kind TEXT,
        watch_target TEXT,
        cycle_task_id TEXT,
        cycle_max_rounds INTEGER,
        next_trigger_at INTEGER,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS appointment_version (
        appointment_id TEXT NOT NULL,
        version INTEGER NOT NULL,
        title TEXT NOT NULL,
        condition TEXT NOT NULL,
        rule TEXT NOT NULL,
        timezone TEXT NOT NULL,
        start_at INTEGER NOT NULL,
        binding_kind TEXT NOT NULL,
        binding_ref TEXT NOT NULL,
        watch_kind TEXT,
        watch_target TEXT,
        cycle_task_id TEXT,
        cycle_max_rounds INTEGER,
        reason TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (appointment_id, version)
    );
    CREATE TABLE IF NOT EXISTS run (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        appointment_id TEXT NOT NULL,
        nominal_at INTEGER NOT NULL,
        manual INTEGER NOT NULL,
        trigger_kind TEXT NOT NULL,
        result TEXT NOT NULL,
        reason TEXT NOT NULL DEFAULT '',
        merged_count INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        sent_turn INTEGER,
        UNIQUE (appointment_id, nominal_at, manual)
    );
    CREATE INDEX IF NOT EXISTS run_appointment ON run(appointment_id, nominal_at);
    CREATE TABLE IF NOT EXISTS watch_state (
        appointment_id TEXT PRIMARY KEY,
        content TEXT NOT NULL,
        hash TEXT NOT NULL,
        attachment_id TEXT,
        updated_at INTEGER NOT NULL
    );
";

pub struct CalendarStore {
    conn: Mutex<Connection>,
    clock: Clock,
}

impl CalendarStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, CalendarError> {
        Self::open_with_clock(path, Arc::new(now_millis))
    }

    pub fn open_with_clock(
        path: impl AsRef<std::path::Path>,
        clock: Clock,
    ) -> Result<Self, CalendarError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(CREATE_TABLES)?;
        let version: Option<u64> = conn
            .query_row(
                "SELECT value FROM kv WHERE key = ?1",
                params![SCHEMA_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| value.parse().ok());
        if let Some(version) = version
            && version > SCHEMA_VERSION
        {
            return Err(CalendarError::InvalidInput(format!(
                "日历库 schema 太新：{version} > {SCHEMA_VERSION}"
            )));
        }
        if version.is_none() {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)",
                params![SCHEMA_KEY, SCHEMA_VERSION.to_string()],
            )?;
        } else if version.is_some_and(|value| value < SCHEMA_VERSION) {
            // v1 → v2：加 watch_state（上一份内容+哈希）。
            conn.execute(
                "UPDATE kv SET value = ?1 WHERE key = ?2",
                params![SCHEMA_VERSION.to_string(), SCHEMA_KEY],
            )?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
            clock,
        })
    }

    pub fn create_appointment(&self, draft: NewAppointment) -> Result<Appointment, CalendarError> {
        draft.schedule.validate()?;
        let now = (self.clock)();
        let next_trigger_at = draft.schedule.next_after(now)?;
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let transaction = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id: String =
            transaction.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
        transaction.execute(
            "INSERT INTO appointment (
                id, status, version, title, condition, rule, timezone, start_at,
                binding_kind, binding_ref, watch_kind, watch_target, cycle_task_id,
                cycle_max_rounds, next_trigger_at, created_at, updated_at
            ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
            params![
                id,
                AppointmentStatus::Active.as_str(),
                draft.title,
                draft.condition,
                draft.schedule.rule,
                draft.schedule.timezone,
                draft.schedule.start_at,
                binding_kind(&draft.binding),
                binding_ref(&draft.binding),
                watch_kind(&draft.watch),
                watch_target(&draft.watch),
                cycle_task(&draft.cycle),
                cycle_max(&draft.cycle),
                next_trigger_at,
                now,
            ],
        )?;
        insert_version(&transaction, &id, 1, &draft, "create", now)?;
        transaction.commit()?;
        Ok(Appointment {
            id,
            status: AppointmentStatus::Active,
            version: 1,
            title: draft.title,
            condition: draft.condition,
            schedule: draft.schedule,
            binding: draft.binding,
            watch: draft.watch,
            cycle: draft.cycle,
            next_trigger_at,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn appointment(&self, id: &str) -> Result<Option<Appointment>, CalendarError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        load_appointment(&conn, id)
            .map(Some)
            .or_else(|error| match error {
                CalendarError::AppointmentNotFound(_) => Ok(None),
                other => Err(other),
            })
    }

    pub fn list_appointments(&self) -> Result<Vec<Appointment>, CalendarError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let mut statement = conn.prepare("SELECT id FROM appointment ORDER BY created_at, id")?;
        let ids: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        let mut appointments = Vec::with_capacity(ids.len());
        for id in ids {
            appointments.push(load_appointment(&conn, &id)?);
        }
        Ok(appointments)
    }

    pub fn update_appointment(
        &self,
        id: &str,
        expected_version: u32,
        patch: AppointmentPatch,
        reason: &str,
    ) -> Result<Appointment, CalendarError> {
        let now = (self.clock)();
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let transaction = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut appointment = load_appointment(&transaction, id)?;
        if appointment.version != expected_version {
            return Err(CalendarError::StaleVersion {
                expected: expected_version,
                actual: appointment.version,
            });
        }
        if let Some(title) = patch.title {
            appointment.title = title;
        }
        if let Some(condition) = patch.condition {
            appointment.condition = condition;
        }
        if let Some(schedule) = patch.schedule {
            appointment.schedule = schedule;
        }
        if let Some(binding) = patch.binding {
            appointment.binding = binding;
        }
        if let Some(watch) = patch.watch {
            appointment.watch = watch;
        }
        if let Some(cycle) = patch.cycle {
            appointment.cycle = cycle;
        }
        appointment.schedule.validate()?;
        appointment.next_trigger_at = appointment.schedule.next_after(now)?;
        appointment.version += 1;
        appointment.updated_at = now;
        transaction.execute(
            "UPDATE appointment SET
                status = ?2, version = ?3, title = ?4, condition = ?5, rule = ?6,
                timezone = ?7, start_at = ?8, binding_kind = ?9, binding_ref = ?10,
                watch_kind = ?11, watch_target = ?12, cycle_task_id = ?13,
                cycle_max_rounds = ?14, next_trigger_at = ?15, updated_at = ?16
             WHERE id = ?1",
            params![
                appointment.id,
                appointment.status.as_str(),
                appointment.version,
                appointment.title,
                appointment.condition,
                appointment.schedule.rule,
                appointment.schedule.timezone,
                appointment.schedule.start_at,
                binding_kind(&appointment.binding),
                binding_ref(&appointment.binding),
                watch_kind(&appointment.watch),
                watch_target(&appointment.watch),
                cycle_task(&appointment.cycle),
                cycle_max(&appointment.cycle),
                appointment.next_trigger_at,
                now,
            ],
        )?;
        insert_version(
            &transaction,
            &appointment.id,
            appointment.version,
            &NewAppointment {
                title: appointment.title.clone(),
                condition: appointment.condition.clone(),
                schedule: appointment.schedule.clone(),
                binding: appointment.binding.clone(),
                watch: appointment.watch.clone(),
                cycle: appointment.cycle.clone(),
            },
            reason,
            now,
        )?;
        transaction.commit()?;
        Ok(appointment)
    }

    pub fn set_status(
        &self,
        id: &str,
        status: AppointmentStatus,
    ) -> Result<Appointment, CalendarError> {
        let now = (self.clock)();
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let transaction = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut appointment = load_appointment(&transaction, id)?;
        if appointment.status == status {
            return Ok(appointment);
        }
        let allowed = match appointment.status {
            AppointmentStatus::Active => matches!(
                status,
                AppointmentStatus::Paused | AppointmentStatus::Cancelled | AppointmentStatus::Ended
            ),
            AppointmentStatus::Paused => matches!(
                status,
                AppointmentStatus::Active | AppointmentStatus::Cancelled | AppointmentStatus::Ended
            ),
            AppointmentStatus::Ended | AppointmentStatus::Cancelled => false,
        };
        if !allowed {
            return Err(CalendarError::InvalidStatus(format!(
                "{} → {}",
                appointment.status.as_str(),
                status.as_str()
            )));
        }
        let next_trigger_at = match status {
            AppointmentStatus::Active => appointment.schedule.next_after(now)?,
            AppointmentStatus::Paused | AppointmentStatus::Cancelled | AppointmentStatus::Ended => {
                None
            }
        };
        transaction.execute(
            "UPDATE appointment SET status = ?2, next_trigger_at = ?3, updated_at = ?4 WHERE id = ?1",
            params![appointment.id, status.as_str(), next_trigger_at, now],
        )?;
        transaction.commit()?;
        appointment.status = status;
        appointment.next_trigger_at = next_trigger_at;
        appointment.updated_at = now;
        Ok(appointment)
    }

    /// T064：带原因暂停循环约定（原因进版本历史；恢复时清不掉历史，但只在暂停态读）。
    pub fn pause_with_reason(&self, id: &str, reason: &str) -> Result<Appointment, CalendarError> {
        let now = (self.clock)();
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let transaction = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut appointment = load_appointment(&transaction, id)?;
        if appointment.status != AppointmentStatus::Active {
            return Ok(appointment);
        }
        appointment.version += 1;
        appointment.updated_at = now;
        insert_version(
            &transaction,
            &appointment.id,
            appointment.version,
            &NewAppointment {
                title: appointment.title.clone(),
                condition: appointment.condition.clone(),
                schedule: appointment.schedule.clone(),
                binding: appointment.binding.clone(),
                watch: appointment.watch.clone(),
                cycle: appointment.cycle.clone(),
            },
            &format!("循环约定暂停：{reason}"),
            now,
        )?;
        transaction.execute(
            "UPDATE appointment SET status = 'paused', version = ?2, next_trigger_at = NULL, updated_at = ?3 WHERE id = ?1",
            params![appointment.id, appointment.version, now],
        )?;
        transaction.commit()?;
        appointment.status = AppointmentStatus::Paused;
        appointment.next_trigger_at = None;
        Ok(appointment)
    }

    /// 约定最新版本的变更原因；暂停原因从这里读。
    pub fn latest_version_reason(&self, id: &str) -> Result<Option<String>, CalendarError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        conn.query_row(
            "SELECT reason FROM appointment_version WHERE appointment_id = ?1 ORDER BY version DESC LIMIT 1",
            params![id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(CalendarError::from)
    }

    /// 活跃约定里最近的 next_trigger；运行时计时器只等这一条。
    pub fn next_wake_at(&self) -> Result<Option<i64>, CalendarError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        conn.query_row(
            "SELECT MIN(next_trigger_at) FROM appointment
             WHERE status = 'active' AND next_trigger_at IS NOT NULL",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(CalendarError::from)
    }

    /// 到点该复核的活跃约定（next_trigger_at <= now）。
    pub fn due_appointments(&self, now: i64) -> Result<Vec<Appointment>, CalendarError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let mut statement = conn.prepare(
            "SELECT id FROM appointment
             WHERE status = 'active' AND next_trigger_at IS NOT NULL AND next_trigger_at <= ?1
             ORDER BY next_trigger_at, id",
        )?;
        let ids: Vec<String> = statement
            .query_map(params![now], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        let mut appointments = Vec::with_capacity(ids.len());
        for id in ids {
            appointments.push(load_appointment(&conn, &id)?);
        }
        Ok(appointments)
    }

    /// 运行时推进下次触发；不算内容改版（版本号不动）。
    pub fn set_next_trigger(
        &self,
        id: &str,
        next_trigger_at: Option<i64>,
    ) -> Result<Appointment, CalendarError> {
        let now = (self.clock)();
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let transaction = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let appointment = load_appointment(&transaction, id)?;
        transaction.execute(
            "UPDATE appointment SET next_trigger_at = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, next_trigger_at, now],
        )?;
        transaction.commit()?;
        let mut appointment = appointment;
        appointment.next_trigger_at = next_trigger_at;
        appointment.updated_at = now;
        Ok(appointment)
    }

    /// 盯变化的上一份内容和哈希；没查过返回 None。
    pub fn watch_state(
        &self,
        appointment_id: &str,
    ) -> Result<Option<crate::types::WatchState>, CalendarError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        conn.query_row(
            "SELECT content, hash, attachment_id, updated_at FROM watch_state WHERE appointment_id = ?1",
            params![appointment_id],
            |row| {
                Ok(crate::types::WatchState {
                    content: row.get(0)?,
                    hash: row.get(1)?,
                    attachment_id: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(CalendarError::from)
    }

    /// 存盯变化的上一份内容；失败不调用这里（不更新上次内容）。
    pub fn save_watch_state(
        &self,
        appointment_id: &str,
        content: &str,
        hash: &str,
        attachment_id: Option<&str>,
    ) -> Result<(), CalendarError> {
        let now = (self.clock)();
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let transaction = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO watch_state (appointment_id, content, hash, attachment_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(appointment_id) DO UPDATE SET
                content = excluded.content, hash = excluded.hash,
                attachment_id = excluded.attachment_id, updated_at = excluded.updated_at",
            params![appointment_id, content, hash, attachment_id, now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn versions(&self, id: &str) -> Result<Vec<AppointmentVersion>, CalendarError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let mut statement = conn.prepare(
            "SELECT appointment_id, version, title, condition, rule, timezone, start_at,
                    binding_kind, binding_ref, watch_kind, watch_target, cycle_task_id,
                    cycle_max_rounds, reason, created_at
             FROM appointment_version WHERE appointment_id = ?1 ORDER BY version",
        )?;
        let rows = statement
            .query_map(params![id], |row| {
                let binding_kind: String = row.get(7)?;
                let binding_ref: String = row.get(8)?;
                let watch_kind: Option<String> = row.get(9)?;
                let watch_target: Option<String> = row.get(10)?;
                let cycle_task_id: Option<String> = row.get(11)?;
                let cycle_max_rounds: Option<i64> = row.get(12)?;
                Ok(AppointmentVersion {
                    appointment_id: row.get(0)?,
                    version: row.get(1)?,
                    title: row.get(2)?,
                    condition: row.get(3)?,
                    schedule: Schedule {
                        rule: row.get(4)?,
                        timezone: row.get(5)?,
                        start_at: row.get(6)?,
                    },
                    binding: parse_binding(&binding_kind, &binding_ref)?,
                    watch: parse_watch(watch_kind, watch_target)?,
                    cycle: parse_cycle(cycle_task_id, cycle_max_rounds)?,
                    reason: row.get(13)?,
                    created_at: row.get(14)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 排队一条运行；同一 (约定 id, 名义时间, 是否手动) 已经有记录就直接返回旧的，不重复送。
    pub fn record_run(&self, run: NewRun) -> Result<(Run, bool), CalendarError> {
        let now = (self.clock)();
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let transaction = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM appointment WHERE id = ?1",
                params![run.appointment_id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(CalendarError::AppointmentNotFound(run.appointment_id));
        }
        let manual = i64::from(run.manual);
        let changed = transaction.execute(
            "INSERT OR IGNORE INTO run (
                appointment_id, nominal_at, manual, trigger_kind, result, reason,
                merged_count, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                run.appointment_id,
                run.nominal_at,
                manual,
                run.trigger.as_str(),
                RunResult::Queued.as_str(),
                run.reason,
                run.merged_count,
                now,
            ],
        )?;
        let stored = load_run_by_identity(
            &transaction,
            &run.appointment_id,
            run.nominal_at,
            run.manual,
        )?;
        transaction.commit()?;
        Ok((stored, changed == 1))
    }

    pub fn finish_run(
        &self,
        run_id: i64,
        result: RunResult,
        reason: &str,
        sent_turn: Option<i64>,
    ) -> Result<Run, CalendarError> {
        let now = (self.clock)();
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let transaction = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut run) = load_run(&transaction, run_id)? else {
            return Err(CalendarError::RunNotFound(run_id));
        };
        if run.result == result {
            return Ok(run);
        }
        if !run.result.can_move_to(result) {
            return Err(CalendarError::InvalidRunResult {
                from: run.result.as_str().to_owned(),
                to: result.as_str().to_owned(),
            });
        }
        transaction.execute(
            "UPDATE run SET result = ?2, reason = ?3, sent_turn = ?4, updated_at = ?5 WHERE id = ?1",
            params![run_id, result.as_str(), reason, sent_turn, now],
        )?;
        transaction.commit()?;
        run.result = result;
        run.reason = reason.to_owned();
        run.sent_turn = sent_turn;
        run.updated_at = now;
        Ok(run)
    }

    pub fn runs(&self, appointment_id: &str) -> Result<Vec<Run>, CalendarError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| CalendarError::InvalidInput("日历库锁坏了".to_owned()))?;
        let mut statement = conn.prepare(
            "SELECT id, appointment_id, nominal_at, manual, trigger_kind, result, reason,
                    merged_count, created_at, updated_at, sent_turn
             FROM run WHERE appointment_id = ?1 ORDER BY id",
        )?;
        let rows = statement
            .query_map(params![appointment_id], map_run)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn binding_kind(binding: &Binding) -> &'static str {
    match binding {
        Binding::Kind => "kind",
        Binding::Task(_) => "task",
    }
}

fn binding_ref(binding: &Binding) -> String {
    match binding {
        Binding::Kind => String::new(),
        Binding::Task(task) => task.clone(),
    }
}

fn parse_binding(kind: &str, reference: &str) -> rusqlite::Result<Binding> {
    match kind {
        "kind" => Ok(Binding::Kind),
        "task" => Ok(Binding::Task(reference.to_owned())),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("未知 binding：{other}"),
            )),
        )),
    }
}

fn watch_kind(watch: &Option<WatchSource>) -> Option<&'static str> {
    match watch {
        None => None,
        Some(WatchSource::Url(_)) => Some("url"),
        Some(WatchSource::File(_)) => Some("file"),
        Some(WatchSource::TaskFacts(_)) => Some("task_facts"),
    }
}

fn watch_target(watch: &Option<WatchSource>) -> Option<String> {
    match watch {
        None => None,
        Some(
            WatchSource::Url(value) | WatchSource::File(value) | WatchSource::TaskFacts(value),
        ) => Some(value.clone()),
    }
}

fn parse_watch(
    kind: Option<String>,
    target: Option<String>,
) -> rusqlite::Result<Option<WatchSource>> {
    let (Some(kind), Some(target)) = (kind, target) else {
        return Ok(None);
    };
    match kind.as_str() {
        "url" => Ok(Some(WatchSource::Url(target))),
        "file" => Ok(Some(WatchSource::File(target))),
        "task_facts" => Ok(Some(WatchSource::TaskFacts(target))),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("未知 watch：{other}"),
            )),
        )),
    }
}

fn cycle_task(cycle: &Option<CycleSpec>) -> Option<String> {
    cycle.as_ref().map(|cycle| cycle.task_id.clone())
}

fn cycle_max(cycle: &Option<CycleSpec>) -> Option<i64> {
    cycle
        .as_ref()
        .and_then(|cycle| cycle.max_rounds)
        .map(i64::from)
}

fn parse_cycle(
    task_id: Option<String>,
    max_rounds: Option<i64>,
) -> rusqlite::Result<Option<CycleSpec>> {
    let Some(task_id) = task_id else {
        return Ok(None);
    };
    Ok(Some(CycleSpec {
        task_id,
        max_rounds: max_rounds.and_then(|value| u32::try_from(value).ok()),
    }))
}

fn insert_version(
    connection: &Connection,
    appointment_id: &str,
    version: u32,
    draft: &NewAppointment,
    reason: &str,
    now: i64,
) -> Result<(), CalendarError> {
    connection.execute(
        "INSERT INTO appointment_version (
            appointment_id, version, title, condition, rule, timezone, start_at,
            binding_kind, binding_ref, watch_kind, watch_target, cycle_task_id,
            cycle_max_rounds, reason, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            appointment_id,
            version,
            draft.title,
            draft.condition,
            draft.schedule.rule,
            draft.schedule.timezone,
            draft.schedule.start_at,
            binding_kind(&draft.binding),
            binding_ref(&draft.binding),
            watch_kind(&draft.watch),
            watch_target(&draft.watch),
            cycle_task(&draft.cycle),
            cycle_max(&draft.cycle),
            reason,
            now,
        ],
    )?;
    Ok(())
}

fn load_appointment(connection: &Connection, id: &str) -> Result<Appointment, CalendarError> {
    let appointment = connection
        .query_row(
            "SELECT id, status, version, title, condition, rule, timezone, start_at,
                    binding_kind, binding_ref, watch_kind, watch_target, cycle_task_id,
                    cycle_max_rounds, next_trigger_at, created_at, updated_at
             FROM appointment WHERE id = ?1",
            params![id],
            |row| {
                let status: String = row.get(1)?;
                let parsed = AppointmentStatus::parse(&status)
                    .ok_or_else(|| rusqlite_error(format!("未知状态：{status}")))?;
                Ok(Appointment {
                    id: row.get(0)?,
                    status: parsed,
                    version: row.get(2)?,
                    title: row.get(3)?,
                    condition: row.get(4)?,
                    schedule: Schedule {
                        rule: row.get(5)?,
                        timezone: row.get(6)?,
                        start_at: row.get(7)?,
                    },
                    binding: parse_binding(&row.get::<_, String>(8)?, &row.get::<_, String>(9)?)?,
                    watch: parse_watch(row.get(10)?, row.get(11)?)?,
                    cycle: parse_cycle(row.get(12)?, row.get(13)?)?,
                    next_trigger_at: row.get(14)?,
                    created_at: row.get(15)?,
                    updated_at: row.get(16)?,
                })
            },
        )
        .optional()?;
    appointment.ok_or_else(|| CalendarError::AppointmentNotFound(id.to_owned()))
}

fn load_run(connection: &Connection, id: i64) -> Result<Option<Run>, CalendarError> {
    connection
        .query_row(
            "SELECT id, appointment_id, nominal_at, manual, trigger_kind, result, reason,
                    merged_count, created_at, updated_at, sent_turn
             FROM run WHERE id = ?1",
            params![id],
            map_run,
        )
        .optional()
        .map_err(CalendarError::from)
}

fn load_run_by_identity(
    connection: &Connection,
    appointment_id: &str,
    nominal_at: i64,
    manual: bool,
) -> Result<Run, CalendarError> {
    connection
        .query_row(
            "SELECT id, appointment_id, nominal_at, manual, trigger_kind, result, reason,
                    merged_count, created_at, updated_at, sent_turn
             FROM run WHERE appointment_id = ?1 AND nominal_at = ?2 AND manual = ?3",
            params![appointment_id, nominal_at, i64::from(manual)],
            map_run,
        )
        .map_err(CalendarError::from)
}

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    let trigger: String = row.get(4)?;
    let result: String = row.get(5)?;
    let manual: i64 = row.get(3)?;
    Ok(Run {
        id: row.get(0)?,
        appointment_id: row.get(1)?,
        nominal_at: row.get(2)?,
        manual: manual != 0,
        trigger: TriggerKind::parse(&trigger)
            .ok_or_else(|| rusqlite_error(format!("未知触发：{trigger}")))?,
        result: RunResult::parse(&result)
            .ok_or_else(|| rusqlite_error(format!("未知运行结果：{result}")))?,
        reason: row.get(6)?,
        merged_count: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        sent_turn: row.get(10)?,
    })
}

fn rusqlite_error(message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    )
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Binding, Schedule};
    use std::sync::Arc;

    const NOW: i64 = 1_790_000_000_000;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn store(directory: &tempfile::TempDir) -> CalendarStore {
        CalendarStore::open_with_clock(directory.path().join("calendar.sqlite"), Arc::new(|| NOW))
            .unwrap()
    }

    fn draft() -> NewAppointment {
        NewAppointment {
            title: "每个工作日早上 9 点列给我".to_owned(),
            condition: "把等我的事列给我".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: NOW,
            },
            binding: Binding::Kind,
            watch: None,
            cycle: None,
        }
    }

    fn new_run(appointment: &Appointment, nominal_at: i64, manual: bool) -> NewRun {
        NewRun {
            appointment_id: appointment.id.clone(),
            nominal_at,
            manual,
            trigger: if manual {
                crate::types::TriggerKind::Manual
            } else {
                crate::types::TriggerKind::Due
            },
            reason: String::new(),
            merged_count: 0,
        }
    }

    #[test]
    fn create_stores_first_version_and_next_trigger() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let appointment = store.create_appointment(draft()).unwrap();
        assert_eq!(appointment.version, 1);
        assert_eq!(appointment.status, AppointmentStatus::Active);
        assert_eq!(appointment.next_trigger_at, Some(NOW + DAY));
        assert_eq!(appointment.created_at, NOW);
        let versions = store.versions(&appointment.id).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[0].reason, "create");
        assert_eq!(store.list_appointments().unwrap().len(), 1);
    }

    #[test]
    fn update_checks_expected_version_and_appends_a_version() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let appointment = store.create_appointment(draft()).unwrap();
        let stale = store.update_appointment(
            &appointment.id,
            99,
            AppointmentPatch {
                title: Some("改成 8 点半".to_owned()),
                ..AppointmentPatch::default()
            },
            "user",
        );
        assert!(matches!(
            stale,
            Err(CalendarError::StaleVersion {
                expected: 99,
                actual: 1
            })
        ));
        let updated = store
            .update_appointment(
                &appointment.id,
                1,
                AppointmentPatch {
                    title: Some("改成 8 点半".to_owned()),
                    schedule: Some(Schedule {
                        rule: "FREQ=DAILY".to_owned(),
                        timezone: "Asia/Shanghai".to_owned(),
                        start_at: NOW,
                    }),
                    ..AppointmentPatch::default()
                },
                "user",
            )
            .unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.title, "改成 8 点半");
        let versions = store.versions(&appointment.id).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[1].version, 2);
        assert_eq!(versions[1].reason, "user");
    }

    #[test]
    fn pause_resume_cancel_and_end_follow_allowed_moves() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let appointment = store.create_appointment(draft()).unwrap();
        let paused = store
            .set_status(&appointment.id, AppointmentStatus::Paused)
            .unwrap();
        assert_eq!(paused.status, AppointmentStatus::Paused);
        assert_eq!(
            store.versions(&appointment.id).unwrap().len(),
            1,
            "暂停不改版本"
        );
        let resumed = store
            .set_status(&appointment.id, AppointmentStatus::Active)
            .unwrap();
        assert_eq!(resumed.status, AppointmentStatus::Active);
        let cancelled = store
            .set_status(&appointment.id, AppointmentStatus::Cancelled)
            .unwrap();
        assert_eq!(cancelled.status, AppointmentStatus::Cancelled);
        assert!(
            store
                .set_status(&appointment.id, AppointmentStatus::Active)
                .is_err(),
            "取消后不复活"
        );
        let other = store.create_appointment(draft()).unwrap();
        let ended = store
            .set_status(&other.id, AppointmentStatus::Ended)
            .unwrap();
        assert_eq!(ended.status, AppointmentStatus::Ended);
        assert!(
            store
                .set_status(&other.id, AppointmentStatus::Paused)
                .is_err(),
            "结束后不能再改状态"
        );
    }

    #[test]
    fn manual_run_does_not_move_next_trigger_and_identity_is_unique() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let appointment = store.create_appointment(draft()).unwrap();
        let before = appointment.next_trigger_at;
        let (first, created) = store.record_run(new_run(&appointment, NOW, true)).unwrap();
        assert!(created);
        let (again, created_again) = store.record_run(new_run(&appointment, NOW, true)).unwrap();
        assert!(!created_again);
        assert_eq!(first.id, again.id, "同一唤醒身份只有一条");
        assert_eq!(store.runs(&appointment.id).unwrap().len(), 1);
        assert_eq!(
            store
                .appointment(&appointment.id)
                .unwrap()
                .unwrap()
                .next_trigger_at,
            before
        );
    }

    #[test]
    fn run_result_only_moves_forward() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let appointment = store.create_appointment(draft()).unwrap();
        let (run, _) = store.record_run(new_run(&appointment, NOW, false)).unwrap();
        let failed = store
            .finish_run(run.id, RunResult::Failed, "没送成：根会话不在", None)
            .unwrap();
        assert_eq!(failed.result, RunResult::Failed);
        let sent = store
            .finish_run(failed.id, RunResult::Sent, "", Some(12))
            .unwrap();
        assert_eq!(sent.result, RunResult::Sent);
        assert_eq!(sent.sent_turn, Some(12));
        assert!(
            store
                .finish_run(sent.id, RunResult::Queued, "", None)
                .is_err(),
            "已送不能退回排着"
        );
        let (other, _) = store
            .record_run(new_run(&appointment, NOW + 1, false))
            .unwrap();
        assert!(
            store
                .finish_run(other.id, RunResult::Skipped, "跳过：约定已暂停", None)
                .is_ok()
        );
    }

    #[test]
    fn same_nominal_time_manual_and_due_are_different_identities() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let appointment = store.create_appointment(draft()).unwrap();
        let (due, _) = store.record_run(new_run(&appointment, NOW, false)).unwrap();
        let (manual, _) = store.record_run(new_run(&appointment, NOW, true)).unwrap();
        assert_ne!(due.id, manual.id, "手动与到点不是同一个唤醒身份");
        assert_eq!(store.runs(&appointment.id).unwrap().len(), 2);
    }

    #[test]
    fn stale_update_does_not_append_a_version() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        let appointment = store.create_appointment(draft()).unwrap();
        assert!(
            store
                .update_appointment(&appointment.id, 99, AppointmentPatch::default(), "stale",)
                .is_err()
        );
        assert_eq!(store.versions(&appointment.id).unwrap().len(), 1);
    }

    #[test]
    fn newer_schema_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("calendar.sqlite");
        drop(CalendarStore::open_with_clock(&path, Arc::new(|| NOW)).unwrap());
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE kv SET value = '99' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        drop(connection);
        let Err(error) = CalendarStore::open_with_clock(&path, Arc::new(|| NOW)) else {
            panic!("newer schema should be rejected");
        };
        assert!(error.to_string().contains("schema 太新"), "{error}");
    }

    #[test]
    fn watch_state_round_trips_and_migrates_from_v1() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("calendar.sqlite");
        let id = {
            let store = CalendarStore::open_with_clock(&path, Arc::new(|| NOW)).unwrap();
            let appointment = store.create_appointment(draft()).unwrap();
            appointment.id
        };
        // 模拟 v1 老库：删表、把版本写回 1，再打开要迁移到 v2。
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("DROP TABLE watch_state;").unwrap();
        connection
            .execute("UPDATE kv SET value = '1' WHERE key = 'schema_version'", [])
            .unwrap();
        drop(connection);
        let store = CalendarStore::open_with_clock(&path, Arc::new(|| NOW)).unwrap();
        assert!(store.watch_state(&id).unwrap().is_none(), "v1 没有旧内容");
        store
            .save_watch_state(&id, "迁移后", "hash-2", None)
            .unwrap();
        let state = store.watch_state(&id).unwrap().expect("迁移后能写读");
        assert_eq!(state.content, "迁移后");
        assert_eq!(state.hash, "hash-2");
    }

    #[test]
    fn reopened_database_keeps_appointment_and_runs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("calendar.sqlite");
        let id = {
            let store = CalendarStore::open_with_clock(&path, Arc::new(|| NOW)).unwrap();
            let appointment = store.create_appointment(draft()).unwrap();
            store.record_run(new_run(&appointment, NOW, false)).unwrap();
            appointment.id
        };
        let store = CalendarStore::open_with_clock(&path, Arc::new(|| NOW)).unwrap();
        let appointment = store.appointment(&id).unwrap().unwrap();
        assert_eq!(appointment.version, 1);
        assert_eq!(store.runs(&id).unwrap().len(), 1);
    }
}
