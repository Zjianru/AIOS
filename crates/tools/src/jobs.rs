//! 后台任务注册表：进程内、按会话隔离（照 DSH jobs/jobs-local 与 jobs/tool-jobs 的最小语义）。
//!
//! 每个任务是独立的 bash 子进程：stdout/stderr 由线程流式收集（内存保留尾部 + 完整流落
//! <data-dir>/jobs/session-<id>/<job>.{out,err}）。T006.9 照 DSH：
//! - id 是 `<kind>-<16 位小写十六进制 uuid>`（jobs/jobs/README.zh.md:32）；
//! - 注册/结算各通知一次生命周期回调（agent 写 job/registered、job/settled 会话事件）；
//! - 模型读输出按游标推进（DSH registry read），重复读不重复返回；
//! - 结算额外通知一次"完成通知"回调（await/first-delivery 语义，DSH tool-jobs 的 settled 投递）。

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ToolContext;
use crate::ToolFailure;
use crate::session::SpillStore;
use crate::spill::SpillFile;
use tokio::sync::Notify;

/// 同一会话同时运行的后台任务上限（照 DSH jobs-local 的并发约束；AIOS 先取 10）。
pub const MAX_RUNNING_JOBS: usize = 10;
/// 每个流在内存在保留的尾部字节数（照 DSH jobs-local retainBytes 默认 262,144）。
pub const RETAIN_BYTES: usize = 262_144;
/// 结算后每个流保留的尾部字节数（照 DSH jobs-local settledRetainBytes 默认 16,384）；
/// 模型游标尚未消费的字节优先保留，首次终态读之后才裁到这个上限。
pub const SETTLED_RETAIN_BYTES: usize = 16 * 1024;
/// job_output/job_kill 等待结算的上限（毫秒）。
pub const MAX_WAIT_MS: u64 = 600_000;
/// 前台流式 spill 的单流上限（照 DSH bash-local maxSpillBytes 64 MiB）。
pub const MAX_SPILL_BYTES: usize = 64 * 1024 * 1024;
/// 结束任务的 SIGTERM→SIGKILL grace（照 DSH bash-local graceMs 默认 3s）。
pub const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// 任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Completed,
    Failed,
    Killed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// 注册回调（agent 用它写 job/registered 会话事件；照 DSH jobs-local 的 recordSessionEvent）。
pub type JobRegisteredCallback = Arc<dyn Fn(&JobRegistered) + Send + Sync>;
/// 结算回调（agent 用它写 job/settled 会话事件；每次结算恰一次，与 await/通知无关）。
pub type JobSettledCallback = Arc<dyn Fn(&JobSettled) + Send + Sync>;
/// 完成通知回调（模型可见投递；照 DSH tool-jobs 的 settled 投递：awaited / 已送过的不再发）。
pub type JobNotifyCallback = Arc<dyn Fn(&JobSettled) + Send + Sync>;

/// 任务对外投影。
#[derive(Debug, Clone)]
pub struct JobView {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub detail: Option<String>,
}

/// 读一次任务输出（非消耗式快照）。
#[derive(Debug, Clone)]
pub struct JobRead {
    pub view: JobView,
    pub stdout: String,
    pub stderr: String,
    pub stdout_dropped: bool,
    pub stderr_dropped: bool,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub stdout_spill: Option<PathBuf>,
    pub stderr_spill: Option<PathBuf>,
}

/// 创建 job 的工具调用（DSH jobs 的 JobToolCall：嵌套调用保留根调用 id）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobToolCall {
    pub call_id: String,
    pub root_call_id: String,
}

/// 启动一个后台任务的输入。
pub struct JobSpec {
    pub command: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub label: String,
    /// 通过工具创建时带上（DSH job/registered 的 toolCall）。
    pub tool_call: Option<JobToolCall>,
}

/// job 注册时刻的生命周期记录（DSH job/registered 的 data）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRegistered {
    pub id: String,
    pub owner: Option<String>,
    pub kind: &'static str,
    pub label: String,
    pub started_at: u64,
    pub tool_call: Option<JobToolCall>,
}

/// 结算原因（DSH JobSettleCause）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCause {
    Producer,
    Kill,
    Teardown,
}

impl JobCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Producer => "producer",
            Self::Kill => "kill",
            Self::Teardown => "teardown",
        }
    }
}

/// job 结算时刻的生命周期记录（DSH job/settled 的 data）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSettled {
    pub id: String,
    pub owner: Option<String>,
    pub kind: &'static str,
    pub label: String,
    pub started_at: u64,
    pub tool_call: Option<JobToolCall>,
    pub finished_at: u64,
    pub status: JobStatus,
    pub cause: JobCause,
    pub producer_reported: bool,
    pub detail: Option<String>,
}

/// 模型游标读的结果（DSH registry read：只返回上次读之后的新输出）。
#[derive(Debug, Clone)]
pub struct JobOutputRead {
    pub view: JobView,
    pub stdout: String,
    pub stderr: String,
    pub lossy: bool,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

/// 前台 bash 的流式 spill 目标（T006.8 照 DSH subprocess spill：完整流边收边写）。
pub struct JobSpill {
    pub store: SpillStore,
    pub stdout_name: String,
    pub stderr_name: String,
}

struct JobState {
    status: JobStatus,
    exit_code: Option<i32>,
    signal: Option<i32>,
    started_at: u64,
    finished_at: Option<u64>,
    detail: Option<String>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_dropped: bool,
    stderr_dropped: bool,
    kill_requested: bool,
    /// 前台 bash 在 start 时标记：结算时不再发完成通知（照 DSH awaited）。
    awaited: bool,
    /// 完成通知已发出（或已补发）；保证最多一次。
    notified: bool,
    /// 前台 spill 文件的 locator（未开 spill 或超过整流体上限时为 None）。
    stdout_spill: Option<PathBuf>,
    stderr_spill: Option<PathBuf>,
    /// 绝对总字节数与模型游标（DSH read 语义：只返回上次读之后的新输出）。
    stdout_total: usize,
    stderr_total: usize,
    read_stdout: usize,
    read_stderr: usize,
}

struct JobRecord {
    id: String,
    kind: &'static str,
    label: String,
    child: Mutex<Child>,
    state: Mutex<JobState>,
    cond: Condvar,
    /// 创建 job 的工具调用（job/registered、job/settled 用）。
    tool_call: Option<JobToolCall>,
    /// settle 线程唤醒异步等待者（T006.8：wait_async 不阻塞 runtime）。
    notify: Notify,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

/// 会话级后台任务注册表。
pub struct TaskRegistry {
    jobs: Mutex<HashMap<String, Arc<JobRecord>>>,
    root: Option<PathBuf>,
    session_id: Option<String>,
    on_registered: Mutex<Option<JobRegisteredCallback>>,
    on_settled: Mutex<Option<JobSettledCallback>>,
    on_notify: Mutex<Option<JobNotifyCallback>>,
    /// T006.9c：注册时同步入队的 job/registered（driver 在 tool/result 提交前取走并落日志，
    /// 照 DSH registry.start 在工具执行内同步 session.append 的提交时点）。
    registered_pending: Mutex<Vec<JobRegistered>>,
    /// DSH tool-jobs 的 killedByModel 集合（跨层共享；判定在通知投递方，不进 session 事件）。
    killed_by_model: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for TaskRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TaskRegistry")
            .finish_non_exhaustive()
    }
}

impl TaskRegistry {
    pub fn new(data_dir: &Path, session_id: &str) -> Self {
        let root = data_dir.join("jobs").join(format!("session-{session_id}"));
        // 旧 data-dir 里的 jobs 目录/文件可能是 0755/0644：启动时修正权限；
        // 旧输出文件保留为历史归档（新 job id 唯一，不会 append 到它们）。
        prepare_jobs_root(&root);
        Self {
            jobs: Mutex::new(HashMap::new()),
            root: Some(root),
            session_id: Some(session_id.to_owned()),
            on_registered: Mutex::new(None),
            on_settled: Mutex::new(None),
            on_notify: Mutex::new(None),
            registered_pending: Mutex::new(Vec::new()),
            killed_by_model: Mutex::new(HashSet::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.root.is_some()
    }

    pub fn set_on_registered(&self, callback: JobRegisteredCallback) {
        *self.on_registered.lock().expect("jobs 回调锁被毒化") = Some(callback);
    }

    pub fn set_on_settled(&self, callback: JobSettledCallback) {
        *self.on_settled.lock().expect("jobs 回调锁被毒化") = Some(callback);
    }

    pub fn set_on_notify(&self, callback: JobNotifyCallback) {
        *self.on_notify.lock().expect("jobs 回调锁被毒化") = Some(callback);
    }

    /// 取走并清除标记（DSH 在结算事件处理时 `killedByModel.delete(id)`）。
    pub fn take_killed_by_model(&self, id: &str) -> bool {
        self.killed_by_model
            .lock()
            .expect("jobs killedByModel 锁被毒化")
            .remove(id)
    }

    /// T006.9c：取走工具执行期间同步登记的 job/registered（driver 在提交 tool/result 前落日志）。
    pub fn take_registered_pending(&self) -> Vec<JobRegistered> {
        std::mem::take(
            &mut *self
                .registered_pending
                .lock()
                .expect("jobs registered 队列锁被毒化"),
        )
    }

    fn settled_callback(&self) -> Option<JobSettledCallback> {
        self.on_settled.lock().expect("jobs 回调锁被毒化").clone()
    }

    fn notify_callback(&self) -> Option<JobNotifyCallback> {
        self.on_notify.lock().expect("jobs 回调锁被毒化").clone()
    }

    fn registered_callback(&self) -> Option<JobRegisteredCallback> {
        self.on_registered
            .lock()
            .expect("jobs 回调锁被毒化")
            .clone()
    }

    /// 起一个后台任务，返回 job id。
    pub fn start(&self, spec: JobSpec, foreground: bool) -> Result<String, ToolFailure> {
        self.start_inner(spec, foreground, None, RETAIN_BYTES)
    }

    /// T006.8：前台 bash 用流式 spill。每个流内存只留 retain_bytes 尾部，完整流边收边写进
    /// <data-dir>/spill/session-<hash>/ 的 spill 文件（照 DSH subprocess collect + bash-local）。
    pub fn start_with_spill(
        &self,
        spec: JobSpec,
        foreground: bool,
        spill: JobSpill,
        retain_bytes: usize,
    ) -> Result<String, ToolFailure> {
        self.start_inner(spec, foreground, Some(spill), retain_bytes)
    }

    fn start_inner(
        &self,
        spec: JobSpec,
        foreground: bool,
        spill: Option<JobSpill>,
        retain_bytes: usize,
    ) -> Result<String, ToolFailure> {
        let Some(root) = &self.root else {
            return Err(ToolFailure::plain(
                "background jobs are unavailable in this tool context",
            ));
        };
        let running = self
            .jobs
            .lock()
            .expect("jobs 表锁被毒化")
            .values()
            .filter(|job| {
                !job.state
                    .lock()
                    .expect("job 状态锁被毒化")
                    .status
                    .is_terminal()
            })
            .count();
        if running >= MAX_RUNNING_JOBS {
            return Err(ToolFailure::plain(format!(
                "cannot start background job: {MAX_RUNNING_JOBS} jobs already running"
            )));
        }
        crate::spill::create_private_dir(root).map_err(|error| {
            ToolFailure::plain(format!(
                "cannot create jobs directory {}: {error}",
                root.display()
            ))
        })?;
        let id = format!("bash-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let stdout_path = root.join(format!("{id}.out"));
        let stderr_path = root.join(format!("{id}.err"));
        let mut process = Command::new("bash");
        process
            .arg("-c")
            .arg(&spec.command)
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // T006.8：子进程自成一个进程组，SIGTERM/SIGKILL 能连子孙一起收（照 DSH subprocess 的 range kill）。
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            process.process_group(0);
        }
        process.env_clear();
        for (key, value) in &spec.env {
            process.env(key, value);
        }
        let mut child = process.spawn().map_err(|error| {
            ToolFailure::plain(format!(
                "cannot run bash in {}: {error}",
                spec.cwd.display()
            ))
        })?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (stdout_spill, stderr_spill) = match &spill {
            Some(target) => (
                SpillFile::create(&target.store, &target.stdout_name, MAX_SPILL_BYTES).ok(),
                SpillFile::create(&target.store, &target.stderr_name, MAX_SPILL_BYTES).ok(),
            ),
            None => (None, None),
        };
        let record = Arc::new(JobRecord {
            id: id.clone(),
            kind: "bash",
            label: spec.label,
            child: Mutex::new(child),
            state: Mutex::new(JobState {
                status: JobStatus::Running,
                exit_code: None,
                signal: None,
                started_at: now_millis(),
                finished_at: None,
                detail: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
                stdout_dropped: false,
                stderr_dropped: false,
                kill_requested: false,
                awaited: foreground,
                notified: false,
                stdout_spill: None,
                stderr_spill: None,
                stdout_total: 0,
                stderr_total: 0,
                read_stdout: 0,
                read_stderr: 0,
            }),
            cond: Condvar::new(),
            tool_call: spec.tool_call.clone(),
            notify: Notify::new(),
            stdout_path: stdout_path.clone(),
            stderr_path: stderr_path.clone(),
        });
        self.jobs
            .lock()
            .expect("jobs 表锁被毒化")
            .insert(id.clone(), Arc::clone(&record));
        let registered = {
            let state = record.state.lock().expect("job 状态锁被毒化");
            JobRegistered {
                id: id.clone(),
                owner: self.session_id.clone(),
                kind: record.kind,
                label: record.label.clone(),
                started_at: state.started_at,
                tool_call: spec.tool_call.clone(),
            }
        };
        // T006.9c：先同步入队（driver 在 tool/result 提交前取走落日志，照 DSH
        // registry.start 在工具执行内同步 session.append 的时点），再通知订阅回调。
        self.registered_pending
            .lock()
            .expect("jobs registered 队列锁被毒化")
            .push(registered.clone());
        if let Some(callback) = self.registered_callback() {
            callback(&registered);
        }

        let (sender, receiver) = std::sync::mpsc::channel::<ReaderDone>();
        spawn_reader(
            stdout,
            Arc::clone(&record),
            true,
            sender.clone(),
            stdout_spill,
            retain_bytes,
        );
        spawn_reader(
            stderr,
            Arc::clone(&record),
            false,
            sender,
            stderr_spill,
            retain_bytes,
        );
        let settled_callback = self.settled_callback();
        let notify_callback = self.notify_callback();
        let owner = self.session_id.clone();
        let tool_call = spec.tool_call.clone();
        let monitored = Arc::clone(&record);
        std::thread::spawn(move || {
            // 两个读线程都结束（完整流已落盘/spill）后再结算。
            let mut spills: [Option<PathBuf>; 2] = [None, None];
            for _ in 0..2 {
                match receiver.recv() {
                    Ok(done) => {
                        spills[usize::from(!done.is_stdout)] =
                            done.spill.map(|saved| saved.locator);
                    }
                    Err(_) => break,
                }
            }
            let (mut status, mut exit_code, mut signal) = (JobStatus::Failed, None, None);
            loop {
                let mut child = monitored.child.lock().expect("job 子进程锁被毒化");
                match child.try_wait() {
                    Ok(Some(exit)) => {
                        let classified = classify(exit);
                        status = classified.0;
                        exit_code = classified.1;
                        signal = classified.2;
                        break;
                    }
                    Ok(None) => {
                        drop(child);
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            let (settled, should_notify) = {
                let mut state = monitored.state.lock().expect("job 状态锁被毒化");
                state.status = status;
                state.exit_code = exit_code;
                state.signal = signal;
                state.finished_at = Some(now_millis());
                state.stdout_spill = spills[0].take();
                state.stderr_spill = spills[1].take();
                // DSH jobs-local settle：producer 事实在前（exit code / signal），killed 结算再追加
                // kill reason；已结算出正常状态的任务保留 producer 事实（reason 描述的是没落地的 kill）。
                let producer_detail = if status == JobStatus::Killed {
                    signal.map_or_else(
                        || "killed before exit".to_owned(),
                        |signal| format!("signal: {}", signal_name(signal)),
                    )
                } else {
                    format!("exit code: {}", exit_code.unwrap_or(0))
                };
                let detail = match (status, state.detail.take()) {
                    (JobStatus::Killed, Some(reason)) => {
                        format!("{producer_detail}; {reason}")
                    }
                    _ => producer_detail,
                };
                state.detail = Some(detail.clone());
                trim_settled_streams(&mut state);
                let settled = JobSettled {
                    id: monitored.id.clone(),
                    owner: owner.clone(),
                    kind: monitored.kind,
                    label: monitored.label.clone(),
                    started_at: state.started_at,
                    tool_call: tool_call.clone(),
                    finished_at: state.finished_at.unwrap_or_else(now_millis),
                    status,
                    // DSH jobs-local：settleCause 由 kill 请求置位，结算时照抄（请求后自然完成仍是 kill）。
                    cause: if state.kill_requested {
                        JobCause::Kill
                    } else {
                        JobCause::Producer
                    },
                    producer_reported: true,
                    detail: Some(detail),
                };
                let should_notify = !state.awaited && !state.notified;
                if should_notify {
                    state.notified = true;
                }
                (settled, should_notify)
            };
            monitored.cond.notify_all();
            monitored.notify.notify_waiters();
            if let Some(callback) = &settled_callback {
                callback(&settled);
            }
            if should_notify && let Some(callback) = &notify_callback {
                callback(&settled);
            }
        });
        Ok(id)
    }

    /// 读一次输出快照；任务不存在返回错误。
    pub fn output(&self, id: &str) -> Result<JobRead, ToolFailure> {
        let record = self.record(id)?;
        Ok(read_of(&record))
    }

    /// 本会话全部任务，按启动时间排序。
    pub fn list(&self) -> Vec<JobView> {
        let jobs = self.jobs.lock().expect("jobs 表锁被毒化");
        let mut views: Vec<JobView> = jobs.values().map(|record| view_of(record)).collect();
        views.sort_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        views
    }

    /// 等任务结算（最多 timeout）；到时未结算返回当前快照。
    pub fn wait(&self, id: &str, timeout: Duration) -> Result<JobRead, ToolFailure> {
        let record = self.record(id)?;
        let deadline = Instant::now() + timeout;
        let mut state = record.state.lock().expect("job 状态锁被毒化");
        while !state.status.is_terminal() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (next, _) = record
                .cond
                .wait_timeout(state, deadline - now)
                .expect("job 条件变量等待失败");
            state = next;
        }
        drop(state);
        Ok(read_of(&record))
    }

    /// T006.8：异步等结算（tokio Notify；不阻塞 runtime）。
    pub async fn wait_async(&self, id: &str, timeout: Duration) -> Result<JobRead, ToolFailure> {
        let record = self.record(id)?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.settled(&record) {
                return Ok(read_of(&record));
            }
            let notified = record.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.settled(&record) {
                return Ok(read_of(&record));
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            if tokio::time::timeout(remaining, notified).await.is_err() {
                break;
            }
        }
        Ok(read_of(&record))
    }

    fn settled(&self, record: &Arc<JobRecord>) -> bool {
        record
            .state
            .lock()
            .expect("job 状态锁被毒化")
            .status
            .is_terminal()
    }

    /// 请求取消（DSH registry.kill 的同步前半）：已终态返回 false（already-finished），
    /// 否则置 kill_requested/detail 并返回 true（requested）。`by_model` 时在**同一个 state 锁内**
    /// 置 killedByModel 标记（DSH tool-jobs 在 `result === requested` 后 add）：标记必须早于
    /// 任何可能的结算回调可见，否则“请求后立刻自然完成”的窗口里会漏掉抑制。
    pub fn request_kill(
        &self,
        id: &str,
        reason: Option<String>,
        by_model: bool,
    ) -> Result<bool, ToolFailure> {
        let record = self.record(id)?;
        let mut state = record.state.lock().expect("job 状态锁被毒化");
        if state.status.is_terminal() {
            return Ok(false);
        }
        state.kill_requested = true;
        if let Some(reason) = reason {
            state.detail = Some(reason);
        }
        // 锁顺序固定 state → killed_by_model；settle 线程先拿 state 锁，故标记一定先于结算可见。
        if by_model {
            self.killed_by_model
                .lock()
                .expect("jobs killedByModel 锁被毒化")
                .insert(id.to_owned());
        }
        Ok(true)
    }

    /// 等任务结束（DSH kill 的异步后半）：先 SIGTERM，等 grace（默认 3s）仍不退再 SIGKILL；
    /// grace 期间读线程继续收输出，退出信号按进程实际终止信号记录。
    pub async fn await_termination(
        &self,
        id: &str,
        grace: Duration,
    ) -> Result<JobRead, ToolFailure> {
        let record = self.record(id)?;
        if self.settled(&record) {
            return Ok(read_of(&record));
        }
        Self::signal_group(&record, "-TERM");
        if !Self::wait_terminal(&record, grace).await {
            Self::signal_group(&record, "-KILL");
            let _ = Self::wait_terminal(&record, Duration::from_secs(5)).await;
        }
        Ok(read_of(&record))
    }

    /// 结束任务（请求取消 + 等终止的组合；T006.8 起的旧入口保留，行为不变）。
    /// grace 期间读线程继续收输出，退出信号按进程实际终止信号记录。
    pub async fn terminate(
        &self,
        id: &str,
        reason: Option<String>,
        grace: Duration,
    ) -> Result<JobRead, ToolFailure> {
        if !self.request_kill(id, reason, false)? {
            let record = self.record(id)?;
            return Ok(read_of(&record));
        }
        self.await_termination(id, grace).await
    }

    /// 前台 bash 超时转后台：原子地清掉 awaited；若在超时与这里之间已经结算且还没通知，
    /// 立即补发一次完成通知（生命周期事件在结算时已写过，这里不再重复写）。
    pub fn promote(&self, id: &str) {
        let Ok(record) = self.record(id) else {
            return;
        };
        let notify_callback = self.notify_callback();
        let settled = {
            let mut state = record.state.lock().expect("job 状态锁被毒化");
            state.awaited = false;
            if state.status.is_terminal() && !state.notified {
                state.notified = true;
                Some(JobSettled {
                    id: record.id.clone(),
                    owner: self.session_id.clone(),
                    kind: record.kind,
                    label: record.label.clone(),
                    started_at: state.started_at,
                    tool_call: record.tool_call.clone(),
                    finished_at: state.finished_at.unwrap_or_else(now_millis),
                    status: state.status,
                    cause: if state.kill_requested {
                        JobCause::Kill
                    } else {
                        JobCause::Producer
                    },
                    producer_reported: true,
                    detail: state.detail.clone(),
                })
            } else {
                None
            }
        };
        if let (Some(settled), Some(callback)) = (settled, notify_callback) {
            callback(&settled);
        }
    }

    /// 前台已收走结果的任务从表里删掉（不再出现在 job_list），并清掉输出文件；
    /// 同时清 killedByModel 标记（DSH 的 removed 事件里 delete）。
    pub fn remove(&self, id: &str) {
        let _ = self.take_killed_by_model(id);
        let removed = self.jobs.lock().expect("jobs 表锁被毒化").remove(id);
        if let Some(record) = removed {
            let _ = std::fs::remove_file(&record.stdout_path);
            let _ = std::fs::remove_file(&record.stderr_path);
        }
    }

    /// 等任务进入终态（tokio Notify；返回 true=已终态）。
    async fn wait_terminal(record: &Arc<JobRecord>, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if record
                .state
                .lock()
                .expect("job 状态锁被毒化")
                .status
                .is_terminal()
            {
                return true;
            }
            let notified = record.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return false;
            }
        }
    }

    /// 给整个进程组发信号（/bin/kill 的负 pid；子进程 start 时 process_group(0)）。
    #[cfg(unix)]
    fn signal_group(record: &Arc<JobRecord>, signal: &str) {
        let pid = record.child.lock().expect("job 子进程锁被毒化").id();
        let _ = std::process::Command::new("/bin/kill")
            .args([signal, "--", &format!("-{pid}")])
            .status();
    }

    #[cfg(not(unix))]
    fn signal_group(_record: &Arc<JobRecord>, _signal: &str) {}

    /// 模型游标读（DSH registry read）：只返回上次读之后的新输出并推进游标；落后于环形保留
    /// 窗口时标记 lossy（调用方提示完整流文件），重复读不会重复返回。
    pub fn read_output(&self, id: &str) -> Result<JobOutputRead, ToolFailure> {
        let record = self.record(id)?;
        let mut state = record.state.lock().expect("job 状态锁被毒化");
        let read = cursor_read(&record, &state);
        state.read_stdout = state.stdout_total;
        state.read_stderr = state.stderr_total;
        // 照 DSH jobs-local readJob：终态读是结算流裁到 settledRetainBytes 的时点；
        // 游标已经推到末尾，这之后不再需要为模型保留更多字节。
        if state.status.is_terminal() {
            trim_terminal_streams(&mut state);
        }
        Ok(read)
    }

    /// 同 `read_output` 但不推进游标：完成通知先看装不装得下，装得下才消耗这次读
    /// （DSH tool-jobs 的 held read：不内联就不吃掉 job_output 要读的数据）。
    pub fn peek_output(&self, id: &str) -> Result<JobOutputRead, ToolFailure> {
        let record = self.record(id)?;
        let state = record.state.lock().expect("job 状态锁被毒化");
        Ok(cursor_read(&record, &state))
    }

    fn record(&self, id: &str) -> Result<Arc<JobRecord>, ToolFailure> {
        self.jobs
            .lock()
            .expect("jobs 表锁被毒化")
            .get(id)
            .cloned()
            .ok_or_else(|| ToolFailure::plain(format!("job not found: {id}")))
    }
}

/// 建/修 jobs 目录：目录 0700、文件 0600；旧文件只修权限不删（归档策略）。
fn prepare_jobs_root(root: &Path) {
    if crate::spill::create_private_dir(root).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
            let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(mode));
        }
        #[cfg(not(unix))]
        let _ = metadata;
    }
}

/// 读线程结束时的 spill 结果（T006.8）。
struct ReaderDone {
    is_stdout: bool,
    spill: Option<crate::spill::SpillRef>,
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    record: Arc<JobRecord>,
    is_stdout: bool,
    done: std::sync::mpsc::Sender<ReaderDone>,
    mut spill: Option<SpillFile>,
    retain_bytes: usize,
) {
    std::thread::spawn(move || {
        let path = if is_stdout {
            record.stdout_path.clone()
        } else {
            record.stderr_path.clone()
        };
        // 每个 job 的输出文件唯一：create/truncate + 0600，绝不 append 旧文件。
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).ok();
        #[cfg(unix)]
        if let Some(file) = file.as_ref() {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if let Some(file) = file.as_mut() {
                        let _ = file.write_all(&buffer[..read]);
                    }
                    if let Some(spill) = spill.as_mut() {
                        spill.write(&buffer[..read]);
                    }
                    let mut state = record.state.lock().expect("job 状态锁被毒化");
                    if is_stdout {
                        state.stdout_total += read;
                    } else {
                        state.stderr_total += read;
                    }
                    let target = if is_stdout {
                        &mut state.stdout
                    } else {
                        &mut state.stderr
                    };
                    target.extend_from_slice(&buffer[..read]);
                    if target.len() > retain_bytes {
                        let overflow = target.len() - retain_bytes;
                        target.drain(..overflow);
                        if is_stdout {
                            state.stdout_dropped = true;
                        } else {
                            state.stderr_dropped = true;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let spill = spill.and_then(SpillFile::finish);
        let _ = done.send(ReaderDone { is_stdout, spill });
    });
}

/// bash 结算照 DSH shell/tool-bash background.ts 的 processOutcome：只有 killed 与 completed 两类
/// （命令跑了非零退出仍是 completed，退出码进 detail）；没有退出码也没有信号才是基础设施失败。
/// 子进程信号名（DSH jobs-local 的 producer 事实写成 `signal: SIGTERM`，不是数字；T006.19）。
pub fn signal_name(signal: i32) -> String {
    let name = match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        17 => "SIGCHLD",
        18 => "SIGCONT",
        19 => "SIGSTOP",
        20 => "SIGTSTP",
        21 => "SIGTTIN",
        22 => "SIGTTOU",
        23 => "SIGURG",
        24 => "SIGXCPU",
        25 => "SIGXFSZ",
        26 => "SIGVTALRM",
        27 => "SIGPROF",
        28 => "SIGWINCH",
        29 => "SIGIO",
        #[cfg(target_os = "macos")]
        7 => "SIGEMT",
        #[cfg(target_os = "macos")]
        8 => "SIGFPE",
        #[cfg(target_os = "macos")]
        10 => "SIGBUS",
        #[cfg(target_os = "macos")]
        12 => "SIGSYS",
        #[cfg(target_os = "macos")]
        30 => "SIGUSR1",
        #[cfg(target_os = "macos")]
        31 => "SIGUSR2",
        #[cfg(target_os = "linux")]
        7 => "SIGBUS",
        #[cfg(target_os = "linux")]
        8 => "SIGFPE",
        #[cfg(target_os = "linux")]
        10 => "SIGUSR1",
        #[cfg(target_os = "linux")]
        12 => "SIGUSR2",
        #[cfg(target_os = "linux")]
        16 => "SIGSTKFLT",
        #[cfg(target_os = "linux")]
        31 => "SIGSYS",
        _ => return format!("signal {signal}"),
    };
    name.to_owned()
}

fn classify(exit: ExitStatus) -> (JobStatus, Option<i32>, Option<i32>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = exit.signal() {
            return (JobStatus::Killed, None, Some(signal));
        }
    }
    match exit.code() {
        Some(code) => (JobStatus::Completed, Some(code), None),
        None => (JobStatus::Failed, None, None),
    }
}

/// 结算裁剪（照 DSH jobs-local settle）：保留 `max(settledRetainBytes, 游标未消费字节)`，
/// 在模型首次终态读之前不丢它还没读到的内容。
fn trim_settled_streams(state: &mut JobState) {
    let stdout_keep =
        SETTLED_RETAIN_BYTES.max(state.stdout_total.saturating_sub(state.read_stdout));
    let stderr_keep =
        SETTLED_RETAIN_BYTES.max(state.stderr_total.saturating_sub(state.read_stderr));
    trim_stream(&mut state.stdout, stdout_keep, &mut state.stdout_dropped);
    trim_stream(&mut state.stderr, stderr_keep, &mut state.stderr_dropped);
}

/// 终态读之后：游标已到末尾，两个流都裁到结算保留量。
fn trim_terminal_streams(state: &mut JobState) {
    trim_stream(
        &mut state.stdout,
        SETTLED_RETAIN_BYTES,
        &mut state.stdout_dropped,
    );
    trim_stream(
        &mut state.stderr,
        SETTLED_RETAIN_BYTES,
        &mut state.stderr_dropped,
    );
}

fn trim_stream(stream: &mut Vec<u8>, keep: usize, dropped: &mut bool) {
    if stream.len() <= keep {
        return;
    }
    let overflow = stream.len() - keep;
    stream.drain(..overflow);
    *dropped = true;
}

fn view_of(record: &JobRecord) -> JobView {
    let state = record.state.lock().expect("job 状态锁被毒化");
    view_locked(record, &state)
}

fn view_locked(record: &JobRecord, state: &JobState) -> JobView {
    JobView {
        id: record.id.clone(),
        kind: record.kind,
        label: record.label.clone(),
        status: state.status,
        exit_code: state.exit_code,
        signal: state.signal,
        started_at: state.started_at,
        finished_at: state.finished_at,
        detail: state.detail.clone(),
    }
}

/// 从模型游标取一次增量（不推进游标；`read_output` 负责推进，`peek_output` 只看）。
fn cursor_read(record: &JobRecord, state: &JobState) -> JobOutputRead {
    let stdout_start = state.stdout_total.saturating_sub(state.stdout.len());
    let stderr_start = state.stderr_total.saturating_sub(state.stderr.len());
    // 游标落后于内存保留窗口：前面的字节已被淘汰，这次读从窗口起点开始并标记 lossy。
    let lossy = state.read_stdout < stdout_start || state.read_stderr < stderr_start;
    let stdout_from = state.read_stdout.max(stdout_start);
    let stderr_from = state.read_stderr.max(stderr_start);
    let stdout = String::from_utf8_lossy(
        &state.stdout[stdout_from - stdout_start..state.stdout_total - stdout_start],
    )
    .into_owned();
    let stderr = String::from_utf8_lossy(
        &state.stderr[stderr_from - stderr_start..state.stderr_total - stderr_start],
    )
    .into_owned();
    JobOutputRead {
        view: view_locked(record, state),
        stdout,
        stderr,
        lossy,
        stdout_path: record.stdout_path.clone(),
        stderr_path: record.stderr_path.clone(),
    }
}

/// 把一次游标读渲染成模型可见增量（照 DSH tool-jobs renderModelDelta）：stdout 与无标记块按序，
/// 然后 stderr 收进一个 `[stderr]` 段；游标落后被淘汰时以丢输出提示收尾并给出完整流文件。
pub fn render_model_delta(read: &JobOutputRead) -> String {
    let mut body = read.stdout.clone();
    if !read.stderr.is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("[stderr]\n");
        body.push_str(&read.stderr);
    }
    if read.lossy {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&format!(
            "[some output was dropped from memory; full output: {}, {}]",
            read.stdout_path.display(),
            read.stderr_path.display()
        ));
    }
    body
}

fn read_of(record: &JobRecord) -> JobRead {
    let state = record.state.lock().expect("job 状态锁被毒化");
    JobRead {
        view: JobView {
            id: record.id.clone(),
            kind: record.kind,
            label: record.label.clone(),
            status: state.status,
            exit_code: state.exit_code,
            signal: state.signal,
            started_at: state.started_at,
            finished_at: state.finished_at,
            detail: state.detail.clone(),
        },
        stdout: String::from_utf8_lossy(&state.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&state.stderr).into_owned(),
        stdout_dropped: state.stdout_dropped,
        stderr_dropped: state.stderr_dropped,
        stdout_path: record.stdout_path.clone(),
        stderr_path: record.stderr_path.clone(),
        stdout_spill: state.stdout_spill.clone(),
        stderr_spill: state.stderr_spill.clone(),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// 父环境里哪些变量不能隐式传给模型 shell（照 DSH subprocess scrubbedParentEnv）。
fn is_sensitive_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("DSH_")
        || ["KEY", "PASSWORD", "SECRET", "TOKEN"]
            .iter()
            .any(|pattern| upper.contains(pattern))
}

/// 子进程环境：继承（vars_os lossy，去 AIOS_* 与敏感名）+ 终端覆盖 + 受管 AIOS_* 变量。
pub fn child_env(context: &ToolContext) -> Vec<(String, String)> {
    let mut variables: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            if key.starts_with("AIOS_") || is_sensitive_env_var(&key) {
                return None;
            }
            Some((key, value.to_string_lossy().into_owned()))
        })
        .collect();
    for (key, value) in [
        ("NO_COLOR", "1"),
        ("TERM", "dumb"),
        ("PAGER", "cat"),
        ("GIT_PAGER", "cat"),
    ] {
        variables.push((key.to_owned(), value.to_owned()));
    }
    variables.push(("AIOS_SHELL".to_owned(), "1".to_owned()));
    if let Some(session_id) = context.session().session_id() {
        variables.push(("AIOS_SESSION_ID".to_owned(), session_id.to_owned()));
    }
    if let Some(data_dir) = context.session().data_dir() {
        variables.push(("AIOS_HOME".to_owned(), data_dir.display().to_string()));
    }
    variables
}
#[cfg(test)]
mod tests {
    use super::is_sensitive_env_var;

    #[test]
    fn credential_names_are_sensitive() {
        for name in [
            "DEEPSEEK_API_KEY",
            "deepseek_api_key",
            "GITHUB_TOKEN",
            "db_password",
            "MY_SECRET",
            "DSH_HOME",
            "HOMEBREW_GITHUB_API_TOKEN",
        ] {
            assert!(is_sensitive_env_var(name), "{name} 应被 scrub");
        }
    }

    #[test]
    fn normal_names_are_kept() {
        for name in [
            "PATH",
            "HOME",
            "HTTPS_PROXY",
            "NO_PROXY",
            "NO_COLOR",
            "TERM",
            "PAGER",
            "GIT_PAGER",
            "LANG",
        ] {
            assert!(!is_sensitive_env_var(name), "{name} 不应被 scrub");
        }
    }
}

/// T006.19 红测试：job 结算 detail 的 signal 用名字（DSH jobs-local `signal: SIGTERM`）。
#[cfg(test)]
mod t006_19_signal_tests {
    use super::*;

    #[test]
    fn signal_names_follow_dsh_producer_detail() {
        assert_eq!(signal_name(9), "SIGKILL");
        assert_eq!(signal_name(15), "SIGTERM");
        assert_eq!(signal_name(2), "SIGINT");
        assert_eq!(signal_name(9999), "signal 9999");
    }
}
