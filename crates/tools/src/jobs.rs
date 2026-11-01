//! 后台任务注册表：进程内、按会话隔离（照 DSH jobs/jobs-local 与 jobs/tool-jobs 的最小语义）。
//!
//! 每个任务是独立的 bash 子进程：stdout/stderr 由线程流式收集（内存保留尾部 + 完整流落
//! <data-dir>/jobs/session-<id>/<job>.{out,err}）；结算后通知注册回调（agent 用来发
//! JobCompleted 事件与把完成通知送进下一步输入）。id 用 bash-<n> 递增（DSH 用短 uuid）。

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ToolContext;
use crate::ToolFailure;

static JOB_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 同一会话同时运行的后台任务上限（照 DSH jobs-local 的并发约束；AIOS 先取 10）。
pub const MAX_RUNNING_JOBS: usize = 10;
/// 每个流在内存在保留的尾部字节数（照 DSH jobs-local retainBytes 默认 262,144）。
pub const RETAIN_BYTES: usize = 262_144;
/// job_output/job_kill 等待结算的上限（毫秒）。
pub const MAX_WAIT_MS: u64 = 600_000;

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

/// 一次结算通知的回调（agent 在收到时发事件并把通知投进会话）。
pub type JobSettledCallback = Arc<dyn Fn(&str, JobStatus) + Send + Sync>;

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
}

/// 启动一个后台任务的输入。
pub struct JobSpec {
    pub command: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub label: String,
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
}

struct JobRecord {
    id: String,
    kind: &'static str,
    label: String,
    child: Mutex<Child>,
    state: Mutex<JobState>,
    cond: Condvar,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

/// 会话级后台任务注册表。
pub struct TaskRegistry {
    jobs: Mutex<HashMap<String, Arc<JobRecord>>>,
    root: Option<PathBuf>,
    on_settled: Mutex<Option<JobSettledCallback>>,
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
            on_settled: Mutex::new(None),
        }
    }

    /// 工具单元测试等无会话目录的场景：任务不可用。
    pub fn detached() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            root: None,
            on_settled: Mutex::new(None),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.root.is_some()
    }

    pub fn set_on_settled(&self, callback: JobSettledCallback) {
        *self.on_settled.lock().expect("jobs 回调锁被毒化") = Some(callback);
    }

    fn callback(&self) -> Option<JobSettledCallback> {
        self.on_settled.lock().expect("jobs 回调锁被毒化").clone()
    }

    /// 起一个后台任务，返回 job id。
    pub fn start(&self, spec: JobSpec, foreground: bool) -> Result<String, ToolFailure> {
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
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.subsec_nanos());
        let id = format!(
            "bash-{}-{nanos}-{}",
            std::process::id(),
            JOB_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
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
            }),
            cond: Condvar::new(),
            stdout_path: stdout_path.clone(),
            stderr_path: stderr_path.clone(),
        });
        self.jobs
            .lock()
            .expect("jobs 表锁被毒化")
            .insert(id.clone(), Arc::clone(&record));

        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        spawn_reader(stdout, Arc::clone(&record), true, sender.clone());
        spawn_reader(stderr, Arc::clone(&record), false, sender);
        let callback = self.callback();
        let monitored = Arc::clone(&record);
        std::thread::spawn(move || {
            // 两个读线程都结束（完整流已落盘）后再结算。
            let _ = receiver.recv();
            let _ = receiver.recv();
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
            let should_notify = {
                let mut state = monitored.state.lock().expect("job 状态锁被毒化");
                if state.kill_requested {
                    status = JobStatus::Killed;
                }
                state.status = status;
                state.exit_code = exit_code;
                state.signal = signal;
                state.finished_at = Some(now_millis());
                let should_notify = !state.awaited && !state.notified;
                if should_notify {
                    state.notified = true;
                }
                drop(state);
                monitored.cond.notify_all();
                should_notify
            };
            if should_notify && let Some(callback) = callback {
                callback(&monitored.id, status);
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

    /// 前台 bash 超时转后台：原子地清掉 awaited；若在超时与这里之间已经结算且还没通知，
    /// 立即补发一次完成通知。
    pub fn promote(&self, id: &str) {
        let Ok(record) = self.record(id) else {
            return;
        };
        let callback = self.callback();
        let settled_status = {
            let mut state = record.state.lock().expect("job 状态锁被毒化");
            state.awaited = false;
            if state.status.is_terminal() && !state.notified {
                state.notified = true;
                Some(state.status)
            } else {
                None
            }
        };
        if let (Some(status), Some(callback)) = (settled_status, callback) {
            callback(id, status);
        }
    }

    /// 请求取消（SIGKILL，无 grace）并等结算；返回最新快照。
    pub fn kill(&self, id: &str, reason: Option<String>) -> Result<JobRead, ToolFailure> {
        let record = self.record(id)?;
        {
            let mut child = record.child.lock().expect("job 子进程锁被毒化");
            let _ = child.kill();
        }
        {
            let mut state = record.state.lock().expect("job 状态锁被毒化");
            state.kill_requested = true;
            if let Some(reason) = reason {
                state.detail = Some(reason);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
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

    /// 前台已收走结果的任务从表里删掉（不再出现在 job_list），并清掉输出文件。
    pub fn remove(&self, id: &str) {
        let removed = self.jobs.lock().expect("jobs 表锁被毒化").remove(id);
        if let Some(record) = removed {
            let _ = std::fs::remove_file(&record.stdout_path);
            let _ = std::fs::remove_file(&record.stderr_path);
        }
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

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    record: Arc<JobRecord>,
    is_stdout: bool,
    done: std::sync::mpsc::Sender<()>,
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
                    let mut state = record.state.lock().expect("job 状态锁被毒化");
                    let target = if is_stdout {
                        &mut state.stdout
                    } else {
                        &mut state.stderr
                    };
                    target.extend_from_slice(&buffer[..read]);
                    if target.len() > RETAIN_BYTES {
                        let overflow = target.len() - RETAIN_BYTES;
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
        let _ = done.send(());
    });
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
        Some(0) => (JobStatus::Completed, Some(0), None),
        Some(code) => (JobStatus::Failed, Some(code), None),
        None => (JobStatus::Failed, None, None),
    }
}

fn view_of(record: &JobRecord) -> JobView {
    let state = record.state.lock().expect("job 状态锁被毒化");
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
