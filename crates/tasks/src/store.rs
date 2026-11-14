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
use crate::ledger::{LedgerNote, LedgerNoteCounts, LedgerNoteKind, NewLedgerNote};
use crate::task::{
    AcceptBatch, AcceptOutcome, Acceptance, Arrangement, Attempt, AttemptEnd, AttemptTrigger,
    Cancellation, CancellationReason, Confirmation, FrozenPolicy, GateCandidate, GateConclusion,
    GateVerdict, Handoff, Input, InputAttachment, InputSource, MilestoneGate, MissingItem, Need,
    NewGraph, NewInput, NewReport, NewSubtask, NewTask, Report, ReportState, RequirementVersion,
    Run, SayOutcome, Task, TaskPolicy, VersionSource, Worktree, resolve_write_scope,
};

/// 任务库 schema 版本；比这新的库拒绝打开。
pub const SCHEMA_VERSION: u64 = 2;

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
    CREATE TABLE IF NOT EXISTS ledger_note (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL,
        attempt_id INTEGER NOT NULL,
        kind TEXT NOT NULL,
        topic TEXT NOT NULL,
        text TEXT NOT NULL,
        at INTEGER NOT NULL,
        closed_at INTEGER,
        closed_why TEXT
    );
    CREATE INDEX IF NOT EXISTS ledger_note_task ON ledger_note(task_id, at);
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

/// T053：门判定的结果；同一份门报告重复判定时 duplicate 为真，不重复记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateJudgeOutcome {
    pub verdict: GateVerdict,
    pub duplicate: bool,
    /// 通过时这次放出的下游标题（relay 的「已自动开跑」）。
    pub released: Vec<String>,
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
        let mut conn = conn;
        let version = stored_schema_version(&conn)?;
        if let Some(version) = version {
            if version > SCHEMA_VERSION {
                return Err(TaskError::SchemaTooNew {
                    found: version,
                    supported: SCHEMA_VERSION,
                });
            }
            // v1 → v2 是账本表迁移；比 v1 更老的库没有迁移路径。
            if version < 1 {
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
        if version == Some(1) {
            // T056.2 schema v1→v2：建账本表 + 写版本，一个事务。
            let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "CREATE TABLE IF NOT EXISTS ledger_note (
                    id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL,
                    attempt_id INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    topic TEXT NOT NULL,
                    text TEXT NOT NULL,
                    at INTEGER NOT NULL,
                    closed_at INTEGER,
                    closed_why TEXT
                );
                CREATE INDEX IF NOT EXISTS ledger_note_task ON ledger_note(task_id, at);",
            )?;
            transaction.execute(
                "UPDATE kv SET value = ?1 WHERE key = ?2",
                params![serde_json::to_string(&SCHEMA_VERSION)?, SCHEMA_KEY],
            )?;
            transaction.commit()?;
        }
        let seq: i64 = conn.query_row("SELECT IFNULL(MAX(seq), 0) FROM effect", [], |row| {
            row.get(0)
        })?;
        Ok(Self {
            conn: Mutex::new(conn),
            seq: AtomicI64::new(seq),
        })
    }

    /// T056.2：在当前 Attempt 上记一批叙述条；同 kind+topic 的旧开条先关掉。
    pub fn add_ledger_notes(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        entries: &[NewLedgerNote],
        now: i64,
    ) -> Result<LedgerNoteCounts, TaskError> {
        for entry in entries {
            if entry.topic.trim().is_empty() {
                return Err(TaskError::EmptyLedgerTopic);
            }
            if entry.text.trim().is_empty() {
                return Err(TaskError::EmptyLedgerText);
            }
            let length = entry.text.chars().count();
            if length > 400 {
                return Err(TaskError::LedgerTextTooLong(length));
            }
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = load_task_required(&transaction, task_id)?;
        if task.is_gate() {
            return Err(TaskError::LedgerOnGateTask);
        }
        let attempt = task
            .attempt(attempt_id)
            .ok_or_else(|| TaskError::AttemptNotFound {
                task: task_id.to_string(),
                attempt: attempt_id.0,
            })?;
        if attempt.ended_at.is_some() {
            return Err(TaskError::LedgerAttemptEnded {
                task: task_id.to_string(),
                attempt: attempt_id.0,
            });
        }
        for entry in entries {
            transaction.execute(
                "UPDATE ledger_note SET closed_at = ?1, closed_why = ?2
                 WHERE task_id = ?3 AND kind = ?4 AND topic = ?5 AND closed_at IS NULL",
                params![
                    now,
                    "被新的一条替换",
                    task_id.as_str(),
                    entry.kind.as_str(),
                    entry.topic.trim()
                ],
            )?;
            let id = generate_id(&transaction)?;
            transaction.execute(
                "INSERT INTO ledger_note
                    (id, task_id, attempt_id, kind, topic, text, at, closed_at, closed_why)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
                params![
                    id,
                    task_id.as_str(),
                    i64::try_from(attempt_id.0).unwrap_or(i64::MAX),
                    entry.kind.as_str(),
                    entry.topic.trim(),
                    entry.text.trim(),
                    now
                ],
            )?;
        }
        let counts = ledger_note_counts(&transaction, task_id)?;
        transaction.commit()?;
        Ok(counts)
    }

    /// T056.2：这个任务全部叙述条（新的在前，含已关闭的历史）。
    pub fn ledger_notes(&self, task_id: &TaskId) -> Result<Vec<LedgerNote>, TaskError> {
        let conn = self.lock()?;
        ledger_notes_in(&conn, task_id)
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
        self.create_task_with_constraints(draft, policy, Vec::new(), now)
    }

    /// T056.2：建任务并把用户原话里的约束放进第一条要求版本。
    pub fn create_task_with_constraints(
        &self,
        draft: NewTask,
        policy: TaskPolicy,
        constraints: Vec<String>,
        now: i64,
    ) -> Result<Task, TaskError> {
        draft.validate()?;
        let frozen = policy.freeze(&draft.cwd)?;
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task =
            self.insert_task_in_with_constraints(&transaction, draft, &frozen, constraints, now)?;
        transaction.commit()?;
        Ok(task)
    }

    /// T052：把一张任务图在一个事务里落库：总任务 + 子任务 + needs。
    ///
    /// 图不在这里开跑：确认卡上点「确认」才把没有 needs 的子任务放进 launch 队列。
    pub fn create_graph(&self, graph: NewGraph, now: i64) -> Result<Task, TaskError> {
        self.create_graph_with_constraints(graph, Vec::new(), now)
    }

    /// T056.2：建图并把用户原话里的约束放进总任务的第一条要求版本（子任务只继承不存）。
    pub fn create_graph_with_constraints(
        &self,
        graph: NewGraph,
        constraints: Vec<String>,
        now: i64,
    ) -> Result<Task, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root = self.create_graph_in(&transaction, graph, constraints, now)?;
        transaction.commit()?;
        Ok(root)
    }

    /// T053：按提议的 call id 幂等建图：同一个 call id 再来返回同一张图，不建第二张。
    pub fn create_graph_once(
        &self,
        graph: NewGraph,
        call_id: &str,
        now: i64,
    ) -> Result<(Task, bool), TaskError> {
        self.create_graph_once_with_constraints(graph, Vec::new(), call_id, now)
    }

    /// T056.2：按 call id 幂等建图，并给总任务第一条要求版本带上约束。
    pub fn create_graph_once_with_constraints(
        &self,
        graph: NewGraph,
        constraints: Vec<String>,
        call_id: &str,
        now: i64,
    ) -> Result<(Task, bool), TaskError> {
        if call_id.trim().is_empty() {
            return Err(TaskError::EmptyAcceptKey);
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key = format!("graph-propose:{call_id}");
        let existing: Option<String> = transaction
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(id) = existing {
            let root = load_task_required(&transaction, &TaskId::from(id))?;
            transaction.commit()?;
            return Ok((root, false));
        }
        let root = self.create_graph_in(&transaction, graph, constraints, now)?;
        transaction.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)",
            params![key, root.id.as_str()],
        )?;
        transaction.commit()?;
        Ok((root, true))
    }

    /// 在一个已有事务里落一张图（校验 + 总任务 + 子任务 + needs）。
    fn create_graph_in(
        &self,
        transaction: &Connection,
        graph: NewGraph,
        constraints: Vec<String>,
        now: i64,
    ) -> Result<Task, TaskError> {
        graph.root.validate()?;
        for subtask in &graph.subtasks {
            subtask.task.validate()?;
        }
        let frozen = graph
            .subtasks
            .iter()
            .map(|subtask| subtask.policy.freeze(&subtask.task.cwd))
            .collect::<Result<Vec<_>, _>>()?;
        validate_graph(&graph.subtasks, &frozen)?;
        let root = task_row(
            transaction,
            None,
            graph.root,
            &FrozenPolicy {
                write_scope: None,
                bash: false,
                worktree: false,
            },
            now,
            None,
            Vec::new(),
            Some(Arrangement {
                version: 1,
                confirmed: None,
                cancelled: None,
            }),
            None,
            constraints,
        )?;
        insert_task_row(transaction, &root)?;
        let ids = (0..graph.subtasks.len())
            .map(|_| generate_id(transaction).map(TaskId::from))
            .collect::<Result<Vec<_>, _>>()?;
        for (index, subtask) in graph.subtasks.iter().enumerate() {
            let needs = subtask
                .after
                .iter()
                .map(|after| Need {
                    from: ids[*after].clone(),
                    handoff: None,
                })
                .collect();
            let task = task_row(
                transaction,
                Some(ids[index].clone()),
                subtask.task.clone(),
                &frozen[index],
                now,
                Some(root.id.clone()),
                needs,
                None,
                if subtask.milestone {
                    Some(MilestoneGate {})
                } else {
                    None
                },
                Vec::new(),
            )?;
            insert_task_row(transaction, &task)?;
        }
        Ok(root)
    }

    /// T053：取消一张任务图；同一个原因重复取消返回已有的取消记录。
    pub fn cancel_graph(
        &self,
        root: &TaskId,
        reason: CancellationReason,
        now: i64,
    ) -> Result<Cancellation, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, root)?;
        let Some(arrangement) = task.arrangement.clone() else {
            return Err(TaskError::NotGraphRoot);
        };
        if let Some(existing) = arrangement.cancelled {
            transaction.commit()?;
            return Ok(existing);
        }
        if arrangement.confirmed.is_some() {
            return Err(TaskError::GraphAlreadyConfirmed);
        }
        let cancellation = Cancellation { at: now, reason };
        task.arrangement = Some(Arrangement {
            version: arrangement.version,
            confirmed: None,
            cancelled: Some(cancellation.clone()),
        });
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(cancellation)
    }

    /// T052：总任务的子任务（按落库顺序）。
    pub fn children(&self, parent: &TaskId) -> Result<Vec<Task>, TaskError> {
        let conn = self.lock()?;
        children_in(&conn, parent)
    }

    /// T052：这个子任务的上游是不是都有了 ready 报告（阶段派生和 launch 入口共用）。
    pub fn needs_ready(&self, task: &Task) -> Result<bool, TaskError> {
        let conn = self.lock()?;
        needs_ready_in(&conn, task)
    }

    /// T052：确认任务图：同一个事务里记确认，并把没有 needs 的子任务放进 launch 队列。
    pub fn confirm_graph(&self, root: &TaskId, now: i64) -> Result<usize, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut root_task = load_task_required(&transaction, root)?;
        let Some(arrangement) = root_task.arrangement.clone() else {
            return Err(TaskError::NotGraphRoot);
        };
        if arrangement.confirmed.is_some() {
            return Err(TaskError::GraphAlreadyConfirmed);
        }
        if arrangement.cancelled.is_some() {
            return Err(TaskError::GraphCancelled);
        }
        root_task.arrangement = Some(Arrangement {
            version: arrangement.version,
            confirmed: Some(Confirmation {
                version: arrangement.version,
                at: now,
            }),
            cancelled: None,
        });
        save_task(&transaction, &root_task)?;
        let mut launched = 0_usize;
        for child in children_in(&transaction, root)? {
            if child.needs.is_empty() {
                self.enqueue_effect_in(
                    &transaction,
                    &EffectPayload::Launch {
                        task_id: child.id.clone(),
                    },
                    now,
                )?;
                launched += 1;
            }
        }
        transaction.commit()?;
        Ok(launched)
    }

    /// T052：这个子任务的 ready 报告落库后，放出上游全有 ready 结果的下游。
    ///
    /// 只处理同一个总任务下的兄弟；交接（报告 id、要求版本、摘要）写进下游的 needs，
    /// launch 效果在同一个事务里排（恰好一次由效果表的键保证）。
    fn release_downstream_in(
        &self,
        connection: &Connection,
        task: &Task,
        now: i64,
    ) -> Result<Vec<String>, TaskError> {
        let Some(parent) = &task.parent else {
            return Ok(Vec::new());
        };
        let mut released = Vec::new();
        for mut child in children_in(connection, parent)? {
            if !child.needs.iter().any(|need| need.from == task.id) {
                continue;
            }
            let mut handoffs = Vec::with_capacity(child.needs.len());
            let mut ready = true;
            for need in &child.needs {
                let upstream = load_task_required(connection, &need.from)?;
                let Some(report) = upstream.latest_report() else {
                    ready = false;
                    break;
                };
                if report.state != ReportState::Ready {
                    ready = false;
                    break;
                }
                handoffs.push(Handoff {
                    report_id: report.id.clone(),
                    requirement_version: report.version,
                    summary: report.summary.clone(),
                });
            }
            if !ready {
                continue;
            }
            // 首次交接才放出下游：上游第二次 ready 只刷新交接（下游以后的 Attempt 按最新结果走），
            // 不重新放 launch、也不再重复说「已自动开跑」。
            let first_release = child.needs.iter().any(|need| need.handoff.is_none());
            for (need, handoff) in child.needs.iter_mut().zip(handoffs) {
                need.handoff = Some(handoff);
            }
            save_task(connection, &child)?;
            if first_release {
                self.enqueue_effect_in(
                    connection,
                    &EffectPayload::Launch {
                        task_id: child.id.clone(),
                    },
                    now,
                )?;
                released.push(child.title.clone());
            }
        }
        Ok(released)
    }

    /// T053：上游任务当前可放行的报告（没门=最新 ready；有门=最新判通过的候选；还没判通过=None）。
    pub fn releasable_report(&self, id: &TaskId) -> Result<Option<Report>, TaskError> {
        let conn = self.lock()?;
        let task = load_task_required(&conn, id)?;
        releasable_report_in(&conn, &task)
    }

    /// T053：这个任务最近 ready 报告是否要按上游新结果重评（D95 事实派生）。
    pub fn needs_reevaluation(&self, id: &TaskId) -> Result<bool, TaskError> {
        let conn = self.lock()?;
        let task = load_task_required(&conn, id)?;
        needs_reevaluation_in(&conn, &task)
    }

    /// T053：按当前可放行报告生成这次 Attempt 的交接（上游还没可放行报告时跳过那一项）。
    pub fn current_handoffs(&self, task: &Task) -> Result<Vec<Handoff>, TaskError> {
        let conn = self.lock()?;
        let mut handoffs = Vec::with_capacity(task.needs.len());
        for need in &task.needs {
            let upstream = load_task_required(&conn, &need.from)?;
            if let Some(report) = releasable_report_in(&conn, &upstream)? {
                handoffs.push(Handoff {
                    report_id: report.id.clone(),
                    requirement_version: report.version,
                    summary: report.summary.clone(),
                });
            }
        }
        Ok(handoffs)
    }

    /// T053：这个任务被哪个门把关（Kind 的 task_continue 认原因用）。
    pub fn guarding_gate(&self, id: &TaskId) -> Result<Option<Task>, TaskError> {
        let conn = self.lock()?;
        let task = load_task_required(&conn, id)?;
        gate_for_guarded(&conn, &task)
    }

    /// T053：上游有了新可放行结果，已有结果的下游需重评时同事务排一条 relay 给 Kind（D95）。
    fn notify_upstream_changed_in(
        &self,
        connection: &Connection,
        upstream: &Task,
        report_id: &ReportId,
        now: i64,
    ) -> Result<(), TaskError> {
        let Some(parent) = &upstream.parent else {
            return Ok(());
        };
        for downstream in children_in(connection, parent)? {
            if downstream.id == upstream.id || downstream.is_gate() {
                continue;
            }
            if !downstream.needs.iter().any(|need| need.from == upstream.id) {
                continue;
            }
            if downstream.latest_attempt().is_none()
                || downstream.latest_report().map(|report| report.state) != Some(ReportState::Ready)
            {
                continue;
            }
            if !needs_reevaluation_in(connection, &downstream)? {
                continue;
            }
            self.enqueue_effect_in(
                connection,
                &EffectPayload::UpstreamChanged {
                    upstream_task: upstream.id.clone(),
                    downstream_task: downstream.id.clone(),
                    upstream_report: report_id.clone(),
                },
                now,
            )?;
        }
        Ok(())
    }

    /// T053：ready 报告落库后放出下一站，返回 relay 给 Kind 的「已自动开跑」标题。
    fn release_ready_in(
        &self,
        connection: &Connection,
        task: &Task,
        report_id: &ReportId,
        now: i64,
    ) -> Result<Vec<String>, TaskError> {
        if task.is_gate() {
            self.after_gate_report_in(connection, task, now)?;
            return Ok(Vec::new());
        }
        if let Some(mut gate) = gate_for_guarded(connection, task)? {
            let report = task
                .reports
                .iter()
                .find(|report| &report.id == report_id)
                .ok_or(TaskError::GateReportChanged)?;
            let handoff = Handoff {
                report_id: report.id.clone(),
                requirement_version: report.version,
                summary: report.summary.clone(),
            };
            gate.needs[0].handoff = Some(handoff);
            save_task(connection, &gate)?;
            // 门正在跑：只记下这份交接，等门这次 Attempt 结束后再放一次。
            let running = gate
                .latest_attempt()
                .is_some_and(|attempt| attempt.ended_at.is_none());
            if !running {
                self.enqueue_effect_in(
                    connection,
                    &EffectPayload::Launch {
                        task_id: gate.id.clone(),
                    },
                    now,
                )?;
                return Ok(vec![gate.title.clone()]);
            }
            return Ok(Vec::new());
        }
        self.release_downstream_in(connection, task, now)
    }

    /// T053：门报回一份 ready 报告后，被把关任务已经有更新的 ready 候选时再放一轮门。
    fn after_gate_report_in(
        &self,
        connection: &Connection,
        gate: &Task,
        now: i64,
    ) -> Result<(), TaskError> {
        let Some(need) = gate.needs.first() else {
            return Ok(());
        };
        let guarded = load_task_required(connection, &need.from)?;
        let Some(latest_ready) = guarded
            .latest_report()
            .filter(|report| report.state == ReportState::Ready)
        else {
            return Ok(());
        };
        let last_candidate = gate
            .latest_attempt()
            .and_then(|attempt| attempt.handoffs.first())
            .map(|handoff| handoff.report_id.clone());
        if last_candidate.as_ref() == Some(&latest_ready.id) {
            return Ok(());
        }
        let mut gate_task = load_task_required(connection, &gate.id)?;
        gate_task.needs[0].handoff = Some(Handoff {
            report_id: latest_ready.id.clone(),
            requirement_version: latest_ready.version,
            summary: latest_ready.summary.clone(),
        });
        save_task(connection, &gate_task)?;
        self.enqueue_effect_in(
            connection,
            &EffectPayload::Launch {
                task_id: gate.id.clone(),
            },
            now,
        )?;
        Ok(())
    }

    /// T053：用户在门结果卡上判定。
    ///
    /// 同一个事务：核对门的最近报告和它那次 Attempt 的候选、去重、记判定、排 relay；
    /// 通过时放下被把关任务的其他下游（交接判通过的那份）。
    pub fn judge_gate(
        &self,
        gate_id: &TaskId,
        gate_report: &ReportId,
        conclusion: GateConclusion,
        note: Option<String>,
        now: i64,
    ) -> Result<GateJudgeOutcome, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut gate = load_task_required(&transaction, gate_id)?;
        if !gate.is_gate() {
            return Err(TaskError::NotMilestoneGate);
        }
        if let Some(existing) = gate.verdict_for_gate_report(gate_report).cloned() {
            transaction.commit()?;
            return Ok(GateJudgeOutcome {
                verdict: existing,
                duplicate: true,
                released: Vec::new(),
            });
        }
        let latest_gate_report = gate.latest_report().ok_or(TaskError::GateReportChanged)?;
        if &latest_gate_report.id != gate_report {
            return Err(TaskError::GateReportChanged);
        }
        let need = gate.needs.first().ok_or(TaskError::GateReportChanged)?;
        let guarded_id = need.from.clone();
        let guarded = load_task_required(&transaction, &guarded_id)?;
        let candidate_report = guarded
            .latest_report()
            .filter(|report| report.state == ReportState::Ready)
            .ok_or(TaskError::GateReportChanged)?;
        let attempt = gate
            .latest_attempt()
            .cloned()
            .ok_or(TaskError::GateReportChanged)?;
        let candidate_handoff = attempt
            .handoffs
            .first()
            .cloned()
            .ok_or(TaskError::GateReportChanged)?;
        if candidate_handoff.report_id != candidate_report.id {
            return Err(TaskError::GateReportChanged);
        }
        // 用户原话原样存；空白当没有。
        let note = note.filter(|note| !note.trim().is_empty());
        let verdict = GateVerdict {
            gate_report: gate_report.clone(),
            candidate: GateCandidate {
                report_id: candidate_handoff.report_id.clone(),
                requirement_version: candidate_handoff.requirement_version,
            },
            conclusion,
            note,
            at: now,
        };
        gate.verdicts.push(verdict.clone());
        save_task(&transaction, &gate)?;
        let mut released = Vec::new();
        if conclusion == GateConclusion::Pass {
            let Some(parent) = gate.parent.clone() else {
                return Err(TaskError::GateReportChanged);
            };
            for mut child in children_in(&transaction, &parent)? {
                if child.is_gate() || !child.needs.iter().any(|need| need.from == guarded_id) {
                    continue;
                }
                for need in &mut child.needs {
                    if need.from == guarded_id {
                        need.handoff = Some(candidate_handoff.clone());
                    }
                }
                save_task(&transaction, &child)?;
                // 放过的再遇到新的通过候选不算首次放行（D95 需重评，T053.3 处理），不新开 launch。
                if child.latest_attempt().is_none() {
                    self.enqueue_effect_in(
                        &transaction,
                        &EffectPayload::Launch {
                            task_id: child.id.clone(),
                        },
                        now,
                    )?;
                    released.push(child.title.clone());
                }
            }
        }
        if conclusion == GateConclusion::Pass {
            // T053：判通过让被把关任务有了新可放行结果，已有结果的下游要按新结果核对。
            self.notify_upstream_changed_in(
                &transaction,
                &guarded,
                &candidate_handoff.report_id,
                now,
            )?;
        }
        self.enqueue_effect_in(
            &transaction,
            &EffectPayload::GateVerdict {
                gate_task: gate_id.clone(),
                gate_report: gate_report.clone(),
            },
            now,
        )?;
        transaction.commit()?;
        Ok(GateJudgeOutcome {
            verdict,
            duplicate: false,
            released,
        })
    }

    /// 从任务和效果表派生阶段。
    pub fn stage(&self, id: &TaskId) -> Result<Stage, TaskError> {
        let conn = self.lock()?;
        let task = load_task_required(&conn, id)?;
        stage_with_graph(&conn, &task)
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
        self.create_task_once_with_constraints(draft, policy, Vec::new(), accept_key, now)
    }

    /// T056.2：幂等建任务，并把用户原话里的约束放进第一条要求版本。
    pub fn create_task_once_with_constraints(
        &self,
        draft: NewTask,
        policy: TaskPolicy,
        constraints: Vec<String>,
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
        let task =
            self.insert_task_in_with_constraints(&transaction, draft, &frozen, constraints, now)?;
        transaction.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)",
            params![key, task.id.as_str()],
        )?;
        transaction.commit()?;
        Ok((task, true))
    }

    fn insert_task_in_with_constraints(
        &self,
        connection: &Connection,
        draft: NewTask,
        policy: &FrozenPolicy,
        constraints: Vec<String>,
        now: i64,
    ) -> Result<Task, TaskError> {
        let task = task_row(
            connection,
            None,
            draft,
            policy,
            now,
            None,
            Vec::new(),
            None,
            None,
            constraints,
        )?;
        insert_task_row(connection, &task)?;
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

    /// T055：全表任务，按创建时间升序（同时间按 id）；看板和「现在」用。
    pub fn all_tasks(&self) -> Result<Vec<Task>, TaskError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare("SELECT value FROM task")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(serde_json::from_str::<Task>(&row?)?);
        }
        tasks.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.as_str().cmp(right.id.as_str()))
        });
        Ok(tasks)
    }

    /// T055：任务关闭的时间（unix 毫秒）；没关闭 / 总任务返回 None。
    ///
    /// 没门的任务取当前版本被接受那条 acceptance 的 at；门任务取它把关的任务的 closed_at。
    pub fn closed_at(&self, id: &TaskId) -> Result<Option<i64>, TaskError> {
        let conn = self.lock()?;
        closed_at_in(&conn, id)
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

    /// 开一个新的 Attempt（没有 Run 时先开 Run）；默认 trigger=Launch、没有上游交接。
    pub fn begin_attempt(&self, id: &TaskId, now: i64) -> Result<AttemptId, TaskError> {
        self.begin_attempt_with(id, now, AttemptTrigger::Launch, Vec::new())
    }

    /// 同 begin_attempt，触发原因和开跑时实际拿到的上游报告由调用方给（T053 门重检/接续用）。
    pub fn begin_attempt_with(
        &self,
        id: &TaskId,
        now: i64,
        trigger: AttemptTrigger,
        handoffs: Vec<Handoff>,
    ) -> Result<AttemptId, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        // T052：总任务没有执行承载，任何路径都不能给它开 Attempt。
        if task.is_graph_root() {
            return Err(TaskError::GraphRootCannotRun);
        }
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
            trigger,
            handoffs,
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
        self.begin_followup_with_handoffs(id, trigger, Vec::new(), now)
    }

    /// 同 begin_followup_with，并记下这次接续实际拿到的上游报告（T053 门重检/核对用）。
    pub fn begin_followup_with_handoffs(
        &self,
        id: &TaskId,
        trigger: AttemptTrigger,
        handoffs: Vec<Handoff>,
        now: i64,
    ) -> Result<Option<AttemptId>, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let Some(attempt_id) = begin_followup_attempt(&mut task, trigger, handoffs, now) else {
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
        let report_state = report.state;
        let report_id = ReportId::from(generate_id(&transaction)?);
        task.reports.push(Report {
            id: report_id.clone(),
            attempt_id,
            state: report.state,
            origin: report.origin,
            summary: report.summary,
            question: report.question,
            at: now,
            version: task.requirement_version(),
        });
        save_task(&transaction, &task)?;
        // T056.2：ready 报告关掉「下一步」叙述条（它意味着这个下一步已经走到了）。
        if report_state == ReportState::Ready {
            close_ledger_notes(
                &transaction,
                id,
                Some(LedgerNoteKind::Next),
                &format!("第 {} 次交了结果", attempt_id.0),
                now,
            )?;
        }
        // T052/T053：ready 报告落库的同一个事务里放出该放的下一站：
        // 普通上游放下游；被把关任务放它的门（只记交接）；门报回时按需再放一次门。
        // 效果表键保证崩溃重放恰好一次。
        let released = if report_state == ReportState::Ready {
            let released = self.release_ready_in(&transaction, &task, &report_id, now)?;
            // T053：这条新可放行结果让已有结果的下游「需重评」时，同事务排一条 relay 给 Kind。
            self.notify_upstream_changed_in(&transaction, &task, &report_id, now)?;
            released
        } else {
            Vec::new()
        };
        self.enqueue_effect_in(
            &transaction,
            &EffectPayload::Relay {
                task_id: id.clone(),
                report_id: report_id.clone(),
                released,
            },
            now,
        )?;
        transaction.commit()?;
        Ok((report_id, true))
    }

    /// 用户接受卡上的那份报告：绑报告和要求版本，追加进接受历史（03 篇 5.1、5.4；T054）。
    ///
    /// 顺序：找不到/不是 ready → 拒；门任务自己 → 拒；不是最近报告 → 「已经变了」；
    /// 版本不是当前 → 「要求改过了」；有门但门没对这份判通过 → 拒；同一报告重复接受幂等。
    pub fn accept(
        &self,
        id: &TaskId,
        report_id: &ReportId,
        now: i64,
    ) -> Result<Acceptance, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        let acceptance = accept_task(&transaction, &mut task, report_id, now)?;
        close_ledger_notes(&transaction, id, None, "任务已关闭", now)?;
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(acceptance)
    }

    /// T055.2：组接受——一个事务里逐项走 accept 的同一套校验，通过的一起记，
    /// 没通过的只回 refused（原话照 T054）；同一报告同一版本第二次是幂等的。
    pub fn accept_many(
        &self,
        picks: &[(TaskId, ReportId)],
        now: i64,
    ) -> Result<AcceptBatch, TaskError> {
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut outcomes = Vec::with_capacity(picks.len());
        for (id, report_id) in picks {
            match load_task_required(&transaction, id) {
                Ok(mut task) => {
                    let title = task.title.clone();
                    match accept_task(&transaction, &mut task, report_id, now) {
                        Ok(_) => {
                            close_ledger_notes(&transaction, id, None, "任务已关闭", now)?;
                            save_task(&transaction, &task)?;
                            outcomes.push(AcceptOutcome {
                                task_id: id.clone(),
                                title,
                                refused: None,
                            });
                        }
                        Err(error) => outcomes.push(AcceptOutcome {
                            task_id: id.clone(),
                            title,
                            refused: Some(error.to_string()),
                        }),
                    }
                }
                Err(error) => outcomes.push(AcceptOutcome {
                    task_id: id.clone(),
                    title: id.as_str().to_owned(),
                    refused: Some(error.to_string()),
                }),
            }
        }
        transaction.commit()?;
        Ok(AcceptBatch {
            accepted_at: now,
            outcomes,
        })
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
        // T054.2：旧版本先回填全文，只改写集的版本照抄上一版要求与 DoD。
        backfill_requirement_text(&mut task);
        let constraints = task.current_constraints().to_vec();
        let (requirement, dod) = task
            .versions
            .last()
            .map(|version| (version.requirement.clone(), version.dod.clone()))
            .unwrap_or((Some(task.requirement.clone()), Some(task.dod.clone())));
        task.versions.push(RequirementVersion {
            write_scope: Some(scope),
            at: now,
            source: VersionSource::UserAgreed,
            requirement,
            dod,
            constraints,
        });
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(task)
    }

    /// T054.2：改要求和 DoD（Kind 的 update_task_requirement 确认后）：一个事务里先回填
    /// 所有没有全文的旧版本，再追加第 N+1 版（写集照旧、来源 UserAgreed），最后改 Task。
    ///
    /// 在跑或在排队拒绝；要求和 DoD 都没变也拒绝。已关闭的任务照样能改（旧接受留历史）。
    pub fn update_requirement(
        &self,
        id: &TaskId,
        requirement: &str,
        dod: Option<&str>,
        now: i64,
    ) -> Result<Task, TaskError> {
        self.update_requirement_with_constraints(id, requirement, dod, None, now)
    }

    /// T056.2：改要求时可选带新的约束；None = 沿用上一版约束。
    pub fn update_requirement_with_constraints(
        &self,
        id: &TaskId,
        requirement: &str,
        dod: Option<&str>,
        constraints: Option<Vec<String>>,
        now: i64,
    ) -> Result<Task, TaskError> {
        let requirement = requirement.trim();
        if requirement.is_empty() {
            return Err(TaskError::EmptyRequirement);
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load_task_required(&transaction, id)?;
        if effect_pending_in(&transaction, EffectKind::Launch, id.as_str())? {
            return Err(TaskError::RequirementWhileQueued);
        }
        if task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none())
        {
            return Err(TaskError::RequirementWhileRunning);
        }
        let next_dod = dod
            .map(str::trim)
            .filter(|dod| !dod.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| task.dod.clone());
        if requirement == task.requirement && next_dod == task.dod {
            return Err(TaskError::RequirementUnchanged);
        }
        backfill_requirement_text(&mut task);
        let constraints = constraints.unwrap_or_else(|| task.current_constraints().to_vec());
        let write_scope = task.write_scope.clone();
        task.versions.push(RequirementVersion {
            write_scope,
            at: now,
            source: VersionSource::UserAgreed,
            requirement: Some(requirement.to_owned()),
            dod: Some(next_dod.clone()),
            constraints,
        });
        task.requirement = requirement.to_owned();
        task.dod = next_dod;
        save_task(&transaction, &task)?;
        transaction.commit()?;
        Ok(task)
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

fn begin_followup_attempt(
    task: &mut Task,
    trigger: AttemptTrigger,
    handoffs: Vec<Handoff>,
    now: i64,
) -> Option<AttemptId> {
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
        handoffs,
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

/// T052：按聚合字段建一行 Task（不落库）；单派与建图共用。
#[allow(clippy::too_many_arguments)]
fn task_row(
    connection: &Connection,
    id: Option<TaskId>,
    draft: NewTask,
    policy: &FrozenPolicy,
    now: i64,
    parent: Option<TaskId>,
    needs: Vec<Need>,
    arrangement: Option<Arrangement>,
    gate: Option<MilestoneGate>,
    constraints: Vec<String>,
) -> Result<Task, TaskError> {
    let versions = vec![RequirementVersion {
        write_scope: policy.write_scope.clone(),
        at: now,
        source: VersionSource::TaskAccept,
        requirement: Some(draft.requirement.clone()),
        dod: Some(draft.dod.clone()),
        constraints,
    }];
    let id = match id {
        Some(id) => id,
        None => TaskId::from(generate_id(connection)?),
    };
    Ok(Task {
        id,
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
        acceptances: Vec::new(),
        parent,
        needs,
        arrangement,
        gate,
        verdicts: Vec::new(),
        missing: Vec::new(),
        created_at: now,
    })
}

fn insert_task_row(connection: &Connection, task: &Task) -> Result<(), TaskError> {
    connection.execute(
        "INSERT INTO task (id, value) VALUES (?1, ?2)",
        params![task.id.as_str(), serde_json::to_string(task)?],
    )?;
    Ok(())
}

/// T052：任务图的提议校验：至少两项、after 只指本次提议、无环（Kahn）、
/// 没有先后关系的两项写入范围不能重叠（会同时跑，D15 单写者）。
fn validate_graph(subtasks: &[NewSubtask], frozen: &[FrozenPolicy]) -> Result<(), TaskError> {
    if subtasks.len() < 2 {
        return Err(TaskError::GraphTooFewSubtasks(subtasks.len()));
    }
    // deps[i] = 第 i 项等哪些项（i 依赖它们）。
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); subtasks.len()];
    for (index, subtask) in subtasks.iter().enumerate() {
        for after in &subtask.after {
            if *after >= subtasks.len() {
                return Err(TaskError::GraphAfterOutOfRange {
                    subtask: index,
                    after: *after,
                });
            }
            if !deps[index].contains(after) {
                deps[index].push(*after);
            }
        }
    }
    // T053：里程碑校验——只读、after 恰好一项、不能有门链/双门、别人 after 不能指门。
    let mut guarded_by: Vec<Option<usize>> = vec![None; subtasks.len()];
    for (index, subtask) in subtasks.iter().enumerate() {
        if !subtask.milestone {
            continue;
        }
        if frozen[index].write_scope.is_some() {
            return Err(TaskError::GraphMilestoneHasWriteScope { subtask: index });
        }
        if frozen[index].worktree {
            return Err(TaskError::GraphMilestoneHasWorktree { subtask: index });
        }
        if subtask.after.len() != 1 {
            return Err(TaskError::GraphMilestoneAfterNotOne {
                subtask: index,
                count: subtask.after.len(),
            });
        }
        let guarded = subtask.after[0];
        if subtasks[guarded].milestone {
            return Err(TaskError::GraphMilestoneAfterIsMilestone { subtask: index });
        }
        if let Some(first) = guarded_by[guarded] {
            return Err(TaskError::GraphTwoMilestonesForGuarded {
                first,
                second: index,
            });
        }
        guarded_by[guarded] = Some(index);
    }
    for (index, subtask) in subtasks.iter().enumerate() {
        if subtask.milestone {
            continue;
        }
        if subtask.after.iter().any(|after| subtasks[*after].milestone) {
            return Err(TaskError::GraphMilestoneCannotBeDependency { subtask: index });
        }
    }
    // Kahn 拓扑排序：处理不完就是有环。
    let mut indegree: Vec<usize> = deps.iter().map(Vec::len).collect();
    let mut ready: Vec<usize> = (0..subtasks.len())
        .filter(|index| indegree[*index] == 0)
        .collect();
    let mut seen = 0_usize;
    while let Some(node) = ready.pop() {
        seen += 1;
        for (index, node_deps) in deps.iter().enumerate() {
            if node_deps.contains(&node) {
                indegree[index] -= 1;
                if indegree[index] == 0 {
                    ready.push(index);
                }
            }
        }
    }
    if seen != subtasks.len() {
        return Err(TaskError::GraphHasCycle);
    }
    // 传递闭包：a 是不是（间接）等 b。
    let depends_on = |a: usize, b: usize| -> bool {
        let mut stack = vec![a];
        let mut visited = vec![false; subtasks.len()];
        while let Some(node) = stack.pop() {
            for dep in &deps[node] {
                if *dep == b {
                    return true;
                }
                if !visited[*dep] {
                    visited[*dep] = true;
                    stack.push(*dep);
                }
            }
        }
        false
    };
    for a in 0..subtasks.len() {
        for b in (a + 1)..subtasks.len() {
            if depends_on(a, b) || depends_on(b, a) {
                continue;
            }
            let (Some(scope_a), Some(scope_b)) = (
                frozen[a].write_scope.as_ref(),
                frozen[b].write_scope.as_ref(),
            ) else {
                continue;
            };
            if scope_a == scope_b || scope_a.starts_with(scope_b) || scope_b.starts_with(scope_a) {
                return Err(TaskError::GraphWriteScopeOverlap {
                    a,
                    b,
                    scope_a: scope_a.display().to_string(),
                    scope_b: scope_b.display().to_string(),
                });
            }
        }
    }
    Ok(())
}

/// T052：总任务的子任务（按落库顺序；图很小，扫全表）。
fn children_in(connection: &Connection, parent: &TaskId) -> Result<Vec<Task>, TaskError> {
    let mut statement = connection.prepare("SELECT value FROM task ORDER BY rowid")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut children = Vec::new();
    for row in rows {
        let task: Task = serde_json::from_str(&row?)?;
        if task.parent.as_ref() == Some(parent) {
            children.push(task);
        }
    }
    Ok(children)
}

/// T052：上游是不是都有 ready 报告（交接能否解析、下游能不能放出）。
fn needs_ready_in(connection: &Connection, task: &Task) -> Result<bool, TaskError> {
    for need in &task.needs {
        let upstream = load_task_required(connection, &need.from)?;
        let Some(latest) = upstream
            .latest_report()
            .filter(|report| report.state == ReportState::Ready)
        else {
            return Ok(false);
        };
        // T053：上游有门时，要门对「上游当前最新 ready 报告」判了通过才算可放行；
        // 门任务自己等被把关任务时不要求自己先通过。
        if let Some(gate) = gate_for_guarded(connection, &upstream)?
            && gate.id != task.id
            && !gate.passed(&latest.id)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// T053：上游当前可放行的报告（没门=最近 ready；有门=最近判通过的候选）。
fn releasable_report_in(connection: &Connection, task: &Task) -> Result<Option<Report>, TaskError> {
    if let Some(gate) = gate_for_guarded(connection, task)? {
        let passed = gate
            .verdicts
            .iter()
            .rev()
            .find(|verdict| verdict.conclusion == GateConclusion::Pass)
            .and_then(|verdict| {
                task.reports
                    .iter()
                    .find(|report| report.id == verdict.candidate.report_id)
            })
            .cloned();
        return Ok(passed);
    }
    Ok(task
        .latest_report()
        .filter(|report| report.state == ReportState::Ready)
        .cloned())
}

/// T053：需重评派生——最近 ready 报告那次的 Attempt.handoffs 与上游当前可放行报告比对。
fn needs_reevaluation_in(connection: &Connection, task: &Task) -> Result<bool, TaskError> {
    let Some(attempt) = task.latest_attempt() else {
        return Ok(false);
    };
    if task.needs.is_empty() {
        return Ok(false);
    }
    if task.latest_report().map(|report| report.state) != Some(ReportState::Ready) {
        return Ok(false);
    }
    for (index, need) in task.needs.iter().enumerate() {
        let upstream = load_task_required(connection, &need.from)?;
        let current = releasable_report_in(connection, &upstream)?;
        match (attempt.handoffs.get(index), current) {
            (Some(handoff), Some(report)) if handoff.report_id == report.id => {}
            _ => return Ok(true),
        }
    }
    Ok(false)
}

/// T053：这个任务被哪个门把关（同一总任务下 needs 恰好指向它的里程碑）。
/// T055.2：accept 与 accept_many 共用的校验（幂等在前、门任务、最新报告、当前版本、门判通过），
/// 通过时只把 Acceptance 追加进内存里的 task，落库由调用方在本事务里做。
fn accept_task(
    connection: &Connection,
    task: &mut Task,
    report_id: &ReportId,
    now: i64,
) -> Result<Acceptance, TaskError> {
    // 同一报告同版本幂等：重复点返回已有接受，不改时间。
    if let Some(existing) = task.acceptance_for(report_id) {
        return Ok(existing.clone());
    }
    let Some(report) = task
        .reports
        .iter()
        .find(|report| &report.id == report_id)
        .cloned()
    else {
        return Err(TaskError::ReportNotReady);
    };
    if report.state != ReportState::Ready {
        return Err(TaskError::ReportNotReady);
    }
    if task.is_gate() {
        return Err(TaskError::GateTaskCannotAccept);
    }
    let Some(latest) = task.latest_report() else {
        return Err(TaskError::NoReportToAccept);
    };
    if latest.id != *report_id {
        return Err(TaskError::ReportNotLatest);
    }
    let current = task.requirement_version();
    if report.version != current {
        return Err(TaskError::ReportNotCurrentVersion {
            report: report.version,
            current,
        });
    }
    if let Some(gate) = gate_for_guarded(connection, task)?
        && !gate.passed(report_id)
    {
        return Err(TaskError::GateNotPassed);
    }
    let acceptance = Acceptance {
        version: current,
        report_id: report_id.clone(),
        at: now,
    };
    task.acceptances.push(acceptance.clone());
    Ok(acceptance)
}

fn gate_for_guarded(connection: &Connection, task: &Task) -> Result<Option<Task>, TaskError> {
    let Some(parent) = &task.parent else {
        return Ok(None);
    };
    for sibling in children_in(connection, parent)? {
        if sibling.is_gate()
            && sibling
                .needs
                .first()
                .is_some_and(|need| need.from == task.id)
        {
            return Ok(Some(sibling));
        }
    }
    Ok(None)
}

/// T052：阶段派生（含图）：总任务按子任务汇总；子任务图没确认/上游没结果 = 待补足条件。
fn stage_with_graph(connection: &Connection, task: &Task) -> Result<Stage, TaskError> {
    if task.is_graph_root() {
        let children = children_in(connection, &task.id)?;
        let mut aggregate: Option<Stage> = None;
        for child in &children {
            let stage = stage_with_graph(connection, child)?;
            aggregate = Some(match aggregate {
                Some(current) if current.rank() <= stage.rank() => current,
                _ => stage,
            });
        }
        return Ok(aggregate.unwrap_or(Stage::Pending));
    }
    if let Some(parent) = &task.parent {
        let root = load_task_required(connection, parent)?;
        // T053：取消的图不进任何阶段推进（子任务不显示成排队）。
        if root
            .arrangement
            .as_ref()
            .and_then(|arrangement| arrangement.cancelled.as_ref())
            .is_some()
        {
            return Ok(Stage::NeedsConditions);
        }
        let confirmed = root
            .arrangement
            .as_ref()
            .and_then(|arrangement| arrangement.confirmed.as_ref())
            .is_some();
        if !confirmed || !needs_ready_in(connection, task)? {
            return Ok(Stage::NeedsConditions);
        }
    }
    let launch_queued = effect_pending_in(connection, EffectKind::Launch, task.id.as_str())?;
    let stage = task.stage(launch_queued);
    // T054：接受后的「已关闭」（门任务跟着被把关任务关；总任务由 children 的 rank 汇总到 Closed）。
    if closed_in(connection, task)? {
        return Ok(Stage::Closed);
    }
    // T053：门任务和被把关任务的阶段覆盖（03 篇 5.1 新增「任务要求已满足」）。
    if let Some(parent) = &task.parent {
        let siblings = children_in(connection, parent)?;
        if task.is_gate() {
            if let Some(latest) = task
                .latest_report()
                .filter(|report| report.state == ReportState::Ready)
            {
                return Ok(if task.verdict_for_gate_report(&latest.id).is_some() {
                    Stage::RequirementMet
                } else {
                    Stage::ReadyForReview
                });
            }
            return Ok(stage);
        }
        let gate_for_task = siblings.iter().find(|sibling| {
            sibling.is_gate()
                && sibling
                    .needs
                    .first()
                    .is_some_and(|need| need.from == task.id)
        });
        if let Some(gate) = gate_for_task
            && let Some(latest) = task
                .latest_report()
                .filter(|report| report.state == ReportState::Ready)
        {
            return Ok(if gate.passed(&latest.id) {
                Stage::RequirementMet
            } else {
                Stage::ReadyForReview
            });
        }
    }
    Ok(stage)
}

// T054：按事实派生「已关闭」——当前版本的最新 ready 报告被接受（有门的还要门判通过）；
// 门任务在被把关任务关闭后关闭。总任务的全关闭由 stage_with_graph 的 rank 汇总得到。
fn closed_in(connection: &Connection, task: &Task) -> Result<bool, TaskError> {
    if task.is_gate() {
        let Some(need) = task.needs.first() else {
            return Ok(false);
        };
        let guarded = load_task_required(connection, &need.from)?;
        return Ok(stage_with_graph(connection, &guarded)? == Stage::Closed);
    }
    if task.is_graph_root() {
        return Ok(false);
    }
    let Some(latest) = task
        .latest_report()
        .filter(|report| report.state == ReportState::Ready)
    else {
        return Ok(false);
    };
    if latest.version != task.requirement_version() {
        return Ok(false);
    }
    if task.acceptance_for(&latest.id).is_none() {
        return Ok(false);
    }
    if let Some(gate) = gate_for_guarded(connection, task)?
        && !gate.passed(&latest.id)
    {
        return Ok(false);
    }
    Ok(true)
}

/// T055：关闭时间的派生（阶段 Closed 的那条 acceptance 的 at；门任务跟被把关任务）。
fn closed_at_in(connection: &Connection, id: &TaskId) -> Result<Option<i64>, TaskError> {
    let task = load_task_required(connection, id)?;
    if task.is_graph_root() {
        return Ok(None);
    }
    if task.is_gate() {
        let Some(need) = task.needs.first() else {
            return Ok(None);
        };
        return closed_at_in(connection, &need.from);
    }
    let Some(latest) = task
        .latest_report()
        .filter(|report| report.state == ReportState::Ready)
    else {
        return Ok(None);
    };
    if latest.version != task.requirement_version() {
        return Ok(None);
    }
    let Some(acceptance) = task.acceptance_for(&latest.id) else {
        return Ok(None);
    };
    if let Some(gate) = gate_for_guarded(connection, &task)?
        && !gate.passed(&latest.id)
    {
        return Ok(None);
    }
    Ok(Some(acceptance.at))
}

/// T054.2：把旧行没有全文的版本用当前 Task 的文本回填（第一次改要求时做）。
fn backfill_requirement_text(task: &mut Task) {
    for version in &mut task.versions {
        if version.requirement.is_none() {
            version.requirement = Some(task.requirement.clone());
        }
        if version.dod.is_none() {
            version.dod = Some(task.dod.clone());
        }
    }
}

fn generate_id(connection: &Connection) -> Result<String, TaskError> {
    let id = connection.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
    Ok(id)
}

/// T056.2：关开着的叙述条；kind=None 关全部（任务接受/关闭）。
fn close_ledger_notes(
    connection: &Connection,
    task_id: &TaskId,
    kind: Option<LedgerNoteKind>,
    why: &str,
    now: i64,
) -> Result<usize, TaskError> {
    let changed = match kind {
        Some(kind) => connection.execute(
            "UPDATE ledger_note SET closed_at = ?1, closed_why = ?2
             WHERE task_id = ?3 AND kind = ?4 AND closed_at IS NULL",
            params![now, why, task_id.as_str(), kind.as_str()],
        )?,
        None => connection.execute(
            "UPDATE ledger_note SET closed_at = ?1, closed_why = ?2
             WHERE task_id = ?3 AND closed_at IS NULL",
            params![now, why, task_id.as_str()],
        )?,
    };
    Ok(changed)
}

fn ledger_notes_in(
    connection: &Connection,
    task_id: &TaskId,
) -> Result<Vec<LedgerNote>, TaskError> {
    let mut statement = connection.prepare(
        "SELECT id, task_id, attempt_id, kind, topic, text, at, closed_at, closed_why
         FROM ledger_note WHERE task_id = ?1 ORDER BY at DESC, id DESC",
    )?;
    let rows = statement.query_map(params![task_id.as_str()], |row| {
        let kind: String = row.get(3)?;
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            kind,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut notes = Vec::new();
    for row in rows {
        let (id, task_id, attempt_id, kind, topic, text, at, closed_at, closed_why) = row?;
        let Some(kind) = LedgerNoteKind::parse(&kind) else {
            continue;
        };
        notes.push(LedgerNote {
            id,
            task_id: TaskId::from(task_id),
            attempt_id: AttemptId(attempt_id.unsigned_abs()),
            kind,
            topic,
            text,
            at,
            closed_at,
            closed_why,
        });
    }
    Ok(notes)
}

fn ledger_note_counts(
    connection: &Connection,
    task_id: &TaskId,
) -> Result<LedgerNoteCounts, TaskError> {
    let mut counts = LedgerNoteCounts {
        rejected: 0,
        next: 0,
    };
    for kind in [LedgerNoteKind::Rejected, LedgerNoteKind::Next] {
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM ledger_note
             WHERE task_id = ?1 AND kind = ?2 AND closed_at IS NULL",
            params![task_id.as_str(), kind.as_str()],
            |row| row.get(0),
        )?;
        match kind {
            LedgerNoteKind::Rejected => counts.rejected = count.unsigned_abs(),
            LedgerNoteKind::Next => counts.next = count.unsigned_abs(),
        }
    }
    Ok(counts)
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
#[path = "graph_tests.rs"]
mod graph_tests;
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
#[cfg(test)]
#[path = "work_tests.rs"]
mod work_tests;

#[cfg(test)]
#[path = "accept_many_tests.rs"]
mod accept_many_tests;

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod ledger_tests;
