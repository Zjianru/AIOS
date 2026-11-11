//! SQLite 存储：一个连接放在锁后，同步接口；建任务和排 launch、写报告和排 relay 各在一个事务里。

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Stage;
use crate::TaskError;
use crate::effect::{
    EffectClaim, EffectHealth, EffectKind, EffectPayload, EffectState, FAILURES_BEFORE_ERROR,
};
use crate::id::{AttemptId, BootId, ConversationId, InputId, ReportId, TaskId};
use crate::task::{
    Acceptance, Attempt, AttemptEnd, AttemptTrigger, FrozenPolicy, Input, InputAttachment,
    InputSource, MissingItem, NewInput, NewReport, NewTask, Report, RequirementVersion, Run,
    SayOutcome, Task, TaskPolicy, VersionSource, Worktree, resolve_write_scope,
};

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

/// 立即停在库里的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAttempt {
    /// 这次写下了 stopped，并排了一条停止 relay。
    Stopped,
    /// 之前已经停过；没有重排第二条 relay。
    AlreadyStopped,
    /// 这次 Attempt 已经以别的理由结束；没有改动。
    AlreadyEnded,
}

/// 启动扫描的结果：这次启动认出哪些没结束的 Attempt，各自怎么收尾。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootScan {
    /// 在跑、没交出结果（或报告后又有了新输入）的 Attempt，已记 interrupted 并排 restart。
    pub interrupted: Vec<TaskId>,
    /// 已经交出报告、之后没有新输入的 Attempt，已记 reported 结束。
    pub reported: Vec<TaskId>,
    /// 崩溃前没开出会话的 Attempt，已记 not_opened 收尾。
    pub not_opened: Vec<TaskId>,
}

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
        self.create_task_with(draft, TaskPolicy::default(), now)
    }

    /// 建任务，带创建时冻结的执行策略（写入范围、bash 开关、工作树）。
    pub fn create_task_with(
        &self,
        draft: NewTask,
        policy: TaskPolicy,
        now: i64,
    ) -> Result<Task, TaskError> {
        draft.validate()?;
        let frozen = policy.freeze(&draft.cwd)?;
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = self.insert_task_in(&transaction, draft, &frozen, now)?;
        transaction.commit()?;
        Ok(task)
    }

    /// 按受理幂等键建任务：同一个键再来返回同一个任务，不建第二行、不重排效果。
    /// 建任务和记下键在同一个事务里，崩在中间不会留下「任务建了但键没记」。
    pub fn create_task_once(
        &self,
        draft: NewTask,
        accept_key: &str,
        now: i64,
    ) -> Result<(Task, bool), TaskError> {
        self.create_task_once_with(draft, TaskPolicy::default(), accept_key, now)
    }

    /// 同 `create_task_once`，并冻结创建时的执行策略。
    pub fn create_task_once_with(
        &self,
        draft: NewTask,
        policy: TaskPolicy,
        accept_key: &str,
        now: i64,
    ) -> Result<(Task, bool), TaskError> {
        draft.validate()?;
        if accept_key.trim().is_empty() {
            return Err(TaskError::EmptyAcceptKey);
        }
        let frozen = policy.freeze(&draft.cwd)?;
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key = format!("task-accept:{accept_key}");
        let existing: Option<String> = transaction
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(id) = existing {
            let task = load_task_required(&transaction, &TaskId::from(id))?;
            transaction.commit()?;
            return Ok((task, false));
        }
        let task = self.insert_task_in(&transaction, draft, &frozen, now)?;
        transaction.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)",
            params![key, task.id.as_str()],
        )?;
        transaction.commit()?;
        Ok((task, true))
    }

    fn insert_task_in(
        &self,
        connection: &Connection,
        draft: NewTask,
        policy: &FrozenPolicy,
        now: i64,
    ) -> Result<Task, TaskError> {
        let versions = vec![RequirementVersion {
            write_scope: policy.write_scope.clone(),
            at: now,
            source: VersionSource::TaskAccept,
        }];
        let task = Task {
            id: TaskId::from(generate_id(connection)?),
            title: draft.title,
            requirement: draft.requirement,
            dod: draft.dod,
            cwd: draft.cwd,
            conversation: draft.conversation,
            model: draft.model,
            tools: draft.tools,
            write_scope: policy.write_scope.clone(),
            bash: policy.bash,
            worktree: policy.worktree,
            worktree_state: None,
            writes: Vec::new(),
            code_mode: false,
            runs: Vec::new(),
            reports: Vec::new(),
            inputs: Vec::new(),
            versions,
            acceptance: None,
            missing: Vec::new(),
            created_at: now,
        };
        connection.execute(
            "INSERT INTO task (id, value) VALUES (?1, ?2)",
            params![task.id.as_str(), serde_json::to_string(&task)?],
        )?;
        self.enqueue_effect_in(
            connection,
            &EffectPayload::Launch {
                task_id: task.id.clone(),
            },
            now,
        )?;
        Ok(task)
    }

    /// 列出全部任务 id（启动扫描与诊断用；不按创建时间排序）。
    pub fn task_ids(&self) -> Result<Vec<TaskId>, TaskError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare("SELECT id FROM task")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(TaskId::from(row?));
        }
        Ok(ids)
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

    /// 领一条到期的效果（同一种类里按队列位置最先的到期行）。
    pub fn claim_effect(
        &self,
        kind: EffectKind,
        now: i64,
    ) -> Result<Option<EffectClaim>, TaskError> {
        let conn = self.lock()?;
        claim_effect_in(&conn, kind, now, false)
    }

    /// 按 ordered 语义领一条效果：先看这条种类里队列最前的一行，没到期就谁也不领，
    /// 后面的行不能越队（照 DSH effects 的 ordered 处理）。
    pub fn claim_effect_ordered(
        &self,
        kind: EffectKind,
        now: i64,
    ) -> Result<Option<EffectClaim>, TaskError> {
        let conn = self.lock()?;
        claim_effect_in(&conn, kind, now, true)
    }

    /// 这个键的效果还在不在排队。
    pub fn effect_pending(&self, kind: EffectKind, key: &str) -> Result<bool, TaskError> {
        let conn = self.lock()?;
        effect_pending_in(&conn, kind, key)
    }

    /// 这个任务有没有一条还没送出去的 restart 汇报。
    ///
    /// 启动扫描把要报的任务并进同一次启动的一条效果；旧库里的行没有任务名单时按
    /// 「全部还被打断的」算（和投递方的回退一致），不能因为缺字段漏掉待核实。
    pub fn restart_pending(&self, id: &TaskId) -> Result<bool, TaskError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare("SELECT value FROM effect WHERE kind = ?1")?;
        let rows = statement.query_map(params![EffectKind::Restart.as_str()], |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            let payload: EffectPayload = serde_json::from_str(&row?)?;
            let EffectPayload::Restart { task_ids, .. } = payload else {
                continue;
            };
            if task_ids.is_empty() || task_ids.contains(id) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 记执行者在起跑前查到的任务包缺项；空表示包齐。
    pub fn record_package_check(
        &self,
        id: &TaskId,
        missing: Vec<MissingItem>,
    ) -> Result<(), TaskError> {
        if missing.iter().any(|item| item.as_str().trim().is_empty()) {
            return Err(TaskError::EmptyMissing);
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        task.missing = missing;
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(())
    }

    /// 开一个新的 Attempt（没有 Run 时先开 Run）。
    pub fn begin_attempt(&self, id: &TaskId, now: i64) -> Result<AttemptId, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        if let Some(open) = task
            .runs
            .iter()
            .flat_map(|run| run.attempts.iter())
            .find(|attempt| attempt.ended_at.is_none())
        {
            return Err(TaskError::AttemptStillOpen {
                task: id.to_string(),
                attempt: open.id.0,
            });
        }
        let attempt_id = AttemptId(
            task.runs
                .iter()
                .map(|run| run.attempts.len() as u64)
                .sum::<u64>()
                + 1,
        );
        let attempt = Attempt {
            id: attempt_id,
            conversation: None,
            trigger: AttemptTrigger::Launch,
            started_at: now,
            ended_at: None,
            end: None,
        };
        if let Some(run) = task.runs.last_mut() {
            run.attempts.push(attempt);
        } else {
            task.runs.push(Run {
                id: crate::RunId(1),
                started_at: now,
                attempts: vec![attempt],
            });
        }
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(attempt_id)
    }

    /// 用户在任务对话里说一句：在一个事务里记输入、排 delivery 和 notice；
    /// 上一 Attempt 已结束了就接着开一次 Attempt（会话不变）。
    pub fn say(&self, id: &TaskId, text: &str, now: i64) -> Result<SayOutcome, TaskError> {
        self.say_with_attachments(id, text, &[], now)
    }

    /// 和 say 一样，再记下随这句话一起发来的附件；只有附件、没有文字也算一句。
    pub fn say_with_attachments(
        &self,
        id: &TaskId,
        text: &str,
        attachments: &[InputAttachment],
        now: i64,
    ) -> Result<SayOutcome, TaskError> {
        self.say_input(
            id,
            NewInput {
                text: text.to_owned(),
                pastes: Vec::new(),
                attachments: attachments.to_vec(),
            },
            now,
        )
    }

    /// 记下用户在任务对话里说的一句话（文字、粘贴段、附件）。
    ///
    /// 输入、delivery 和 notice 同事务；上一 Attempt 已结束了就接着开一次 Attempt（会话不变）。
    pub fn say_input(
        &self,
        id: &TaskId,
        input: NewInput,
        now: i64,
    ) -> Result<SayOutcome, TaskError> {
        // 只有粘贴或只有附件、没有文字，也算一句；空粘贴段和空白文字不算内容。
        if input.text.trim().is_empty()
            && input.pastes.iter().all(String::is_empty)
            && input.attachments.is_empty()
        {
            return Err(TaskError::EmptyInput);
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let input_id = InputId::from(generate_id(&transaction)?);
        task.inputs.push(Input {
            id: input_id.clone(),
            at: now,
            source: InputSource::User,
            text: input.text,
            pastes: input.pastes,
            attachments: input.attachments,
        });
        // 只回报「接着做会开第几次 Attempt」；Attempt 行等投递真的接上会话再落，
        // 免得投递只 Later 时留下一个没有会话的空 Attempt。
        let new_attempt = followup_preview(&task);
        save_task(&transaction, &task)?;
        self.enqueue_effect_in(
            &transaction,
            &EffectPayload::Delivery {
                task_id: task.id.clone(),
            },
            now,
        )?;
        self.enqueue_effect_in(
            &transaction,
            &EffectPayload::Notice {
                task_id: task.id.clone(),
                input_id: input_id.clone(),
            },
            now,
        )?;
        transaction.commit()?;
        Ok(SayOutcome {
            input_id,
            new_attempt,
        })
    }

    /// 投递接上原会话之后，再落一条「接着做」的 Attempt（会话不变）。
    ///
    /// 返回 None：没有可接着做的 Attempt（还在跑、没开成会话、或没有上一个）。
    /// 放在投递成功路径上，是为了不出现「Attempt 已开、会话却没接上」的空行。
    pub fn begin_followup(&self, id: &TaskId, now: i64) -> Result<Option<AttemptId>, TaskError> {
        self.begin_followup_with(id, AttemptTrigger::UserMessage, now)
    }

    /// 同 begin_followup，触发原因由调用方给：说话接续是 user_message，确认卡接续是 resume。
    pub fn begin_followup_with(
        &self,
        id: &TaskId,
        trigger: AttemptTrigger,
        now: i64,
    ) -> Result<Option<AttemptId>, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let Some(attempt_id) = begin_followup_attempt(&mut task, trigger, now) else {
            transaction.commit()?;
            return Ok(None);
        };
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(Some(attempt_id))
    }

    /// 会话真的开出来之后，把会话 id 写回 Attempt。
    pub fn attach_session(
        &self,
        id: &TaskId,
        attempt_id: AttemptId,
        conversation: ConversationId,
    ) -> Result<(), TaskError> {
        if conversation.as_str().trim().is_empty() {
            return Err(TaskError::EmptyConversation);
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let attempt = task
            .runs
            .iter_mut()
            .flat_map(|run| run.attempts.iter_mut())
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| TaskError::AttemptNotFound {
                task: id.to_string(),
                attempt: attempt_id.0,
            })?;
        if attempt.ended_at.is_some() {
            return Err(TaskError::AttemptEnded {
                task: id.to_string(),
                attempt: attempt_id.0,
            });
        }
        match &attempt.conversation {
            Some(existing) if existing == &conversation => return Ok(()),
            Some(existing) => {
                return Err(TaskError::AttemptSessionConflict {
                    attempt: attempt_id.0,
                    existing: existing.to_string(),
                    requested: conversation.to_string(),
                });
            }
            None => attempt.conversation = Some(conversation),
        }
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(())
    }

    /// 记下第一次 Attempt 起跑前开出来的工作树；已经记过时保持原样（重试幂等）。
    pub fn attach_worktree(&self, id: &TaskId, worktree: Worktree) -> Result<Task, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        if task.worktree_state.is_none() {
            task.worktree_state = Some(worktree);
            save_task(&transaction, &task)?;
        }
        transaction.commit()?;
        Ok(task)
    }

    /// 结束一个 Attempt；已经结束的再结束一次不产生变化。
    pub fn end_attempt(
        &self,
        id: &TaskId,
        attempt_id: AttemptId,
        now: i64,
        end: AttemptEnd,
    ) -> Result<(), TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let attempt = task
            .runs
            .iter_mut()
            .flat_map(|run| run.attempts.iter_mut())
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| TaskError::AttemptNotFound {
                task: id.to_string(),
                attempt: attempt_id.0,
            })?;
        if attempt.ended_at.is_none() {
            attempt.ended_at = Some(now);
            attempt.end = Some(end);
            save_task(&transaction, &task)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// 立即停：在一个事务里把 Attempt 记成 stopped，并排一条停止 relay。
    ///
    /// 已经停过时不再排第二条；已经以别的理由结束时不改动，交调用方按「没在跑」处理。
    pub fn stop_attempt(
        &self,
        id: &TaskId,
        attempt_id: AttemptId,
        now: i64,
    ) -> Result<StopAttempt, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let attempt = task
            .runs
            .iter_mut()
            .flat_map(|run| run.attempts.iter_mut())
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| TaskError::AttemptNotFound {
                task: id.to_string(),
                attempt: attempt_id.0,
            })?;
        // 旧库可能只有 ended_at、没有 end（end 是后加的值）；那样也算已经结束，不再改写。
        if attempt.ended_at.is_some() {
            let outcome = if attempt.end == Some(AttemptEnd::Stopped) {
                StopAttempt::AlreadyStopped
            } else {
                StopAttempt::AlreadyEnded
            };
            transaction.commit()?;
            return Ok(outcome);
        }
        attempt.ended_at = Some(now);
        attempt.end = Some(AttemptEnd::Stopped);
        save_task(&transaction, &task)?;
        self.enqueue_effect_in(
            &transaction,
            &EffectPayload::Stop {
                task_id: id.clone(),
                attempt_id,
            },
            now,
        )?;
        transaction.commit()?;
        Ok(StopAttempt::Stopped)
    }

    /// 启动扫描：效果循环开始之前把上次启动留下、没收尾的 Attempt 收掉。
    ///
    /// 最近 Attempt 没结束的任务分三路：交了报告且之后没有新输入的记 reported；崩溃前
    /// 没开出会话的记 not_opened；其余的记 interrupted，并在同一个事务里排一条 restart
    /// 效果（键是启动 id，同一次启动的任务合成一条）。
    pub fn scan_at_boot(&self, boot_id: &BootId, now: i64) -> Result<BootScan, TaskError> {
        if boot_id.as_str().trim().is_empty() {
            return Err(TaskError::EmptyBootId);
        }
        let mut scan = BootScan::default();
        for task_id in self.task_ids()? {
            self.scan_task_at_boot(&task_id, boot_id, now, &mut scan)?;
        }
        Ok(scan)
    }

    /// 扫一个任务：只收尾最近一次 Attempt；每个任务和它的 restart 效果在同一个事务里。
    fn scan_task_at_boot(
        &self,
        id: &TaskId,
        boot_id: &BootId,
        now: i64,
        scan: &mut BootScan,
    ) -> Result<(), TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = load_task(&transaction, id)? else {
            transaction.commit()?;
            return Ok(());
        };
        // 旧库可能只有 ended_at、没有 end；没有 end 的 Attempt 按没结束读。
        let Some(attempt) = task
            .latest_attempt()
            .filter(|attempt| attempt.end.is_none())
        else {
            transaction.commit()?;
            return Ok(());
        };
        let attempt_id = attempt.id;
        let opened = attempt.conversation.is_some();
        let report_at = task.latest_report().map(|report| report.at);
        let decision = if !opened {
            Decision::NotOpened
        } else if let Some(report_at) = report_at {
            // 报告之后又记下了输入：那次输入还没送进去，这次 Attempt 不能记「已交结果」。
            if task.inputs.iter().any(|input| input.at >= report_at) {
                Decision::Interrupted
            } else {
                Decision::Reported
            }
        } else {
            Decision::Interrupted
        };
        let attempt = task
            .runs
            .iter_mut()
            .flat_map(|run| run.attempts.iter_mut())
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| TaskError::AttemptNotFound {
                task: id.to_string(),
                attempt: attempt_id.0,
            })?;
        if attempt.ended_at.is_none() {
            attempt.ended_at = Some(now);
        }
        attempt.end = Some(decision.end());
        save_task(&transaction, &task)?;
        if decision == Decision::Interrupted {
            // 同一次启动只合并一条 restart：把这次扫到的任务并进去，投递时只报这些。
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT value FROM effect WHERE kind = ?1 AND key = ?2",
                    params![EffectKind::Restart.as_str(), boot_id.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            let mut task_ids = match existing {
                Some(value) => match serde_json::from_str(&value)? {
                    EffectPayload::Restart { task_ids, .. } => task_ids,
                    _ => Vec::new(),
                },
                None => Vec::new(),
            };
            task_ids.push(id.clone());
            self.enqueue_effect_in(
                &transaction,
                &EffectPayload::Restart {
                    boot_id: boot_id.clone(),
                    task_ids,
                },
                now,
            )?;
        }
        transaction.commit()?;
        match decision {
            Decision::Interrupted => scan.interrupted.push(id.clone()),
            Decision::Reported => scan.reported.push(id.clone()),
            Decision::NotOpened => scan.not_opened.push(id.clone()),
        }
        Ok(())
    }

    /// 收尾崩溃留下的、没有会话 id 的 Attempt；返回收尾了几个。
    pub fn close_unopened_attempts(&self, id: &TaskId, now: i64) -> Result<usize, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let mut closed = 0;
        for attempt in task.runs.iter_mut().flat_map(|run| run.attempts.iter_mut()) {
            if attempt.conversation.is_none() && attempt.ended_at.is_none() {
                attempt.ended_at = Some(now);
                attempt.end = Some(AttemptEnd::NotOpened);
                closed += 1;
            }
        }
        if closed > 0 {
            save_task(&transaction, &task)?;
        }
        transaction.commit()?;
        Ok(closed)
    }

    /// 记一份报告并在同一个事务里排 relay。
    pub fn record_report(
        &self,
        id: &TaskId,
        attempt_id: AttemptId,
        report: NewReport,
        now: i64,
    ) -> Result<ReportId, TaskError> {
        self.record_report_in(id, attempt_id, report, now)
            .map(|(report_id, _)| report_id)
    }

    /// 交回专用：同一个事务里写报告并排 relay，返回报告 id 和「这次是否新写」。
    /// 同一个 Attempt 已有同态报告时不再写、也不再排 relay；交回靠它把第二次变成「已交回」。
    pub fn record_handback_report(
        &self,
        id: &TaskId,
        attempt_id: AttemptId,
        report: NewReport,
        now: i64,
    ) -> Result<(ReportId, bool), TaskError> {
        self.record_report_in(id, attempt_id, report, now)
    }

    fn record_report_in(
        &self,
        id: &TaskId,
        attempt_id: AttemptId,
        report: NewReport,
        now: i64,
    ) -> Result<(ReportId, bool), TaskError> {
        report.validate()?;
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let attempt = task
            .attempt(attempt_id)
            .ok_or_else(|| TaskError::AttemptNotFound {
                task: id.to_string(),
                attempt: attempt_id.0,
            })?;
        // 停止之后这次 Attempt 的结果已经定死，晚到的报告一律拒绝。
        if attempt.end == Some(AttemptEnd::Stopped) {
            return Err(TaskError::AttemptEnded {
                task: id.to_string(),
                attempt: attempt_id.0,
            });
        }
        // 合成的 waiting 之后，同一个 Attempt 还能报终态：报告按时间追加，最新的那条
        // 决定阶段。终态另起一条报告：relay 的键是报告 id，复用 waiting 的 id 会被下游
        // 按 request id 去重，终态就送不出去。只有旧、新都是终态且不同才冲突；同态幂等。
        let existing = task
            .reports
            .iter()
            .rev()
            .find(|existing| existing.attempt_id == attempt_id);
        if let Some(existing) = existing {
            if existing.state == report.state {
                let id = existing.id.clone();
                transaction.commit()?;
                return Ok((id, false));
            }
            if existing.state.is_terminal() && report.state.is_terminal() {
                return Err(TaskError::ReportConflict {
                    attempt: attempt_id.0,
                    existing: existing.state,
                    requested: report.state,
                });
            }
        }
        let report_id = ReportId::from(generate_id(&transaction)?);
        task.reports.push(Report {
            id: report_id.clone(),
            attempt_id,
            state: report.state,
            origin: report.origin,
            summary: report.summary,
            question: report.question,
            at: now,
        });
        save_task(&transaction, &task)?;
        self.enqueue_effect_in(
            &transaction,
            &EffectPayload::Relay {
                task_id: id.clone(),
                report_id: report_id.clone(),
            },
            now,
        )?;
        transaction.commit()?;
        Ok((report_id, true))
    }

    /// 用户接受当前版本和最近一份报告；只记接受，不改阶段（03 篇 5.1、5.4）。
    pub fn accept(&self, id: &TaskId, now: i64) -> Result<Acceptance, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let Some(report_id) = task.latest_report().map(|report| report.id.clone()) else {
            return Err(TaskError::NoReportToAccept);
        };
        let acceptance = Acceptance {
            version: task.requirement_version(),
            report_id,
            at: now,
        };
        task.acceptance = Some(acceptance.clone());
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(acceptance)
    }

    /// task_update：把写入范围换成新的并追加一条要求版本（D16）；返回更新后的任务。
    /// 只在 Kind 对话里用户同意后用；本票只改写入范围。
    pub fn update_write_scope(
        &self,
        id: &TaskId,
        write_scope: &str,
        now: i64,
    ) -> Result<Task, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let Some(scope) = resolve_write_scope(&task.cwd, Some(write_scope))? else {
            return Err(TaskError::EmptyWriteScope);
        };
        // 工作树任务的新范围必须还在原仓库里，否则换算后落不到工作树里。
        if let Some(worktree) = &task.worktree_state
            && !scope.starts_with(&worktree.repo)
        {
            return Err(TaskError::WriteScopeOutsideWorktree(
                scope.display().to_string(),
            ));
        }
        task.write_scope = Some(scope.clone());
        task.versions.push(RequirementVersion {
            write_scope: Some(scope),
            at: now,
            source: VersionSource::UserAgreed,
        });
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(task)
    }

    /// 从任务和效果表派生阶段。
    pub fn stage(&self, id: &TaskId) -> Result<Stage, TaskError> {
        let conn = self.lock()?;
        let task = load_task_required(&conn, id)?;
        let launch_queued = effect_pending_in(&conn, EffectKind::Launch, id.as_str())?;
        Ok(task.stage(launch_queued))
    }

    /// 删除一条与 claim 的 seq/generation 仍相等的效果行；返回是否删到。
    pub fn complete_effect(&self, claim: &EffectClaim) -> Result<bool, TaskError> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "DELETE FROM effect WHERE kind = ?1 AND key = ?2 AND seq = ?3 AND generation = ?4",
            params![
                claim.payload.kind().as_str(),
                claim.key,
                claim.seq,
                claim.generation
            ],
        )?;
        Ok(changed > 0)
    }

    /// 记一次失败：连续失败次数 +1，按指数退避排下一次。
    pub fn fail_effect(
        &self,
        claim: &EffectClaim,
        error: &str,
        now: i64,
    ) -> Result<bool, TaskError> {
        let (initial, maximum) = claim.payload.kind().retry();
        let delay = initial
            .saturating_mul(1_i64 << claim.attempts.min(16))
            .min(maximum);
        let due = now.saturating_add(delay);
        let error: String = error.chars().take(1000).collect();
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE effect SET attempts = attempts + 1, error = ?1, due = ?2
             WHERE kind = ?3 AND key = ?4 AND seq = ?5 AND generation = ?6",
            params![
                error,
                due,
                claim.payload.kind().as_str(),
                claim.key,
                claim.seq,
                claim.generation
            ],
        )?;
        Ok(changed > 0)
    }

    /// 现在不能跑：不算失败，按初值推迟。
    pub fn defer_effect(&self, claim: &EffectClaim, now: i64) -> Result<bool, TaskError> {
        let (initial, _) = claim.payload.kind().retry();
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE effect SET due = ?1
             WHERE kind = ?2 AND key = ?3 AND seq = ?4 AND generation = ?5",
            params![
                now.saturating_add(initial),
                claim.payload.kind().as_str(),
                claim.key,
                claim.seq,
                claim.generation
            ],
        )?;
        Ok(changed > 0)
    }

    /// 从 effect 表现算一个种类的健康状态。
    pub fn effect_health(&self, kind: EffectKind) -> Result<EffectHealth, TaskError> {
        let conn = self.lock()?;
        let (waiting, since, attempts, last_error) = conn.query_row(
            "SELECT COUNT(*), MIN(since), MAX(attempts),
                    (SELECT error FROM effect WHERE kind = ?1 AND attempts > 0
                     ORDER BY seq DESC LIMIT 1)
             FROM effect WHERE kind = ?1",
            params![kind.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )?;
        let waiting = u32::try_from(waiting).unwrap_or(u32::MAX);
        let attempts = u32::try_from(attempts.unwrap_or(0)).unwrap_or(u32::MAX);
        // quiet 的种类（delivery）失败已经在任务上体现，不把健康项标成 error。
        let state = if waiting > 0 && attempts >= FAILURES_BEFORE_ERROR && !kind.quiet() {
            EffectState::Error
        } else {
            EffectState::Ready
        };
        Ok(EffectHealth {
            state,
            waiting,
            attempts,
            since,
            last_error: if attempts > 0 { last_error } else { None },
        })
    }
}

/// 启动扫描对一个没结束的 Attempt 的收尾方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// 在跑、没交出结果，或报告之后又有了新输入。
    Interrupted,
    /// 报告之后没有新输入：这次 Attempt 已经交出结果。
    Reported,
    /// 崩溃前没开出会话。
    NotOpened,
}

impl Decision {
    fn end(self) -> AttemptEnd {
        match self {
            Self::Interrupted => AttemptEnd::Interrupted,
            Self::Reported => AttemptEnd::Reported,
            Self::NotOpened => AttemptEnd::NotOpened,
        }
    }
}

/// 上一 Attempt 已结束时，在同一个会话里接着开一次 Attempt。
///
/// 旧库可能只有 ended_at、没有 end；那也是已结束，照样接着做。没开成的 Attempt
/// 没有会话可送，不算「可以接着做」。
fn followup_preview(task: &Task) -> Option<AttemptId> {
    let latest = task.latest_attempt()?;
    if latest.ended_at.is_none() || latest.end == Some(AttemptEnd::NotOpened) {
        return None;
    }
    Some(AttemptId(
        task.runs
            .iter()
            .map(|run| run.attempts.len() as u64)
            .sum::<u64>()
            + 1,
    ))
}

fn begin_followup_attempt(task: &mut Task, trigger: AttemptTrigger, now: i64) -> Option<AttemptId> {
    let latest = task.latest_attempt()?;
    if latest.ended_at.is_none() || latest.end == Some(AttemptEnd::NotOpened) {
        return None;
    }
    let conversation = latest.conversation.clone();
    let attempt_id = AttemptId(
        task.runs
            .iter()
            .map(|run| run.attempts.len() as u64)
            .sum::<u64>()
            + 1,
    );
    let attempt = Attempt {
        id: attempt_id,
        conversation,
        trigger,
        started_at: now,
        ended_at: None,
        end: None,
    };
    match task.runs.last_mut() {
        Some(run) => run.attempts.push(attempt),
        None => task.runs.push(Run {
            id: crate::RunId(1),
            started_at: now,
            attempts: vec![attempt],
        }),
    }
    Some(attempt_id)
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

fn load_task_required(connection: &Connection, id: &TaskId) -> Result<Task, TaskError> {
    load_task(connection, id)?.ok_or_else(|| TaskError::TaskNotFound(id.to_string()))
}

fn save_task(connection: &Connection, task: &Task) -> Result<(), TaskError> {
    let changed = connection.execute(
        "UPDATE task SET value = ?1 WHERE id = ?2",
        params![serde_json::to_string(task)?, task.id.as_str()],
    )?;
    if changed != 1 {
        return Err(TaskError::TaskNotFound(task.id.to_string()));
    }
    Ok(())
}

type EffectRow = (String, String, i64, i64, i64, i64, i64);

fn effect_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EffectRow> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, i64>(2)?,
        row.get::<_, i64>(3)?,
        row.get::<_, i64>(4)?,
        row.get::<_, i64>(5)?,
        row.get::<_, i64>(6)?,
    ))
}

fn claim_effect_in(
    connection: &Connection,
    kind: EffectKind,
    now: i64,
    ordered: bool,
) -> Result<Option<EffectClaim>, TaskError> {
    // ordered 种类先只按队列位置取最前一行；它还没到期时整队都不动。
    let row = if ordered {
        connection
            .query_row(
                "SELECT key, value, seq, generation, attempts, due, since
                 FROM effect WHERE kind = ?1 ORDER BY seq LIMIT 1",
                params![kind.as_str()],
                effect_row,
            )
            .optional()?
    } else {
        connection
            .query_row(
                "SELECT key, value, seq, generation, attempts, due, since
                 FROM effect WHERE kind = ?1 AND due <= ?2 ORDER BY seq LIMIT 1",
                params![kind.as_str(), now],
                effect_row,
            )
            .optional()?
    };
    let Some((key, value, seq, generation, attempts, due, since)) = row else {
        return Ok(None);
    };
    if due > now {
        return Ok(None);
    }
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
#[path = "attempt_report_tests.rs"]
mod attempt_report_tests;
#[cfg(test)]
#[path = "boot_scan_tests.rs"]
mod boot_scan_tests;
#[cfg(test)]
#[path = "effect_tests.rs"]
mod effect_tests;
#[cfg(test)]
#[path = "say_tests.rs"]
mod say_tests;
#[cfg(test)]
#[path = "scope_tests.rs"]
mod scope_tests;
#[cfg(test)]
#[path = "stage_tests.rs"]
mod stage_tests;
#[cfg(test)]
#[path = "stop_tests.rs"]
mod stop_tests;
#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
