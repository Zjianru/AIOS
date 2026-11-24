//! T024.3：任务执行者（launch）、ThinAgent 的 task_report、上报 relay，以及 Kind 的两个任务工具。
//!
//! 事实边界：任务聚合和效果表在 tasks；这里把效果泵成真实动作（开 ThinAgent 会话、
//! 把报告 Steer 回发起的 Kind 会话），不自己保存任务状态。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent::{AgentEvent, AgentOpenOptions, ToolPolicy};
use model::{ContentBlock, Effort, UserMessage};
use serde_json::{Value, json};
use session::SessionId;
use tasks::{
    Acceptance, AppointmentOrigin, AppointmentRole, Attempt, AttemptEnd, AttemptId, AttemptTrigger,
    BootId, BootScan, Cancellation, CancellationReason, ContinueCause, ConversationId, EffectClaim,
    EffectKind, EffectPayload, GateCandidate, GateCheck, GateCheckOutcome, GateCommand,
    GateConclusion, Handoff, Input, InputAttachment, MissingItem, ModelChoice, NewGraph, NewInput,
    NewReport, NewSubtask, NewTask, Report, ReportId, ReportOrigin, ReportState, RoundEnd,
    SlotKind, Stage, StopAttempt, Store, Task, TaskError, TaskId, TaskPolicy, WorkerSlots,
};
use tokio::sync::broadcast;
use tools::{
    JobRead, JobSpec, MAX_OUTPUT_BYTES, TaskRegistry, Tool, ToolCall, ToolCondition, ToolContext,
    ToolFailure, ToolFuture, ToolResult,
};

use crate::error::ServiceError;
use crate::events::{page_events, service_event};
use crate::input::attachment_error;
use crate::protocol::{
    DeliveryMode, FollowRequest, FollowResponse, GateJudgeConclusion as ProtocolGateConclusion,
    GateJudgeRequest, GateJudgeResponse, GateVerdictView, ServiceEvent, StopMode,
    TaskContinueRequest, TaskContinueResponse, TaskInterruptResponse, TaskReadHeader,
    TaskReadReport, TaskReadReportOrigin, TaskReadReportState, TaskReadRequest, TaskReadResponse,
    TaskSayRequest, TaskSayResponse, TaskStopResponse,
};
use crate::root::RootSessionOptions;
use crate::session_host::ServiceHost;

mod calendar_api;
mod calendar_cycle;
mod calendar_tools;
pub use calendar_tools::AppointmentCancelTool;
mod calendar_watch;
mod task_tools;
#[doc(hidden)]
pub use task_tools::TaskStopTool;
pub(crate) use task_tools::*;

mod delivery;
mod gating;
mod ledger;
mod report;
use gating::*;
use report::*;
#[cfg(test)]
mod ledger_tests;
#[cfg(test)]
mod missing_tool_policy_tests;
#[cfg(test)]
mod program_gate_tests;
#[cfg(test)]
mod title_limit_tests;

/// 任务库文件名（数据目录下）。
pub const TASKS_DB_FILE: &str = "tasks.sqlite";

/// ThinAgent 的只读工具名单（构造期 only 过滤；顺序就是名单顺序）。
pub const THIN_TOOLS: [&str; 11] = [
    "read",
    "glob",
    "grep",
    "skill",
    "todo_write",
    "web_search",
    "web_fetch",
    "task_report",
    "task_handback",
    "task_ledger_note",
    "task_ledger_history",
];

/// 写任务的 ThinAgent 工具名单：只读 9 件加 write、edit（D85）。
pub const THIN_WRITE_TOOLS: [&str; 13] = [
    "read",
    "glob",
    "grep",
    "write",
    "edit",
    "skill",
    "todo_write",
    "web_search",
    "web_fetch",
    "task_report",
    "task_handback",
    "task_ledger_note",
    "task_ledger_history",
];

/// 开了 bash 的写任务：写任务 11 件再加 bash 和后台任务三件（D85）。
pub const THIN_WRITE_BASH_TOOLS: [&str; 17] = [
    "read",
    "glob",
    "grep",
    "write",
    "edit",
    "bash",
    "job_output",
    "job_list",
    "job_kill",
    "skill",
    "todo_write",
    "web_search",
    "web_fetch",
    "task_report",
    "task_handback",
    "task_ledger_note",
    "task_ledger_history",
];

/// 默认角色的规则（ThinAgent 的 policy_prompt，追加在系统提示末尾；Kind 不装）。
///
/// bash 开关决定 shell/后台那一段的说法（D85）：关着时没有跑命令的手段；开着时
/// 明说 bash 和后台任务在，且写集不约束它们——命令按用户的完全权限跑。
fn thin_policy(bash: bool) -> String {
    let shell = if bash {
        "You also have bash and the background job tools; the write scope does not constrain them, and commands run with the user's full permissions. Use them only for this task. You cannot delegate or ask a person a question directly."
    } else {
        "You cannot run shell commands, start background work, delegate, or ask a person a question directly."
    };
    format!(
        "You are a ThinAgent: the execution carrier for one task. \
The task's requirement and DoD are the whole scope; work inside them and inside the task's working directory.
If the user says something outside this task's scope, asks for a different task, or asks to change the requirement or the DoD, call task_handback once with one sentence saying what exceeded the task; do not do that work yourself.
If the user only thanks you or acknowledges the result, do not reply.
You can read, search, load skills, keep a todo list, read the web, and call task_report or task_handback. \
When the assignment has a write_scope, you also have write and edit, and they only work inside that scope; if what the user asks needs a path outside it, call task_handback instead of trying to get around it. \
{shell}
Your ledger tools keep durable notes for a later attempt: task_ledger_note records a rejected approach with its reason, or what the next attempt should do when you stop or hand back; task_ledger_history reads all lines. Use a short topic; a new line with the same kind and topic replaces the old one. Record progress only through task_report, never in the ledger. \
When the requirement or DoD names a specific tool, command, or approach (for example, \"take the date with the date command\"), that approach is part of the task scope: if this task does not have that tool or permission (for example bash is off), report blocked on the first step with exactly what is missing and what the user should do, and put that direct question in question when the user must decide; do not substitute another tool, do not simulate its output, and do not wait until the ready report to mention it. \
Do not report progress, and do not claim work you did not do."
    )
}

/// 向 ThinAgent 要状态时的输入（照 DSH STATUS_REQUEST）。
const STATUS_REQUEST: &str = "Report the current task status with task_report. This is a status-only request: do not resume implementation or perform external actions.";

/// 一轮结束但 ThinAgent 什么都没说时的 relay 摘要（照 spec 的提示）。
const WAITING_FALLBACK: &str = "ThinAgent 没上报，用 task_status 去问";

/// 任务没在跑时的固定原因；页面和 Kind 工具用同一句话。
const NOT_RUNNING: &str = "这个任务没在跑";

/// 收尾停插进 ThinAgent 会话的固定说明（spec v1:44）；request id 是 task-stop:<taskId>:<attemptId>。
const AFTER_REPORT_STOP: &str =
    "用户要求收尾停：做完手上这一步，用 task_report 报一次当前结果，然后结束。不再调其他工具。";

/// 交回时程序写死的唯一问题（spec v1 T028.4）；Kind 收到 relay 后怎么问由 Kind 自己说。
const HANDBACK_QUESTION: &str = "这超出了任务范围，要不要改任务？";

/// 开了 bash 的写任务，结果卡上的固定说明（spec 故事 7；原文进卡数据，由结果卡渲染）。
const BASH_NOTE: &str = "这个任务开了 bash，bash 的改动不在清单里";

/// cwd 不是 git 仓库时开不了工作树的固定说明（卡上照实写）。
const WORKTREE_NOT_A_REPO: &str = "这个目录不是 git 仓库，开不了工作树";

/// 立即停任务时写给后台命令的取消原因（进 job 的 detail，给人和日志看）。
const STOPPED_JOB_REASON: &str = "任务被立即停了";
/// ThinAgent 一轮结束后等后台命令的上限；不进设置。
const BACKGROUND_LIMIT_MS: i64 = 30 * 60 * 1000;
const BACKGROUND_STOPPED_NOTE: &str = "这些命令跑过 30 分钟被停了";

/// 确认接续后给 ThinAgent 的第一条输入（照 DSH RECOVERY_NOTE，换成 AIOS 的话；spec v1）。
const RESUME_NOTE: &str = "AIOS 在这个任务进行中重启了。先看当前的文件和状态再继续；已经做完的外部动作不要再做一遍。把原来的任务做完，然后用 task_report 上报。";

/// 重启汇报里一份最近报告最多带多少字（spec v1：last_report ≤ 300）。
const RESTART_REPORT_CHARS: usize = 300;

/// 执行者的一个已开会话；task_status 向 waiting 的 ThinAgent 要报告时用它。
struct RunningThin {
    host: Arc<ServiceHost>,
    task_id: TaskId,
    attempt_id: AttemptId,
    /// 报告后还有后台命令，宿主保留着只为停/等最后一条命令。
    retained: bool,
}

/// 接上原会话的结果：新 Attempt 和它的宿主；第一条消息由调用方送。
struct OpenedResume {
    host: Arc<ServiceHost>,
    attempt_id: AttemptId,
}

/// Kind 的 task_accept 解析后的参数：建任务并冻结执行策略需要的全部字段。
struct TaskAcceptArgs {
    title: String,
    quote: String,
    requirement: String,
    dod: String,
    kind_note: Option<String>,
    cwd: Option<String>,
    project: Option<String>,
    write_scope: Option<String>,
    worktree: bool,
    constraints: Vec<String>,
}

/// T052：task_graph_propose 的一个子任务参数。
#[derive(Debug)]
struct GraphSubtaskArgs {
    title: String,
    requirement: String,
    dod: String,
    write_scope: Option<String>,
    worktree: bool,
    /// 本提议里其他子任务的下标（0 起）。
    after: Vec<usize>,
    /// T053：里程碑（只读核查门）；after 恰好一项，指向它把关的任务。
    milestone: bool,
    /// T057：程序门命令（与 milestone 二选一，milestone 必须 true）。
    program_gate: Vec<GateCommand>,
}

/// T052：task_graph_propose 的参数：总任务 + 至少两个子任务。
#[derive(Debug)]
struct TaskGraphArgs {
    title: String,
    quote: String,
    requirement: String,
    dod: String,
    kind_note: Option<String>,
    cwd: Option<String>,
    project: Option<String>,
    subtasks: Vec<GraphSubtaskArgs>,
    constraints: Vec<String>,
}

/// 任务没在跑时接着开一次的方式：普通说话，或重启打断后的接续。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeMode {
    /// 停/结束之后的普通说话：trigger user_message，person 席位。
    Say,
    /// 重启打断后的说话：trigger resume，wake 席位，第一条输入先放恢复说明。
    Interrupted,
}

/// 接不上原会话的原因；说话接续和确认卡接续按各自的方式处理。
enum ResumeBlock {
    /// 现在接不上，晚点再来（会话写者没让出、没席位、正在关机）。
    Later,
    /// 另一个 Attempt 已经先接上了。
    AlreadyStarted,
    /// 没有可续的会话；原因给用户看。
    NoSession(String),
    /// 任务包缺项；缺项报告已经写好。
    PackageMissing,
}

/// 任务事件流的打开结果：当前 Attempt 的会话 id、打开时要补的日志事件，
/// 以及任务还在跑时的 live 订阅面（不在跑就只补，不出 live 帧）。
pub struct TaskStream {
    pub session_id: String,
    pub follow: FollowResponse,
    pub live: Option<TaskLive>,
    /// T058：任务事实变化（门判定/命令版本/接受）的轻量广播；不落日志。
    pub changed: broadcast::Receiver<String>,
}

/// 在跑任务的 live 订阅面。
///
/// 宿主句柄也放在这里：拿到流之后即使执行者已经把这个 Attempt 从「在跑」表里摘掉，
/// 事件仍会从同一个宿主送到连接关闭；不会因为收尾而把半轮事件截断。
pub struct TaskLive {
    pub host: Arc<ServiceHost>,
    pub events: broadcast::Receiver<ServiceEvent>,
    pub agent_events: broadcast::Receiver<AgentEvent>,
}

/// 监控 WS 的一个在跑任务：补齐快照 + live 事件接收端。
pub struct MonitorLiveTask {
    pub task_id: String,
    pub attempt_id: u64,
    pub follow: FollowResponse,
    pub events: broadcast::Receiver<ServiceEvent>,
}

/// 监控 WS 打开面：各在跑任务的事件 + 任务事实变化广播。
pub struct MonitorLive {
    pub tasks: Vec<MonitorLiveTask>,
    pub changed: broadcast::Receiver<String>,
}

/// 任务执行者：效果泵、ThinAgent 会话和任务工具的共享状态。
/// 停止入口对 agent 状态的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentInterrupt {
    /// 正在跑：取消当前一轮（cancel_with 同时杀这一轮命令）。
    Cancel,
    /// 一轮已结束但后台命令还在跑：杀那一轮命令。
    KillTurn(u64),
}

pub struct TaskRuntime {
    data_dir: PathBuf,
    store: Arc<Store>,
    options: RootSessionOptions,
    /// 根会话当前的选择；task_accept 在这一刻冻结它（D26、D74）。
    selection: Arc<Mutex<Option<session::ModelSelection>>>,
    /// 根宿主的会话槽：relay 只走它，不拿卡锁。弱引用，免得任务工具和它成环；
    /// 根宿主没了就升级失败，relay 留到下次启动。
    inner: Weak<tokio::sync::Mutex<Option<ServiceHost>>>,
    notify: Arc<tokio::sync::Notify>,
    shutting_down: Arc<AtomicBool>,
    /// 测试钩子：暂停整个效果泵（launch/relay 都不跑）。
    pump_paused: AtomicBool,
    /// 等后台的到点时窗；生产固定 30 分钟，测试调短不用真等。
    background_limit_ms: AtomicI64,
    /// 全局暂停（settings.json 的 `exec.paused`）：只挡 launch 准入，relay 照送。
    exec_paused: AtomicBool,
    /// 席位上限（settings.json 的 `exec.seats`），默认 3；改小只影响之后的准入。
    exec_seats: AtomicUsize,
    /// 默认角色的 bash 开关（settings.json 的 `exec.bash`），默认关；任务受理时读它冻结。
    exec_bash: AtomicBool,
    /// 起跑预约，按任务 id 和在跑集合合并计数；launch 成功前占住席位。
    slots: Mutex<WorkerSlots>,
    /// 还活着的 ThinAgent（按会话 id）；终态报告后移除，释放会话写锁。
    running: Mutex<std::collections::HashMap<String, RunningThin>>,
    /// 有宿主移出在跑表时通知续接等锁的人。
    session_relinquished: tokio::sync::Notify,
    /// 测试钩子：效果泵醒来的次数。
    pump_wakes: AtomicUsize,
    /// T057：正在跑的程序门检查（按任务 id）；进程组停任务用。
    gate_jobs: Mutex<std::collections::HashMap<String, Arc<RunningGate>>>,
    /// T058：任务事实变化（门判定/命令版本/接受）广播；任务 WS 用它在页面开着时重读。
    changed: broadcast::Sender<String>,
    /// 根宿主已打开的项目库；建任务解析归属/目录共用这一份，不每步重开。
    projects: Option<Arc<tasks::ProjectStore>>,
    /// 测试钩子：下一次 relay 在 Kind append 前「崩掉」——留下效果行，泵退出。
    crash_before_relay: AtomicBool,
    /// 日历库；打不开就没有日历能力。
    calendar: Option<Arc<calendar::CalendarStore>>,
    /// calendar/changed 的推送端（由根宿主注入）。
    calendar_events: Option<broadcast::Sender<ServiceEvent>>,
    /// 测试钩子：下一次日历唤醒在送进 Kind 前失败一次，留下效果行等退避重试。
    crash_before_wake: AtomicBool,
    /// 全局暂停开始的毫秒时间；漏掉的多次触发落在暂停期间还是服务没在跑，用它分辨。
    global_pause_since: AtomicI64,
    /// 由约定到点开的那些 Kind 轮：key=(根会话,turn)，值=那次的约定/运行。
    /// task_accept/task_graph_propose 用它给新建任务标 dispatched，不靠模型参数。
    trigger_turns: Mutex<std::collections::HashMap<(String, u64), AppointmentOrigin>>,
    /// 测试钩子：盯变化的 URL 抓取用这个出站环境（wiremock 代理）。
    watch_http: std::sync::Mutex<Option<model::http::HttpEnv>>,
}

fn quote_source_text(source: &tasks::QuoteSource) -> String {
    match source {
        tasks::QuoteSource::UserMessage { conversation, seq } => {
            format!("用户消息 {conversation} seq {seq}")
        }
        tasks::QuoteSource::Appointment { appointment_id } => {
            format!("约定 {appointment_id}")
        }
    }
}

impl TaskRuntime {
    /// 退出收尾：先挡新 Attempt，再停所有在跑/保留宿主的命令，关宿主，清表。
    pub(crate) async fn shutdown_all(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let hosts: Vec<Arc<ServiceHost>> = {
            let mut running = self.running.lock().expect("thinagent 表锁被毒化");
            running.drain().map(|(_, entry)| entry.host).collect()
        };
        for host in hosts {
            let agent = host.agent();
            agent.cancel_with(session::AbortCause::User);
            agent
                .stop_background_jobs("AIOS 退出，后台命令一起停")
                .await;
            host.shutdown().await;
        }
        self.session_relinquished.notify_waiters();
        self.wake();
    }

    pub(crate) fn new(
        options: &RootSessionOptions,
        selection: Arc<Mutex<Option<session::ModelSelection>>>,
        inner: Weak<tokio::sync::Mutex<Option<ServiceHost>>>,
        calendar_events: Option<broadcast::Sender<ServiceEvent>>,
    ) -> Result<Self, TaskError> {
        let store = Store::open(options.data_dir.join(TASKS_DB_FILE))?;
        let changed = broadcast::channel(64).0;
        {
            let sender = changed.clone();
            store.set_on_changed(Arc::new(move |task_id: &str| {
                drop(sender.send(task_id.to_owned()));
            }));
        }
        // 重启后暂停和席位还在：启动时读一次设置文件，缺了按默认。
        let exec = crate::settings::load(&options.data_dir).exec;
        let exec_paused = exec.paused;
        let exec_seats = exec.seat_limit();
        let exec_bash = exec.bash;
        Ok(Self {
            data_dir: options.data_dir.clone(),
            store: Arc::new(store),
            options: options.clone(),
            selection,
            inner,
            notify: Arc::new(tokio::sync::Notify::new()),
            shutting_down: Arc::new(AtomicBool::new(false)),
            pump_paused: AtomicBool::new(false),
            background_limit_ms: AtomicI64::new(BACKGROUND_LIMIT_MS),
            exec_paused: AtomicBool::new(exec_paused),
            exec_seats: AtomicUsize::new(exec_seats),
            exec_bash: AtomicBool::new(exec_bash),
            slots: Mutex::new(WorkerSlots::new()),
            running: Mutex::new(std::collections::HashMap::new()),
            session_relinquished: tokio::sync::Notify::new(),
            pump_wakes: AtomicUsize::new(0),
            gate_jobs: Mutex::new(std::collections::HashMap::new()),
            changed,
            projects: options.projects.clone(),
            crash_before_relay: AtomicBool::new(false),
            calendar: calendar::CalendarStore::open(options.data_dir.join("calendar.sqlite"))
                .ok()
                .map(Arc::new),
            calendar_events,
            crash_before_wake: AtomicBool::new(false),
            watch_http: std::sync::Mutex::new(None),
            global_pause_since: AtomicI64::new(0),
            trigger_turns: Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// 根 WS 转发 task-changed 用。
    pub(crate) fn subscribe_task_changed(&self) -> broadcast::Receiver<String> {
        self.changed.subscribe()
    }

    /// 项目库变了也走 task-changed，让页面重读项目 store；空 id 表示不是某个任务。
    pub(crate) fn notify_projects_changed(&self) {
        drop(self.changed.send(String::new()));
    }

    /// 任务库（测试和诊断读同一份事实）。
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// 在任务对话里说一句：记输入、排 delivery 和 notice，然后叫醒效果泵。
    pub(crate) async fn say_task(
        &self,
        request: TaskSayRequest,
    ) -> Result<TaskSayResponse, ServiceError> {
        // 附件和 Kind 对话共用仓库与校验：先按 id 读回只读副本，错就按同一套错误码返回。
        let mut attachments = Vec::with_capacity(request.attachments.len());
        for attachment in &request.attachments {
            tools::attachments::read(&self.data_dir, &attachment.id).map_err(attachment_error)?;
            attachments.push(InputAttachment {
                id: attachment.id.clone(),
                name: attachment.name.clone(),
                bytes: attachment.bytes,
            });
        }
        let id = TaskId::from(request.task_id);
        let outcome = self
            .store
            .say_input(
                &id,
                NewInput {
                    text: request.text,
                    pastes: request.pastes,
                    attachments,
                },
                Self::now(),
            )
            .map_err(|error| match error {
                TaskError::EmptyInput => ServiceError::MessageEmpty,
                TaskError::Cancelled => {
                    ServiceError::TaskUnavailable(tasks::CANCELLED_MESSAGE.to_owned())
                }
                other => ServiceError::TaskUnavailable(format!("这句话没记进任务库：{other}")),
            })?;
        self.wake();
        Ok(TaskSayResponse {
            input_id: outcome.input_id.as_str().to_owned(),
            new_attempt: outcome.new_attempt.map(|attempt| attempt.0),
        })
    }

    /// 用户接受一张结果卡：只接受这张卡指向的、当前要求版本的 ready 报告。
    ///
    /// 返回接受记录；重复接受由 store 保证返回同一条。错误文字直接给卡片看。
    pub(crate) fn accept_result(
        &self,
        task_id: &str,
        report_id: &str,
    ) -> Result<Acceptance, String> {
        let id = TaskId::from(task_id);
        let report = ReportId::from(report_id);
        // 校验顺序、回原因和幂等都由 store 一个事务做（T054）：
        // 找不到/非 ready、门任务、不是最新、版本旧、门没判通过各回各的原因。
        let acceptance = self
            .store
            .accept(&id, &report, Self::now())
            .map_err(|error| error.to_string())?;
        Ok(acceptance)
    }

    /// T053：Kind 让任务在原要求里接着做（用户判了需修正 / 最近报告卡住失败等）。
    ///
    /// 同一 Task、原会话、新 Attempt；不改要求、不改写集、不开 bash。
    pub(crate) async fn continue_task(
        self: &Arc<Self>,
        request: TaskContinueRequest,
    ) -> Result<TaskContinueResponse, String> {
        let task_id = TaskId::from(request.task_id.as_str());
        let task = self
            .store
            .task(&task_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("找不到这个任务：{}", request.task_id))?;
        if let Some(reason) = task.cancelled_message() {
            return Err(reason.to_owned());
        }
        if let Some(reason) = Self::trigger_task_rejection(&task) {
            return Err(reason);
        }
        if task.is_graph_root() {
            return Err("总任务没有执行承载，不能接着做".to_owned());
        }
        if task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none())
        {
            return Err("这个任务还在跑；等这一轮结束再让它接着做".to_owned());
        }
        let cause = if let Some(cycle) = self.calendar_cycle_continue_cause(&task)? {
            cycle
        } else {
            self.continue_cause(&task)?
        };
        let package = self.continue_package(&task, &cause, request.note.as_deref())?;
        let Some(attempt) = task.latest_attempt().cloned() else {
            return Err("这个任务还没开跑，不能接着做".to_owned());
        };
        // T053：按上游当前可放行报告交接（需重评时拿到的是新结果）。
        let handoffs = self
            .store
            .current_handoffs(&task)
            .map_err(|error| error.to_string())?;
        let trigger = AttemptTrigger::KindContinue {
            cause,
            note: request.note.clone(),
        };
        let opened = match self
            .open_resume(&task, &attempt, trigger, SlotKind::Person, handoffs)
            .await
        {
            Ok(opened) => opened,
            Err(ResumeBlock::Later) => {
                return Err("现在接不上这个任务（没有空位或会话还在收尾），稍后再试一次".to_owned());
            }
            Err(ResumeBlock::AlreadyStarted) => return Err("已经接着做了".to_owned()),
            Err(ResumeBlock::PackageMissing) => {
                return Err("任务包缺了东西，接不上；看任务里的缺项".to_owned());
            }
            Err(ResumeBlock::NoSession(reason)) => return Err(reason),
        };
        let request_id = format!("continue:{}:{}", task.id, opened.attempt_id);
        let sent = opened
            .host
            .send_message(
                &request_id,
                UserMessage::text(package),
                DeliveryMode::FollowUp,
            )
            .await;
        if let Err(error) = sent {
            opened.host.shutdown().await;
            self.forget_running(opened.host.agent().id().as_str());
            best_effort(
                "接着做的 Attempt 没收尾",
                self.store.end_attempt(
                    &task.id,
                    opened.attempt_id,
                    Self::now(),
                    AttemptEnd::NotOpened,
                ),
            );
            self.release_seat(task.id.as_str());
            return Err(format!("接着做的说明没送进任务会话：{error}"));
        }
        self.wake();
        Ok(TaskContinueResponse {
            attempt_id: opened.attempt_id.0,
        })
    }

    /// T053：认 task_continue 的原因（当前候选需修正 → 最近报告卡住/失败/等报告）。
    fn continue_cause(&self, task: &Task) -> Result<ContinueCause, String> {
        if let Some(gate) = self
            .store
            .guarding_gate(&task.id)
            .map_err(|error| error.to_string())?
            && let Some(latest) = task.latest_report()
            && let Some(verdict) = gate.verdicts.iter().rev().find(|verdict| {
                verdict.candidate.report_id == latest.id
                    && verdict.conclusion == GateConclusion::NeedsFix
            })
        {
            return Ok(ContinueCause::GateNeedsFix {
                gate_report: verdict.gate_report.clone(),
                candidate: verdict.candidate.clone(),
            });
        }
        // T053：上游有新可放行结果 → 需重评（D95）。
        if self
            .store
            .needs_reevaluation(&task.id)
            .map_err(|error| error.to_string())?
            && let Some(upstream_report) = self.reevaluation_report(task)?
        {
            return Ok(ContinueCause::UpstreamChanged { upstream_report });
        }
        if let Some(report) = task.latest_report()
            && matches!(
                report.state,
                ReportState::Blocked | ReportState::Failed | ReportState::Waiting
            )
        {
            return Ok(ContinueCause::AfterReport {
                report: report.id.clone(),
            });
        }
        Err("没找到让它接着做的事实：用户还没判需修正，最近报告也不是卡住/失败/等报告，上游也没有新结果".to_owned())
    }

    /// T053：需重评时，第一条和当前可放行报告不一致的上游报告 id。
    fn reevaluation_report(&self, task: &Task) -> Result<Option<ReportId>, String> {
        let Some(attempt) = task.latest_attempt() else {
            return Ok(None);
        };
        for (index, need) in task.needs.iter().enumerate() {
            let Some(current) = self
                .store
                .releasable_report(&need.from)
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            match attempt.handoffs.get(index) {
                Some(handoff) if handoff.report_id == current.id => {}
                _ => return Ok(Some(current.id)),
            }
        }
        Ok(None)
    }

    /// T053：接着做的任务包说明（需修正时带用户原话和核查发现）。
    fn continue_package(
        &self,
        task: &Task,
        cause: &ContinueCause,
        note: Option<&str>,
    ) -> Result<String, String> {
        let mut lines = self.version_quote_lines(task);
        let has_quotes = !lines.is_empty();
        if has_quotes {
            lines.push(String::new());
        }
        let requirement_label = if has_quotes {
            "Kind 的理解（不是用户原话）"
        } else {
            "原要求不变"
        };
        match cause {
            ContinueCause::GateNeedsFix { gate_report, .. } => {
                lines.push("你要按这个改。".to_owned());
                lines.push(format!("{requirement_label}：{}", task.requirement));
                if let Some(note) = self.latest_kind_note(task) {
                    lines.push(format!("Kind 的补充（不算用户要求）：{note}"));
                }
                if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
                    lines.push(format!("用户原话：{note}"));
                }
                if let Some(gate) = self
                    .store
                    .guarding_gate(&task.id)
                    .map_err(|error| error.to_string())?
                    && let Some(report) =
                        gate.reports.iter().find(|report| &report.id == gate_report)
                {
                    lines.push(format!("核查发现：{}", report.summary));
                }
                match task.effective_write_scope() {
                    Some(scope) => lines.push(format!("写集不变：{}", scope.display())),
                    None => lines.push("写集不变：只读".to_owned()),
                }
                lines
                    .push("改完用 task_report 上报；核查会自动重跑，结果仍然由用户判。".to_owned());
            }
            ContinueCause::UpstreamChanged { upstream_report } => {
                lines.push("上游有新结果，按新结果核对。".to_owned());
                lines.push(format!("{requirement_label}：{}", task.requirement));
                if let Some(note) = self.latest_kind_note(task) {
                    lines.push(format!("Kind 的补充（不算用户要求）：{note}"));
                }
                for need in &task.needs {
                    let Ok(Some(upstream)) = self.store.task(&need.from) else {
                        continue;
                    };
                    let Ok(Some(current)) = self.store.releasable_report(&need.from) else {
                        continue;
                    };
                    if &current.id == upstream_report {
                        lines.push(format!(
                            "上游结果（{}）：{}",
                            upstream.title, current.summary
                        ));
                        break;
                    }
                }
                match task.effective_write_scope() {
                    Some(scope) => lines.push(format!("写集不变：{}", scope.display())),
                    None => lines.push("写集不变：只读".to_owned()),
                }
                lines.push("核对完用 task_report 上报。".to_owned());
            }
            ContinueCause::CycleDue { appointment_id, .. } => {
                lines.push("循环约定到点：这一轮报回是约定叫的，接着做下一轮。".to_owned());
                lines.push(format!("{requirement_label}：{}", task.requirement));
                if let Some(note) = self.latest_kind_note(task) {
                    lines.push(format!("Kind 的补充（不算用户要求）：{note}"));
                }
                if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
                    lines.push(format!("用户原话：{note}"));
                }
                if let Some(report) = task.latest_report() {
                    lines.push(format!("上一轮报告：{}", report.summary));
                }
                lines.push(format!("约定：{appointment_id}"));
                match task.effective_write_scope() {
                    Some(scope) => lines.push(format!("写集不变：{}", scope.display())),
                    None => lines.push("写集不变：只读".to_owned()),
                }
                lines.push("做完用 task_report 上报。".to_owned());
            }
            ContinueCause::AfterReport { report } => {
                let (state, summary) = task
                    .reports
                    .iter()
                    .find(|report_now| &report_now.id == report)
                    .map(|report_now| {
                        (
                            report_state_key(report_now.state).to_owned(),
                            report_now.summary.clone(),
                        )
                    })
                    .unwrap_or_else(|| ("unknown".to_owned(), String::new()));
                lines.push("接着做：上一次没交出可用的结果。".to_owned());
                if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
                    lines.push(format!("用户原话：{note}"));
                }
                lines.push(format!("上一次报告（{state}）：{summary}"));
                match task.effective_write_scope() {
                    Some(scope) => lines.push(format!("写集不变：{}", scope.display())),
                    None => lines.push("写集不变：只读".to_owned()),
                }
                lines.push("做完用 task_report 上报。".to_owned());
            }
        }
        let ledger = self.ledger_section(task);
        if !ledger.is_empty() {
            lines.push(String::new());
            lines.push(ledger);
        }
        Ok(lines.join(
            "
",
        ))
    }

    pub(crate) async fn task_read(
        &self,
        request: TaskReadRequest,
    ) -> Result<TaskReadResponse, ServiceError> {
        let task = self.task(&request.task_id)?;
        let attempt = task
            .latest_attempt()
            .filter(|attempt| attempt.conversation.is_some())
            .cloned();
        let (all, selected, last_seq) = match &attempt {
            Some(attempt) => {
                let conversation = attempt.conversation.clone().expect("上面筛过");
                let session_id = SessionId::parse(conversation.as_str()).map_err(|error| {
                    ServiceError::TaskUnavailable(format!("任务对话的会话 id 不合法：{error}"))
                })?;
                let snapshot = session::SessionStore::open_read_only(&self.data_dir, &session_id)
                    .map_err(|error| {
                    ServiceError::TaskUnavailable(format!("任务对话打不开：{error}"))
                })?;
                let all = snapshot.events().to_vec();
                let (selected, _) = page_events(&all, request.before_seq, 500, None, None);
                let events: Vec<ServiceEvent> =
                    selected.iter().map(|event| service_event(event)).collect();
                let last_seq = all.last().map(|event| event.seq).unwrap_or(0);
                (all, events, last_seq)
            }
            None => (Vec::new(), Vec::new(), 0),
        };
        // T051：统计由服务端算（和 Kind status 同一个 session_stats），用任务冻结的模型；
        // 对当前 Attempt 会话的全部事件算，不受分页影响。上下文窗口按冻结连接/模型查，
        // 查不到给 0（页面照旧隐藏占比）。
        let selection = session::ModelSelection {
            connection: task.model.connection.clone(),
            model: task.model.model.clone(),
            effort_wanted: task.model.effort_wanted.clone(),
        };
        let settings = crate::settings::load(&self.data_dir);
        let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
        let context_window = crate::session_select::model_context_window(
            &self.data_dir,
            self.options.identity.as_deref(),
            &selection,
        )
        .unwrap_or(0);
        let context_used = Self::context_used_of(&all);
        let stats = match credentials::Keyring::open_read_only(
            self.data_dir.clone(),
            self.options.identity.as_deref(),
        )
        .ok()
        {
            Some(keyring) => {
                let view = keyring.view();
                let mut stats = crate::stats::session_stats(
                    &all,
                    &view.connections,
                    &catalog,
                    &selection,
                    context_used,
                    context_window,
                );
                stats.effort_actual = view
                    .connections
                    .iter()
                    .find(|connection| connection.id == selection.connection)
                    .map(|connection| crate::stats::actual_effort_of(connection, &selection))
                    .unwrap_or_else(|| selection.effort_wanted.clone());
                stats
            }
            None => crate::stats::SessionStats {
                context_used,
                context_window,
                connection: selection.connection.clone(),
                model: selection.model.clone(),
                effort_wanted: selection.effort_wanted.clone(),
                ..crate::stats::SessionStats::default()
            },
        };
        // 页面热路径：只读汇率缓存，不发起网络请求（status 那边负责刷新缓存）。
        let fx = crate::fx::cached_or_fallback(&self.data_dir);
        let attempt_id = attempt.as_ref().map(|attempt| attempt.id);
        let latest_report = attempt_id.and_then(|attempt_id| {
            task.reports
                .iter()
                .rev()
                .find(|report| report.attempt_id == attempt_id)
        });
        let report = latest_report.map(|report| TaskReadReport {
            state: task_read_report_state(report.state),
            origin: task_read_report_origin(report.origin),
            summary: report.summary.clone(),
            question: report.question.clone(),
            attempt: report.attempt_id.0,
            checks: report
                .checks
                .iter()
                .map(|check| crate::protocol::TaskReadGateCheck {
                    index: check.index,
                    command: check.command.clone(),
                    exit_code: check.exit_code,
                    outcome: gate_check_outcome_key(check.outcome).to_owned(),
                    duration_ms: check.duration_ms,
                    tail: check.tail.clone(),
                    spill: check.spill.clone(),
                })
                .collect(),
        });
        let review = latest_report.is_some_and(|report| is_review_report(&task, report));
        let accepted = latest_report.is_some_and(|report| {
            report.version == task.requirement_version()
                && task.acceptance_for(&report.id).is_some()
        });
        // retained 宿主只表示 backgroundRunning；running 语义不变。
        let background_running =
            attempt_id.is_some_and(|attempt_id| self.background_running(&task.id, attempt_id));
        let running = attempt_id.is_some_and(|attempt_id| {
            self.running_for(&task.id, attempt_id)
                .is_some_and(|running| !running.retained && running.host.agent().is_running())
        });
        let guard_target = task
            .needs
            .first()
            .and_then(|need| self.store.task(&need.from).ok().flatten());
        let gate_view = task.gate.as_ref().map(|gate| {
            let verdict = task
                .latest_report()
                .and_then(|report| task.verdict_for_gate_report(&report.id));
            let verdicts = task
                .verdicts
                .iter()
                .rev()
                .map(|verdict| crate::protocol::TaskReadGateVerdict {
                    candidate_ordinal: guard_target.as_ref().and_then(|guarded| {
                        ready_report_ordinal(guarded, &verdict.candidate.report_id)
                    }),
                    conclusion: match verdict.conclusion {
                        GateConclusion::Pass => "pass",
                        GateConclusion::NeedsFix => "needs_fix",
                        GateConclusion::Undetermined => "undetermined",
                    }
                    .to_owned(),
                    note: verdict.note.clone(),
                    at: verdict.at,
                    judged_by: match verdict.judged_by {
                        tasks::VerdictSource::User => "user",
                        tasks::VerdictSource::Program => "program",
                    }
                    .to_owned(),
                })
                .collect();
            let attempts = task
                .reports
                .iter()
                .rev()
                .map(|report| crate::protocol::TaskReadGateAttempt {
                    attempt: report.attempt_id.0,
                    at: report.at,
                    summary: report.summary.clone(),
                })
                .collect();
            crate::protocol::TaskReadGate {
                commands: gate
                    .commands
                    .iter()
                    .map(|command| crate::protocol::TaskReadGateCommand {
                        command: command.command.clone(),
                        timeout_ms: command.timeout_ms,
                    })
                    .collect(),
                commands_version: gate.commands_version,
                judged_by: verdict.map(|verdict| match verdict.judged_by {
                    tasks::VerdictSource::User => "user".to_owned(),
                    tasks::VerdictSource::Program => "program".to_owned(),
                }),
                conclusion: verdict.map(|verdict| match verdict.conclusion {
                    GateConclusion::Pass => "pass".to_owned(),
                    GateConclusion::NeedsFix => "needs_fix".to_owned(),
                    GateConclusion::Undetermined => "undetermined".to_owned(),
                }),
                runs: task
                    .reports
                    .iter()
                    .filter(|report| report.origin == ReportOrigin::Program)
                    .rev()
                    .map(|report| {
                        let verdict = task.verdict_for_gate_report(&report.id);
                        crate::protocol::TaskReadGateRun {
                            attempt: report.attempt_id.0,
                            commands_version: verdict.map_or(0, |verdict| verdict.commands_version),
                            conclusion: match verdict.map(|verdict| verdict.conclusion) {
                                Some(GateConclusion::Pass) => "pass",
                                Some(GateConclusion::NeedsFix) => "needs_fix",
                                Some(GateConclusion::Undetermined) | None => "undetermined",
                            }
                            .to_owned(),
                            at: report.at,
                            checks: task_read_gate_checks(&report.checks),
                        }
                    })
                    .collect(),
                verdicts,
                attempts,
            }
        });

        let (program_running, program_index) = self
            .gate_jobs
            .lock()
            .expect("程序门表锁被毒化")
            .get(task.id.as_str())
            .map(|running| {
                (
                    true,
                    *running
                        .current_index
                        .lock()
                        .expect("程序门当前命令锁被毒化"),
                )
            })
            .unwrap_or((false, None));
        let ledger_view =
            crate::ledger::ledger_for(&self.store, &task).map_err(ServiceError::TaskUnavailable)?;
        let ledger = crate::protocol::TaskReadLedger {
            constraints: ledger_view
                .constraints
                .iter()
                .map(|constraint| crate::protocol::TaskReadLedgerConstraint {
                    text: constraint.text.clone(),
                    from: constraint.from.clone(),
                })
                .collect(),
            notes: ledger_view
                .rejected
                .iter()
                .chain(ledger_view.next.iter())
                .map(|note| crate::protocol::TaskReadLedgerNote {
                    id: note.id.clone(),
                    kind: match note.kind {
                        tasks::LedgerNoteKind::Rejected => "rejected".to_owned(),
                        tasks::LedgerNoteKind::Next => "next".to_owned(),
                    },
                    text: note.text.clone(),
                    attempt: note.attempt,
                })
                .collect(),
            history: ledger_view
                .history
                .iter()
                .map(|note| crate::protocol::TaskReadLedgerHistory {
                    id: note.id.clone(),
                    kind: match note.kind {
                        tasks::LedgerNoteKind::Rejected => "rejected".to_owned(),
                        tasks::LedgerNoteKind::Next => "next".to_owned(),
                    },
                    text: note.text.clone(),
                    attempt: note.attempt,
                    closed_at: note.closed_at,
                    closed_why: note.closed_why.clone(),
                })
                .collect(),
        };
        let guarding_gate = if task.is_gate() {
            None
        } else {
            self.store
                .guarding_gate(&task.id)
                .ok()
                .flatten()
                .map(|gate| {
                    let verdict = gate
                        .latest_report()
                        .and_then(|report| gate.verdict_for_gate_report(&report.id));
                    crate::protocol::TaskReadGuardingGate {
                        id: gate.id.as_str().to_owned(),
                        title: gate.title.clone(),
                        program: gate.is_program_gate(),
                        conclusion: verdict.map(|verdict| match verdict.conclusion {
                            GateConclusion::Pass => "pass".to_owned(),
                            GateConclusion::NeedsFix => "needs_fix".to_owned(),
                            GateConclusion::Undetermined => "undetermined".to_owned(),
                        }),
                        commands_version: gate.commands_version(),
                        at: verdict.map(|verdict| verdict.at),
                    }
                })
        };
        Ok(TaskReadResponse {
            task: TaskReadHeader {
                id: task.id.as_str().to_owned(),
                title: task.title.clone(),
                requirement: task.requirement.clone(),
                requirement_versions: self.requirement_version_views(&task),
                dod: task.dod.clone(),
                cwd: task.cwd.to_string_lossy().into_owned(),
                write_scope: crate::result_cards::write_scope_display(&task),
                bash: task.bash,
                cancelled_at: task.cancelled.as_ref().map(|cancelled| cancelled.at),
                project_id: task.project_id.clone(),
                cwd_source: task.cwd_source.clone(),
                graph_root: task.is_graph_root(),
                graph_member: task.is_graph_root() || task.parent.is_some(),
                model: task.model.model.clone(),
                tools: task.tools.clone(),
                attempts: task.runs.iter().map(|run| run.attempts.len() as u64).sum(),
                running,
                background_running,
                stopped: task
                    .latest_attempt()
                    .is_some_and(|attempt| attempt.end == Some(AttemptEnd::Stopped)),
                interrupted: task.is_interrupted(),
                restart_pending: task.is_interrupted()
                    && self.store.restart_pending(&task.id).map_err(|error| {
                        ServiceError::TaskUnavailable(format!("重启汇报的状态读不出来：{error}"))
                    })?,
                reported_at: latest_report
                    .filter(|report| {
                        matches!(
                            report.state,
                            ReportState::Ready | ReportState::Blocked | ReportState::Failed
                        )
                    })
                    .map(|report| report.at),
                report,
                review,
                accepted,
                gate: gate_view,
                program_running,
                program_index,
                guarding_gate,
            },
            events: selected,
            last_seq,
            stats: Some(stats),
            fx: Some(fx),
            currency: Some(settings.currency),
            ledger,
        })
    }

    /// 任务要求版本给页面的只读视图；子任务/门/里程碑标继承来源=总任务标题。
    fn requirement_version_views(
        &self,
        task: &Task,
    ) -> Vec<crate::protocol::RequirementVersionView> {
        let inherited_from = task.parent.as_ref().and_then(|parent| {
            self.store
                .task(parent)
                .ok()
                .flatten()
                .map(|root| root.title)
        });
        task.versions
            .iter()
            .enumerate()
            .map(|(index, version)| crate::protocol::RequirementVersionView {
                version: u32::try_from(index + 1).unwrap_or(u32::MAX),
                quote: version.quote.clone(),
                kind_note: version.kind_note.clone(),
                inherited_from: inherited_from.clone(),
                quote_source: version.quote_source.as_ref().map(quote_source_text),
            })
            .collect()
    }

    /// 任务聚合：读任务库；找不到/读不出来都带可读原因。
    fn task(&self, task_id: &str) -> Result<Task, ServiceError> {
        self.store
            .task(&TaskId::from(task_id))
            .map_err(|error| ServiceError::TaskUnavailable(format!("任务读不出来：{error}")))?
            .ok_or_else(|| ServiceError::TaskUnavailable(format!("找不到这个任务：{task_id}")))
    }

    /// T051：上下文占用 = 当前 Attempt 日志末尾最近一次 assistant/message 的 prompt 用量
    /// （输入 + 缓存读写）；还没有请求就是 0。
    fn context_used_of(events: &[session::Event]) -> u64 {
        for event in events.iter().rev() {
            if let session::EventKind::AssistantMessage {
                usage: Some(usage), ..
            } = &event.kind
            {
                return usage.input_tokens.unwrap_or(0)
                    + usage.cache_read_input_tokens.unwrap_or(0)
                    + usage.cache_creation_input_tokens.unwrap_or(0);
            }
        }
        0
    }

    /// 监控 WS 打开面：给每个在跑任务一份补齐 + live 订阅；任务事实变化另走 changed。
    /// 掉帧由连接层按任务发 task-changed 让页面重读。
    pub async fn monitor_live(&self, last_seq: u64) -> Result<MonitorLive, ServiceError> {
        let running: Vec<(String, u64, Arc<ServiceHost>)> = self
            .running
            .lock()
            .expect("thinagent 表锁被毒化")
            .values()
            .filter(|entry| !entry.retained)
            .map(|entry| {
                (
                    entry.task_id.as_str().to_owned(),
                    entry.attempt_id.0,
                    Arc::clone(&entry.host),
                )
            })
            .collect();
        let mut tasks = Vec::with_capacity(running.len());
        for (task_id, attempt_id, host) in running {
            let events = host.subscribe_events();
            let follow = host.follow(FollowRequest { last_seq }).await?;
            tasks.push(MonitorLiveTask {
                task_id,
                attempt_id,
                follow,
                events,
            });
        }
        Ok(MonitorLive {
            tasks,
            changed: self.changed.subscribe(),
        })
    }

    /// 监控消耗快照：按用户时区口径现算全部任务（不缓存，先不建账本）。
    pub(crate) fn monitor_snapshot(
        &self,
        request: crate::protocol::MonitorSnapshotRequest,
        kind_events: &[session::Event],
    ) -> Result<crate::protocol::MonitorSnapshotResponse, ServiceError> {
        crate::monitor_facts::snapshot(
            &self.store,
            &self.data_dir,
            request.range,
            &request.timezone,
            &self.running_tasks(),
            &self.running_request_times(),
            self.exec_seats.load(Ordering::SeqCst) as u64,
            kind_events,
        )
    }

    /// 监控诊断：一个任务的 Attempt/五块/todo/事件。
    pub(crate) fn monitor_task(
        &self,
        request: crate::protocol::MonitorTaskRequest,
    ) -> Result<crate::protocol::MonitorTaskResponse, ServiceError> {
        let running = {
            let task_id = request.task_id.clone();
            self.running_tasks().iter().any(|id| id == &task_id)
        };
        crate::monitor_facts::task(&self.store, &self.data_dir, request, running)
    }

    /// `/ws?task=` 的打开面：补齐 `lastSeq` 之后的日志；任务还在跑时带上 live 订阅。
    ///
    /// 先订阅、再 follow：补齐和 live 之间不留缝；同一段事件由调用方按 seq 去重。
    /// 任务不在「在跑」表里（终态收尾 / 未开跑）时只读补齐，不出 live 帧。
    pub async fn task_stream(
        &self,
        task_id: &str,
        last_seq: u64,
    ) -> Result<TaskStream, ServiceError> {
        let task = self.task(task_id)?;
        // T051：还没开跑的任务没有会话：只回空补齐、不出 live 帧。
        let empty = || TaskStream {
            session_id: String::new(),
            follow: FollowResponse {
                events: Vec::new(),
                last_seq,
            },
            live: None,
            changed: self.changed.subscribe(),
        };
        let Some(attempt) = task.latest_attempt() else {
            return Ok(empty());
        };
        let Some(conversation) = attempt.conversation.clone() else {
            return Ok(empty());
        };
        let attempt = attempt.id;
        let session_id = SessionId::parse(conversation.as_str()).map_err(|error| {
            ServiceError::TaskUnavailable(format!("任务对话的会话 id 不合法：{error}"))
        })?;
        if let Some(running) = self.running_for(&task.id, attempt) {
            let host = running.host;
            let events = host.subscribe_events();
            let agent_events = host.subscribe_agent_events();
            let follow = host.follow(FollowRequest { last_seq }).await?;
            return Ok(TaskStream {
                session_id: conversation.as_str().to_owned(),
                follow,
                live: Some(TaskLive {
                    host,
                    events,
                    agent_events,
                }),
                changed: self.changed.subscribe(),
            });
        }
        let snapshot = session::SessionStore::open_read_only(&self.data_dir, &session_id)
            .map_err(|error| ServiceError::TaskUnavailable(format!("任务对话打不开：{error}")))?;
        let events: Vec<ServiceEvent> = snapshot
            .events()
            .iter()
            .filter(|event| event.seq > last_seq)
            .map(service_event)
            .collect();
        let end = events.last().map(|event| event.seq).unwrap_or(last_seq);
        Ok(TaskStream {
            session_id: conversation.as_str().to_owned(),
            follow: FollowResponse {
                events,
                last_seq: end,
            },
            live: None,
            changed: self.changed.subscribe(),
        })
    }

    /// 启动扫描：效果循环开始之前收掉上次启动留下的 Attempt，并排 restart（T029）。
    pub(crate) fn scan_at_boot(&self, boot_id: &BootId) -> Result<BootScan, TaskError> {
        self.store.scan_at_boot(boot_id, Self::now())
    }

    /// 测试用：暂停整个效果泵（launch/relay 都不跑）。
    #[doc(hidden)]
    pub fn set_pump_paused(&self, paused: bool) {
        self.pump_paused.store(paused, Ordering::SeqCst);
        self.wake();
    }

    /// T057：给程序门排一次 launch（已授权过，不再出卡）。
    pub(crate) fn queue_launch(&self, task_id: &TaskId) -> Result<(), String> {
        self.store
            .enqueue_effect(
                EffectPayload::Launch {
                    task_id: task_id.clone(),
                },
                Self::now(),
            )
            .map_err(|error| error.to_string())?;
        self.wake();
        Ok(())
    }

    /// 当前全局暂停（D64）；做梦的自动时机据此让路。
    pub(crate) fn exec_paused(&self) -> bool {
        self.exec_paused.load(Ordering::SeqCst)
    }

    /// 全局暂停的闸门：RootHost 先写设置文件，再改这里的内存值。
    ///
    /// 只挡 launch 准入；relay/notice 照送。排队的效果留在库里，开闸后按原顺序领取。
    /// 到点送 Kind 前，幂等建一条只读触发任务（不排 launch、不开 Attempt）。
    fn trigger_task_for_run(
        &self,
        appointment: &calendar::Appointment,
        run: &calendar::Run,
        conversation: &str,
    ) -> Result<Task, String> {
        let selection = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .clone()
            .ok_or_else(|| "还没有可用的模型选择，不能建触发任务".to_owned())?;
        let draft = NewTask {
            project_id: None,
            cwd_source: None,
            quote: None,
            quote_source: None,
            kind_note: None,
            title: appointment.title.clone(),
            requirement: appointment.condition.clone(),
            dod: "到点办完".to_owned(),
            cwd: self.options.cwd.clone(),
            conversation: ConversationId::new(conversation),
            model: ModelChoice {
                connection: selection.connection,
                model: selection.model,
                effort_wanted: selection.effort_wanted,
            },
            tools: Vec::new(),
        };
        let key = format!("appointment-run:{}", run.id);
        let (task, _created) = self
            .store
            .create_trigger_task_once(draft, &key, Self::now())
            .map_err(|error| error.to_string())?;
        let origin = AppointmentOrigin {
            appointment_id: appointment.id.clone(),
            run_id: run.id,
            role: AppointmentRole::Trigger,
        };
        self.store
            .set_origin(&task.id, origin)
            .map_err(|error| error.to_string())?;
        Ok(task)
    }

    /// 记住这次 wake 落在根会话的哪一轮；dispatched 只按这条记录认。
    fn remember_trigger_turn(
        &self,
        conversation: &str,
        turn: u64,
        appointment_id: &str,
        run_id: i64,
    ) {
        self.trigger_turns
            .lock()
            .expect("trigger turns 锁被毒化")
            .insert(
                (conversation.to_owned(), turn),
                AppointmentOrigin {
                    appointment_id: appointment_id.to_owned(),
                    run_id,
                    role: AppointmentRole::Trigger,
                },
            );
    }

    /// 当前根会话最新的轮号（会话日志事实；没有就给 None）。
    fn current_root_turn(&self, conversation: &str) -> Option<u64> {
        let session_id = SessionId::parse(conversation).ok()?;
        let snapshot = session::SessionStore::open_read_only(&self.data_dir, &session_id).ok()?;
        snapshot
            .events()
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                session::EventKind::TurnStart { turn } => Some(*turn),
                _ => None,
            })
    }

    /// 当前这一轮若由 appointment-wake 开，返回那次的约定/运行来源。
    fn dispatched_origin(&self, conversation: &str) -> Option<AppointmentOrigin> {
        let turn = self.current_root_turn(conversation)?;
        self.trigger_turns
            .lock()
            .expect("trigger turns 锁被毒化")
            .get(&(conversation.to_owned(), turn))
            .cloned()
            .map(|origin| AppointmentOrigin {
                role: AppointmentRole::Dispatched,
                ..origin
            })
    }

    /// 把新派出/建图的任务标成 dispatched（图的根和子任务都带）。
    fn mark_dispatched(&self, conversation: &str, task: &Task) {
        let Some(origin) = self.dispatched_origin(conversation) else {
            return;
        };
        best_effort(
            "派出的任务没写来源",
            self.store.set_origin(&task.id, origin.clone()),
        );
        if let Ok(tasks) = self.store.all_tasks() {
            for child in tasks
                .into_iter()
                .filter(|child| child.parent.as_ref() == Some(&task.id) && child.origin.is_none())
            {
                best_effort(
                    "图子任务没写来源",
                    self.store.set_origin(&child.id, origin.clone()),
                );
            }
        }
    }

    /// 触发任务不能被当成普通任务接着做/改/停/交回。
    fn trigger_task_rejection(task: &Task) -> Option<String> {
        (task.origin.as_ref().map(|origin| origin.role) == Some(AppointmentRole::Trigger))
            .then(|| "这是约定到点的任务，不能接着做".to_owned())
    }

    /// 根会话 turn/end 钩子：这一轮是约定到点开的，就给触发任务写 round_end。
    pub(crate) fn calendar_on_turn_end(&self, conversation: &str, turn: u64, at: i64) {
        let Some(origin) = self
            .trigger_turns
            .lock()
            .expect("trigger turns 锁被毒化")
            .get(&(conversation.to_owned(), turn))
            .cloned()
        else {
            return;
        };
        let Ok(tasks) = self.store.all_tasks() else {
            return;
        };
        for task in tasks {
            let Some(task_origin) = &task.origin else {
                continue;
            };
            if task_origin.appointment_id == origin.appointment_id
                && task_origin.run_id == origin.run_id
                && task_origin.role == AppointmentRole::Trigger
            {
                best_effort(
                    "触发任务没写回合结束",
                    self.store.set_round_end(&task.id, RoundEnd { turn, at }),
                );
            }
        }
    }

    /// 启动补结束轮——触发任务的 round_end 没写、但那一轮日志里已有 turn/end。
    pub(crate) fn calendar_close_finished_turns(&self, conversation: &str) {
        let Some(calendar) = self.calendar.as_ref() else {
            return;
        };
        let Ok(tasks) = self.store.all_tasks() else {
            return;
        };
        for task in tasks {
            if task.conversation.as_str() != conversation
                || task.round_end.is_some()
                || task.origin.as_ref().map(|origin| origin.role) != Some(AppointmentRole::Trigger)
            {
                continue;
            }
            let Some(origin) = &task.origin else {
                continue;
            };
            let Ok(runs) = calendar.runs(&origin.appointment_id) else {
                continue;
            };
            let Some(sent_turn) = runs
                .iter()
                .find(|run| run.id == origin.run_id)
                .and_then(|run| run.sent_turn)
                .and_then(|turn| u64::try_from(turn).ok())
            else {
                continue;
            };
            let Ok(session_id) = SessionId::parse(conversation) else {
                continue;
            };
            let Ok(snapshot) = session::SessionStore::open_read_only(&self.data_dir, &session_id)
            else {
                continue;
            };
            if let Some(end) = snapshot
                .events()
                .iter()
                .find_map(|event| match &event.kind {
                    session::EventKind::TurnEnd { turn, .. } if *turn == sent_turn => {
                        Some(event.time)
                    }
                    _ => None,
                })
            {
                best_effort(
                    "启动补写触发任务回合结束",
                    self.store.set_round_end(
                        &task.id,
                        RoundEnd {
                            turn: sent_turn,
                            at: i64::try_from(end).unwrap_or(i64::MAX),
                        },
                    ),
                );
            }
        }
    }

    pub(crate) fn set_exec_paused(&self, paused: bool) {
        self.exec_paused.store(paused, Ordering::SeqCst);
        if paused {
            self.global_pause_since.store(Self::now(), Ordering::SeqCst);
        }
        self.wake();
    }

    /// 席位上限的闸门：RootHost 先写设置文件，再改这里的内存值。
    ///
    /// 只改之后的准入：已经在跑的任务不掐断；改大后排着的 launch 立刻再试。
    pub(crate) fn set_exec_seats(&self, seats: u8) {
        self.exec_seats.store(usize::from(seats), Ordering::SeqCst);
        self.wake();
    }

    /// bash 开关：RootHost 先写设置文件，再改这里的内存值。
    ///
    /// 只在受理任务时读：已经派出去的任务冻结了自己的值，不跟着开关变。
    pub(crate) fn set_exec_bash(&self, bash: bool) {
        self.exec_bash.store(bash, Ordering::SeqCst);
    }

    /// 在飞请求的起点（task_id -> requestAt）；只给监控在飞异常用。
    fn running_request_times(&self) -> std::collections::HashMap<String, u64> {
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .values()
            .filter(|entry| !entry.retained)
            .filter_map(|entry| {
                entry
                    .host
                    .agent()
                    .in_flight_request_at()
                    .map(|at| (entry.task_id.as_str().to_owned(), at))
            })
            .collect()
    }

    /// 正在跑一轮的任务（按任务 id）。turn 结束的不算——席位在轮次结束时释放。
    fn running_tasks(&self) -> Vec<String> {
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .values()
            .filter(|entry| !entry.retained && entry.host.agent().is_running())
            .map(|entry| entry.task_id.as_str().to_owned())
            .collect()
    }

    /// 测试钩子：改要求走生产入口。
    #[doc(hidden)]
    pub async fn update_requirement_for_test(
        self: &Arc<Self>,
        task_id: &str,
        requirement: String,
        dod: Option<String>,
        constraints: Option<Vec<String>>,
    ) -> Result<String, String> {
        self.update_requirement(task_id, requirement, dod, constraints, None, None, None)
            .await
    }

    /// 测试钩子：改写入范围走生产入口。
    #[doc(hidden)]
    pub async fn update_scope_for_test(
        self: &Arc<Self>,
        task_id: &str,
        write_scope: String,
    ) -> Result<String, String> {
        self.update_scope(task_id, write_scope)
            .await
            .map_err(|error| error.message)
    }

    /// 测试钩子：叫醒效果泵（直接排队 delivery 时用）。
    #[doc(hidden)]
    pub fn wake_for_test(&self) {
        self.wake();
    }

    /// 测试钩子：效果泵醒来的次数。
    #[doc(hidden)]
    pub fn pump_wake_count(&self) -> usize {
        self.pump_wakes.load(Ordering::SeqCst)
    }

    /// 测试钩子：走生产的 set_exec_paused（全局暂停只等通知）。
    #[doc(hidden)]
    pub fn set_exec_paused_for_test(&self, paused: bool) {
        self.set_exec_paused(paused);
    }

    /// 测试钩子：当前占用席位数（在跑 + 起跑预约）。
    #[doc(hidden)]
    pub fn used_seats(&self) -> usize {
        let running = self.running_tasks();
        self.slots.lock().expect("席位表锁被毒化").used(&running)
    }

    /// 现在还能不能再起一个 launch：在跑 + 起跑预约（person 可多占 1 个）没占满。
    fn has_launch_room(&self) -> bool {
        let running = self.running_tasks();
        let used = self.slots.lock().expect("席位表锁被毒化").used(&running);
        used < self.exec_seats.load(Ordering::SeqCst)
    }

    /// 占一个起跑席位；同键重试幂等。占不到返回 false，调用方按 Later 处理，不计失败。
    fn reserve_launch(&self, task_id: &str) -> bool {
        let running = self.running_tasks();
        let max = self.exec_seats.load(Ordering::SeqCst);
        self.slots
            .lock()
            .expect("席位表锁被毒化")
            .reserve(task_id, SlotKind::Launch, &running, max)
            .is_ok()
    }

    /// 释放一个任务的起跑席位，并叫醒效果泵去领下一条排队的效果。
    fn release_seat(&self, task_id: &str) {
        self.slots.lock().expect("席位表锁被毒化").free(task_id);
        self.wake();
    }

    /// 测试用：下一次 relay 在 Kind 收到之前「崩掉」（留下效果行，泵退出）。
    #[doc(hidden)]
    pub fn crash_before_next_relay(&self) {
        self.crash_before_relay.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// 测试用：下一次日历唤醒在送进 Kind 前失败一次（留效果行等退避重试）。
    #[doc(hidden)]
    pub fn crash_before_next_wake(&self) {
        self.crash_before_wake.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// 测试用：模拟进程退出——丢掉还开着的 ThinAgent（放开会话写锁），效果泵不再领取。
    ///
    /// 生产里进程退出由操作系统回收；测试要复现「重启后会话写者已经放开」。
    pub fn crash(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.running.lock().expect("thinagent 表锁被毒化").clear();
        self.wake();
    }

    /// 测试钩子：把等后台的上限调短；生产恒为 30 分钟。
    #[doc(hidden)]
    pub fn set_background_limit_ms(&self, millis: i64) {
        self.background_limit_ms
            .store(millis.max(1), Ordering::SeqCst);
    }

    /// T055：工作快照读任务库（只读；同 crate 用）。
    pub(crate) fn tasks_store(&self) -> &Store {
        &self.store
    }

    fn wake(&self) {
        self.notify.notify_one();
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or_default()
    }

    /// 下一次该醒来的时刻：日历下次到点和效果最近到期的较早者；暂停时没有到点。
    fn next_wake_at(&self) -> Option<i64> {
        if self.exec_paused.load(Ordering::SeqCst) {
            return None;
        }
        let calendar_due = self
            .calendar
            .as_ref()
            .and_then(|store| store.next_wake_at().ok().flatten());
        let effect_due = self.store.next_effect_due().ok().flatten();
        match (calendar_due, effect_due) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(at), None) | (None, Some(at)) => Some(at),
            (None, None) => None,
        }
    }

    /// 开效果泵；一个泵顺序处理，relay 的 ordered/独占就是这一条循环。
    pub(crate) fn start_pump(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let runtime = Arc::clone(self);
        tokio::spawn(async move { runtime.pump().await })
    }

    async fn pump(self: Arc<Self>) {
        loop {
            if self.shutting_down.load(Ordering::SeqCst) {
                return;
            }
            if !self.pump_paused.load(Ordering::SeqCst) {
                // 日历计时器与效果泵同一条循环：醒来先补到期的约定，再处理效果。
                self.calendar_tick(Self::now()).await;
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                let mut worked = false;
                // 取消停效果优先于 relay：取消后立即掐断在跑的宿主，不等 Kind 投递。
                match self
                    .store
                    .claim_effect_ordered(EffectKind::Cancel, Self::now())
                {
                    Ok(Some(claim)) => {
                        worked = true;
                        self.deliver_cancel(claim).await;
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("任务取消领取失败：{error}"),
                }
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                match self
                    .store
                    .claim_effect_ordered(EffectKind::Relay, Self::now())
                {
                    Ok(Some(claim)) => {
                        worked = true;
                        self.deliver_relay(claim).await;
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("任务 relay 领取失败：{error}"),
                }
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                // restart 汇报要叫模型，受全局暂停管：暂停时不领，开闸后按 seq 再领。
                if !self.exec_paused.load(Ordering::SeqCst) {
                    match self
                        .store
                        .claim_effect_ordered(EffectKind::Restart, Self::now())
                    {
                        Ok(Some(claim)) => {
                            worked = true;
                            self.deliver_restart(claim).await;
                        }
                        Ok(None) => {}
                        Err(error) => eprintln!("任务 restart 领取失败：{error}"),
                    }
                }
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                // 抄送 Kind 不受全局暂停影响：notice 照送，不唤醒。
                match self
                    .store
                    .claim_effect_ordered(EffectKind::Notice, Self::now())
                {
                    Ok(Some(claim)) => {
                        worked = true;
                        self.deliver_notice(claim).await;
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("任务 notice 领取失败：{error}"),
                }
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                // delivery 受全局暂停管：暂停时上限 0，效果留在库里，开闸后按 seq 再领。
                if !self.exec_paused.load(Ordering::SeqCst) {
                    match self
                        .store
                        .claim_effect_ordered(EffectKind::Delivery, Self::now())
                    {
                        Ok(Some(claim)) => {
                            worked = true;
                            self.deliver_delivery(claim).await;
                        }
                        Ok(None) => {}
                        Err(error) => eprintln!("任务 delivery 领取失败：{error}"),
                    }
                }
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                // 日历到点唤醒：全局暂停时先不送，开闸后再领；根会话不在就 Later，退避重试。
                if !self.exec_paused.load(Ordering::SeqCst) {
                    match self
                        .store
                        .claim_effect_ordered(EffectKind::AppointmentWake, Self::now())
                    {
                        Ok(Some(claim)) => {
                            worked = true;
                            self.deliver_appointment_wake(claim).await;
                        }
                        Ok(None) => {}
                        Err(error) => eprintln!("日历唤醒领取失败：{error}"),
                    }
                }
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                // 等后台的 30 分钟到点；到点杀命令、投说明、再合成等待。
                match self
                    .store
                    .claim_effect(EffectKind::BackgroundLimit, Self::now())
                {
                    Ok(Some(claim)) => {
                        worked = true;
                        self.deliver_background_limit(claim).await;
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("背景等待到点领取失败：{error}"),
                }
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                // 全局暂停只关 launch 这一道闸门：relay 已经送完了，队里的 launch 不领。
                // 席位满时也不领：效果原样留在库里（Later，不算失败），释放后按 seq 再领。
                if !self.exec_paused.load(Ordering::SeqCst) && self.has_launch_room() {
                    match self.store.claim_effect(EffectKind::Launch, Self::now()) {
                        Ok(Some(claim)) => {
                            let task_id = claim.key.clone();
                            if self.reserve_launch(&task_id) {
                                worked = true;
                                // 开了会话就留到轮次结束/终态报告，补上「已开好、轮还没开始」
                                // 的窗口；没开成（缺项、任务没了、派发失败）当场释放预约。
                                if !self.launch(claim).await {
                                    self.release_seat(&task_id);
                                }
                            } else {
                                // 检查之后、预约之前席位被改小：按 Later 处理，不算失败。
                                self.later(&claim);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => eprintln!("任务 launch 领取失败：{error}"),
                    }
                }
                if worked {
                    continue;
                }
            }
            // 只等最近一次到点，没有到点就只等通知；暂停只等通知。
            let now = Self::now();
            match self.next_wake_at() {
                Some(at) => {
                    let wait =
                        Duration::from_millis(u64::try_from((at - now).max(0)).unwrap_or(u64::MAX));
                    tokio::select! {
                        _ = self.notify.notified() => {}
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                None => {
                    tokio::select! {
                        _ = self.notify.notified() => {}
                        _ = std::future::pending::<()>() => {}
                    }
                }
            }
            self.pump_wakes.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// 把一条 restart Steer 进 Kind 根会话：汇报这次启动打断、还没接续的任务，出接续卡。
    ///
    /// 根会话不可用（模型/凭证没接上）时 Later：效果留在库里，根会话好了再送。
    async fn deliver_restart(self: &Arc<Self>, claim: EffectClaim) {
        let EffectPayload::Restart { boot_id, task_ids } = &claim.payload else {
            self.finish_or_fail(&claim, Err("restart 载荷不是 restart".to_owned()));
            return;
        };
        let tasks = match self.interrupted_facts(task_ids) {
            Ok(tasks) => tasks,
            Err(error) => {
                self.finish_or_fail(&claim, Err(error));
                return;
            }
        };
        // 送之前已经被接续的任务不再报；没有要报的就收掉这条效果。
        if tasks.is_empty() {
            self.finish_or_fail(&claim, Ok(()));
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            self.later(&claim);
            return;
        };
        let guard = inner.lock().await;
        let Some(host) = guard.as_ref() else {
            drop(guard);
            self.later(&claim);
            return;
        };
        let body = json!({ "kind": "tasks_interrupted", "tasks": tasks });
        let sent = host
            .send_message(
                &format!("restart:{boot_id}"),
                UserMessage::text(body.to_string()),
                DeliveryMode::Steer,
            )
            .await;
        drop(guard);
        match sent {
            Ok(_) => best_effort(
                "restart 完成没写进任务库",
                self.store.complete_effect(&claim),
            ),
            Err(error) => best_effort(
                "restart 失败没写进任务库",
                self.store
                    .fail_effect(&claim, &error.to_string(), Self::now()),
            ),
        }
    }

    /// 这次启动扫描打断、此刻还「被打断了」的任务事实，按扫描顺序；只给 Kind 事实，不给判断。
    ///
    /// 只认载荷里的任务：上次启动问过、用户暂不的任务不再上卡。旧库里没带任务 id 的
    /// restart 载荷退回「全部还被打断的」——不能因为缺字段把那次汇报吞掉。
    fn interrupted_facts(&self, task_ids: &[TaskId]) -> Result<Vec<Value>, String> {
        let ids = if task_ids.is_empty() {
            self.store.task_ids().map_err(|error| error.to_string())?
        } else {
            task_ids.to_vec()
        };
        let mut facts = Vec::new();
        for task_id in ids {
            let Some(task) = self
                .store
                .task(&task_id)
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            if !task.is_interrupted() {
                continue;
            }
            let mut entry = json!({
                "id": task.id.as_str(),
                "title": task.title,
                "attempts": task
                    .runs
                    .iter()
                    .map(|run| run.attempts.len() as u64)
                    .sum::<u64>(),
            });
            if let Some(report) = task.latest_report() {
                let summary: String = report.summary.chars().take(RESTART_REPORT_CHARS).collect();
                entry["last_report"] = json!(summary);
            }
            facts.push(entry);
        }
        Ok(facts)
    }

    /// 一条任务输入发给 ThinAgent 的消息：文字、粘贴段、附件，照 Kind 对话的拼法。
    ///
    /// 附件路径在投递时从附件仓库解析（和 Kind 对话一样是只读副本）；空输入在记库阶段
    /// 就拒了，这里不会拼出空消息。
    fn say_message(&self, input: &Input) -> Result<UserMessage, String> {
        let mut blocks = Vec::new();
        if !input.text.is_empty() {
            blocks.push(model::ContentBlock::text(input.text.clone()));
        }
        for paste in &input.pastes {
            if !paste.is_empty() {
                blocks.push(model::ContentBlock::paste(paste.clone()));
            }
        }
        for attachment in &input.attachments {
            let stored = tools::attachments::read(&self.data_dir, &attachment.id)
                .map_err(|error| error.to_string())?;
            blocks.push(model::ContentBlock::File {
                attachment_id: attachment.id.clone(),
                name: attachment.name.clone(),
                bytes: attachment.bytes,
                path: Some(stored.path.to_string_lossy().into_owned()),
            });
        }
        Ok(UserMessage::new(blocks))
    }

    /// 停/结束之后有人说话：按 mode 占席位，在原会话里接着开一次 Attempt，把输入送进去。
    ///
    /// 返回 Some(opened)：会话已开好、登记在跑表里，调用方负责 Steer 输入并收效果行。
    /// 返回 None：已经推迟/失败/完成，调用方直接返回。
    async fn resume_for_delivery(
        self: &Arc<Self>,
        claim: &EffectClaim,
        task: &Task,
        attempt: &Attempt,
        mode: ResumeMode,
    ) -> Option<OpenedResume> {
        if task.cancelled_message().is_some() {
            self.finish_or_fail(claim, Ok(()));
            return None;
        }
        let (trigger, slot) = match mode {
            ResumeMode::Say => (AttemptTrigger::UserMessage, SlotKind::Person),
            ResumeMode::Interrupted => (AttemptTrigger::Resume, SlotKind::Wake),
        };
        match self
            .open_resume(task, attempt, trigger, slot, Vec::new())
            .await
        {
            Ok(opened) => Some(opened),
            Err(ResumeBlock::Later | ResumeBlock::AlreadyStarted) => {
                self.later(claim);
                None
            }
            Err(ResumeBlock::PackageMissing) => {
                self.finish_or_fail(claim, Ok(()));
                None
            }
            Err(ResumeBlock::NoSession(reason)) => {
                self.finish_or_fail(claim, Err(reason));
                None
            }
        }
    }

    /// 确认卡「接续」：在原会话开一次新 Attempt（trigger resume），第一条输入是恢复说明。
    ///
    /// 返回新 Attempt 的编号；接不上时返回给用户看的原因。
    pub(crate) async fn resume_task(
        self: &Arc<Self>,
        task_id: &TaskId,
    ) -> Result<AttemptId, String> {
        let Some(task) = self
            .store
            .task(task_id)
            .map_err(|error| error.to_string())?
        else {
            return Err(format!("找不到这个任务：{task_id}"));
        };
        if let Some(reason) = task.cancelled_message() {
            return Err(reason.to_owned());
        }
        if task.is_program_gate() {
            return Err(
                "程序门没有会话可接续；要重跑用 gate_check_again，要换命令回 Kind 对话说"
                    .to_owned(),
            );
        }
        let Some(attempt) = task.latest_attempt().cloned() else {
            return Err("这个任务还没开跑，没有可接的会话".to_owned());
        };
        if !task.is_interrupted() {
            return Err(if task.was_interrupted() {
                "已经接着做了".to_owned()
            } else {
                "这个任务现在不是被打断了".to_owned()
            });
        }
        let opened = match self
            .open_resume(
                &task,
                &attempt,
                AttemptTrigger::Resume,
                SlotKind::Wake,
                Vec::new(),
            )
            .await
        {
            Ok(opened) => opened,
            Err(ResumeBlock::Later) => {
                return Err("现在接不上这个任务（没有空位或会话还在收尾），稍后再点一次".to_owned());
            }
            Err(ResumeBlock::AlreadyStarted) => return Err("已经接着做了".to_owned()),
            Err(ResumeBlock::PackageMissing) => {
                return Err("任务包缺了东西，接不上；看任务里的缺项".to_owned());
            }
            Err(ResumeBlock::NoSession(reason)) => return Err(reason),
        };
        // T052：接续前先把最新的上游结果交进会话（图子任务才有一段）。
        if let Err(error) = self
            .send_upstream_note(&opened.host, &task, opened.attempt_id)
            .await
        {
            eprintln!("上游结果没送进任务会话：{error}");
        }
        let request_id = format!("resume:{task_id}:{}", opened.attempt_id);
        let sent = opened
            .host
            .send_message(
                &request_id,
                UserMessage::text(self.resume_note(&task)),
                DeliveryMode::Steer,
            )
            .await;
        if let Err(error) = sent {
            // 恢复说明没送出去：别留一个没人做的空 Attempt。
            opened.host.shutdown().await;
            self.forget_running(opened.host.agent().id().as_str());
            best_effort(
                "接续说明没送出去的 Attempt 没收尾",
                self.store.end_attempt(
                    task_id,
                    opened.attempt_id,
                    Self::now(),
                    AttemptEnd::NotOpened,
                ),
            );
            self.release_seat(task_id.as_str());
            return Err(format!("恢复说明没送进任务会话：{error}"));
        }
        self.wake();
        Ok(opened.attempt_id)
    }

    /// 重开同一会话前，如果这个会话的宿主是保留状态（报告后后台还在跑），
    /// 先停命令、关宿主（shutdown 显式放掉会话写锁）、移除，再 notify/open。
    async fn stop_retained_for_reopen(&self, task_id: &TaskId, conversation: &str) {
        let host = {
            let running = self.running.lock().expect("thinagent 表锁被毒化");
            running.get(conversation).and_then(|entry| {
                if entry.retained {
                    Some(Arc::clone(&entry.host))
                } else {
                    None
                }
            })
        };
        let Some(host) = host else {
            return;
        };
        host.agent()
            .stop_background_jobs("重开任务会话前停掉保留的后台命令")
            .await;
        host.shutdown().await;
        drop(host);
        let removed = self
            .running
            .lock()
            .expect("thinagent 表锁被毒化")
            .remove(conversation);
        drop(removed);
        self.session_relinquished.notify_waiters();
        self.notify_changed(task_id);
    }

    /// 在原会话里接一次 Attempt（T028 的说话接续和确认卡接续共用）。
    ///
    /// 只做「占席位、落 Attempt、开回原会话、登记在跑表」；第一条消息由调用方送。
    /// 先占席位再落 Attempt：席位不够按 Later 处理，不能留下没有会话的空 Attempt。
    async fn open_resume(
        self: &Arc<Self>,
        task: &Task,
        attempt: &Attempt,
        trigger: AttemptTrigger,
        slot: SlotKind,
        handoffs: Vec<Handoff>,
    ) -> Result<OpenedResume, ResumeBlock> {
        // 崩溃/重启留下的、还没收尾的 Attempt 不在这里抢（留给恢复链）。
        if attempt.ended_at.is_none() {
            return Err(ResumeBlock::Later);
        }
        // 没开成会话的 Attempt 没有历史可续：说明了再失败，别一直压着。
        if attempt.end == Some(AttemptEnd::NotOpened) {
            return Err(ResumeBlock::NoSession(
                "上一次 Attempt 没开成会话，接不上；任务要重新派发".to_owned(),
            ));
        }
        let Some(conversation) = attempt.conversation.clone() else {
            return Err(ResumeBlock::NoSession(
                "这个任务没有可接着做的会话".to_owned(),
            ));
        };
        // 会话 id 先解析：续接入口要的是 SessionId，不是不透明字符串。
        let Ok(resume) = SessionId::parse(conversation.as_str()) else {
            return Err(ResumeBlock::NoSession(format!(
                "原会话 id 不合法：{conversation}"
            )));
        };
        // 同一会话的 5 条重开路径都在这里汇合：先停掉保留宿主里的后台命令、
        // 关宿主，再走现有的锁通知，避免报「会话写锁没让出来」。
        self.stop_retained_for_reopen(&task.id, conversation.as_str())
            .await;
        // 终态报告在工具调用里落库，那一轮可能还在写最后一段回答：会话写者没让出来
        // 之前不落新 Attempt（T028.5 e2e：报完立刻说「接着做」会撞上收尾）。
        // 效果留到下一次泵再领，攒够 2s 退避也不丢；不能在这里记一份「没开成」的失败报告。
        if let Err(session::SessionError::AlreadyOwned { .. }) =
            session::SessionStore::open(&self.data_dir, &resume)
        {
            return Err(ResumeBlock::Later);
        }
        let key = task.id.as_str().to_owned();
        let running = self.running_tasks();
        let max = self.exec_seats.load(Ordering::SeqCst);
        if self
            .slots
            .lock()
            .expect("席位表锁被毒化")
            .reserve(&key, slot, &running, max)
            .is_err()
        {
            // 席位满（说话接续含 person 多占的那 1 个）：效果留在库里，腾出来再领。
            return Err(ResumeBlock::Later);
        }
        let attempt_id =
            match self
                .store
                .begin_followup_with_handoffs(&task.id, trigger, handoffs, Self::now())
            {
                Ok(Some(attempt_id)) => attempt_id,
                Ok(None) => {
                    self.release_seat(&key);
                    return Err(ResumeBlock::AlreadyStarted);
                }
                Err(error) => {
                    self.release_seat(&key);
                    return Err(ResumeBlock::NoSession(format!(
                        "接着做没写进任务库：{error}"
                    )));
                }
            };
        let prepared = match self.prepare(task) {
            Ok(prepared) => prepared,
            Err(missing) => {
                self.record_missing(task, missing);
                best_effort(
                    "接不上的 Attempt 没收尾",
                    self.store.end_attempt(
                        &task.id,
                        attempt_id,
                        Self::now(),
                        AttemptEnd::NotOpened,
                    ),
                );
                self.release_seat(&key);
                return Err(ResumeBlock::PackageMissing);
            }
        };
        if self.shutting_down.load(Ordering::SeqCst) {
            best_effort(
                "关机时没开的 Attempt 没收尾",
                self.store
                    .end_attempt(&task.id, attempt_id, Self::now(), AttemptEnd::NotOpened),
            );
            self.release_seat(&key);
            return Err(ResumeBlock::Later);
        }
        // 上一轮可能还在收尾。先挂上「宿主移出在跑表」的通知，再查锁；
        // 等到通知后只 open 一次，还占着就报错，不循环重试。
        let relinquished = self.session_relinquished.notified();
        tokio::pin!(relinquished);
        relinquished.as_mut().enable();
        if matches!(
            session::SessionStore::open(&self.data_dir, &resume),
            Err(session::SessionError::AlreadyOwned { .. })
        ) {
            if tokio::time::timeout(Duration::from_secs(5), relinquished)
                .await
                .is_err()
            {
                return Err(ResumeBlock::NoSession(
                    "上一轮还在收尾，会话写锁没让出来".to_owned(),
                ));
            }
            if matches!(
                session::SessionStore::open(&self.data_dir, &resume),
                Err(session::SessionError::AlreadyOwned { .. })
            ) {
                return Err(ResumeBlock::NoSession(
                    "上一轮还在收尾，会话写锁没让出来".to_owned(),
                ));
            }
        }
        let host = match self
            .open_thin(task, attempt_id, prepared, Some(resume))
            .await
        {
            Ok(host) => Arc::new(host),
            Err(error) => {
                best_effort(
                    "接着做的会话没开成的失败报告没写进去",
                    self.store.record_report(
                        &task.id,
                        attempt_id,
                        NewReport {
                            state: ReportState::Failed,
                            origin: ReportOrigin::Executor,
                            summary: format!("接着做时任务会话没开成：{error}"),
                            question: None,
                        },
                        Self::now(),
                    ),
                );
                best_effort(
                    "接着做没开成的 Attempt 没收尾",
                    self.store.end_attempt(
                        &task.id,
                        attempt_id,
                        Self::now(),
                        AttemptEnd::NotOpened,
                    ),
                );
                self.release_seat(&key);
                return Err(ResumeBlock::NoSession(format!(
                    "接着做时会话没开成：{error}"
                )));
            }
        };
        // 每次 await 之后重查：没在关机、任务还在、这次仍是最新的 Attempt。
        let current = !self.shutting_down.load(Ordering::SeqCst)
            && self
                .store
                .task(&task.id)
                .ok()
                .flatten()
                .and_then(|stored| stored.latest_attempt().map(|attempt| attempt.id))
                == Some(attempt_id);
        if !current {
            host.shutdown().await;
            best_effort(
                "重查后失效的 Attempt 没收尾",
                self.store
                    .end_attempt(&task.id, attempt_id, Self::now(), AttemptEnd::NotOpened),
            );
            self.release_seat(&key);
            return Err(ResumeBlock::Later);
        }
        let conversation = host.agent().id().clone();
        if let Err(error) = self.store.attach_session(
            &task.id,
            attempt_id,
            ConversationId::new(conversation.as_str()),
        ) {
            host.shutdown().await;
            best_effort(
                "会话没接上的 Attempt 没收尾",
                self.store
                    .end_attempt(&task.id, attempt_id, Self::now(), AttemptEnd::NotOpened),
            );
            self.release_seat(&key);
            return Err(ResumeBlock::NoSession(format!("会话没接上：{error}")));
        }
        // 先订阅、登记、再交给调用方发：和 launch 一样，终态报告赶在第一条消息之前也不会漏。
        let events = host.agent().subscribe();
        self.running.lock().expect("thinagent 表锁被毒化").insert(
            conversation.as_str().to_owned(),
            RunningThin {
                host: Arc::clone(&host),
                task_id: task.id.clone(),
                attempt_id,
                retained: false,
            },
        );
        self.monitor(task.id.clone(), attempt_id, conversation, events);
        self.notify_changed(&task.id);
        Ok(OpenedResume { host, attempt_id })
    }

    /// 抄送 Kind：Inject 进发起任务的 Kind 会话，不唤醒；不在当前根会话时 Later。
    async fn deliver_notice(self: &Arc<Self>, claim: EffectClaim) {
        let EffectPayload::Notice { task_id, input_id } = &claim.payload else {
            self.finish_or_fail(&claim, Err("notice 载荷不是 notice".to_owned()));
            return;
        };
        let task = match self.store.task(task_id) {
            Ok(Some(task)) => task,
            Ok(None) => {
                self.finish_or_fail(&claim, Ok(()));
                return;
            }
            Err(error) => {
                self.finish_or_fail(&claim, Err(error.to_string()));
                return;
            }
        };
        let Some(input) = task
            .inputs
            .iter()
            .find(|input| input.id.as_str() == input_id.as_str())
        else {
            self.finish_or_fail(&claim, Ok(()));
            return;
        };
        let (text, chars) = relay_summary(&input.text);
        let mut body = json!({
            "kind": "task_cc",
            "task": {"id": task.id.as_str(), "title": task.title},
            "text": text,
        });
        if let Some(chars) = chars {
            body["chars"] = json!(chars);
        }
        let request_id = format!("task-cc:{}", input.id.as_str());
        let target = task.conversation.as_str();
        let Some(inner) = self.inner.upgrade() else {
            self.later(&claim);
            return;
        };
        let guard = inner.lock().await;
        let Some(host) = guard.as_ref() else {
            drop(guard);
            self.later(&claim);
            return;
        };
        if host.agent().id().as_str() != target {
            drop(guard);
            self.later(&claim);
            return;
        }
        let sent = host
            .send_message(
                &request_id,
                UserMessage::text(body.to_string()),
                DeliveryMode::Inject,
            )
            .await;
        drop(guard);
        match sent {
            Ok(_) => best_effort(
                "notice 完成没写进任务库",
                self.store.complete_effect(&claim),
            ),
            Err(error) => best_effort(
                "notice 失败没写进任务库",
                self.store
                    .fail_effect(&claim, &error.to_string(), Self::now()),
            ),
        }
    }

    fn later(&self, claim: &EffectClaim) {
        best_effort(
            "relay 推迟没写进任务库",
            self.store.defer_effect(claim, Self::now()),
        );
    }

    fn finish_or_fail(&self, claim: &EffectClaim, result: Result<(), String>) {
        match result {
            Ok(()) => best_effort("效果完成没写进任务库", self.store.complete_effect(claim)),
            Err(error) => best_effort(
                "效果失败没写进任务库",
                self.store.fail_effect(claim, &error, Self::now()),
            ),
        }
    }

    /// 执行一次 launch：重查、查任务包、开 ThinAgent、写回会话 id、发 assignment。
    ///
    /// 返回 true 表示开了会话并把 assignment 送进去了（席位留到轮次结束）；
    /// 其余情况返回 false，由调用方释放起跑预约。
    async fn launch(self: &Arc<Self>, claim: EffectClaim) -> bool {
        let EffectPayload::Launch { task_id } = &claim.payload else {
            self.finish_or_fail(&claim, Err("launch 载荷不是 launch".to_owned()));
            return false;
        };
        if self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        let mut task = match self.store.task(task_id) {
            Ok(Some(task)) => task,
            Ok(None) => {
                self.finish_or_fail(&claim, Ok(()));
                return false;
            }
            Err(error) => {
                self.finish_or_fail(&claim, Err(error.to_string()));
                return false;
            }
        };
        // T052：总任务不跑；图没确认、或上游还没全有 ready 结果的子任务不跑。
        // 重启扫描、开闸、崩溃重放都走这个入口，所以只挡这一处。
        if task.is_graph_root() {
            self.finish_or_fail(&claim, Ok(()));
            return false;
        }
        if let Some(parent) = task.parent.clone() {
            let arrangement = self
                .store
                .task(&parent)
                .ok()
                .flatten()
                .and_then(|root| root.arrangement);
            // T053：取消的图一项都不跑（确认过再取消不可能，这里只挡事实）。
            if arrangement
                .as_ref()
                .is_some_and(|arrangement| arrangement.cancelled.is_some())
            {
                self.finish_or_fail(&claim, Ok(()));
                return false;
            }
            let confirmed = arrangement
                .and_then(|arrangement| arrangement.confirmed)
                .is_some();
            let ready = match self.store.needs_ready(&task) {
                Ok(ready) => ready,
                Err(error) => {
                    self.finish_or_fail(&claim, Err(error.to_string()));
                    return false;
                }
            };
            if !confirmed || !ready {
                self.finish_or_fail(&claim, Ok(()));
                return false;
            }
        }
        // T053：门的重检是新 Attempt，允许同任务之前开过会话；别的任务重跑效果只把行收掉。
        if !task.is_gate()
            && task
                .latest_attempt()
                .is_some_and(|attempt| attempt.conversation.is_some())
        {
            self.finish_or_fail(&claim, Ok(()));
            return false;
        }
        // T053：门任务的准入——候选必须已经交接；同一候选不重复核；门还在跑就推迟。
        let mut gate_candidate = None;
        if task.is_gate() {
            let Some(need) = task.needs.first() else {
                self.finish_or_fail(&claim, Ok(()));
                return false;
            };
            let Some(handoff) = need.handoff.clone() else {
                self.finish_or_fail(&claim, Ok(()));
                return false;
            };
            if let Some(latest) = task.latest_attempt() {
                if latest.ended_at.is_none() {
                    self.later(&claim);
                    return false;
                }
                let rerun_after_undetermined = task.is_program_gate()
                    && (latest.end == Some(AttemptEnd::Interrupted)
                        || task.latest_report().is_none()
                        || task
                            .latest_report()
                            .and_then(|report| task.verdict_for_gate_report(&report.id))
                            .is_some_and(|verdict| {
                                verdict.conclusion == GateConclusion::Undetermined
                                    || verdict.commands_version != task.commands_version()
                            }));
                if latest
                    .handoffs
                    .first()
                    .is_some_and(|checked| checked.report_id == handoff.report_id)
                    && !rerun_after_undetermined
                {
                    self.finish_or_fail(&claim, Ok(()));
                    return false;
                }
            }
            gate_candidate = Some(GateCandidate {
                report_id: handoff.report_id.clone(),
                requirement_version: handoff.requirement_version,
            });
        }
        // 崩溃留下的、没有会话 id 的 Attempt 先收尾，再开新的。
        if let Err(error) = self.store.close_unopened_attempts(&task.id, Self::now()) {
            self.finish_or_fail(&claim, Err(error.to_string()));
            return false;
        }
        if let Ok(Some(reloaded)) = self.store.task(&task.id) {
            task = reloaded;
        }

        // T057：程序门不开 ThinAgent、不占席位；拿到候选后直接排一次异步检查。
        if task.is_program_gate() {
            let Some(candidate) = gate_candidate.clone() else {
                self.finish_or_fail(&claim, Ok(()));
                return false;
            };
            let handoffs: Vec<Handoff> = task
                .needs
                .iter()
                .filter_map(|need| need.handoff.clone())
                .collect();
            let attempt_id = match self.store.begin_attempt_with(
                &task.id,
                Self::now(),
                AttemptTrigger::GateRecheck { candidate },
                handoffs,
            ) {
                Ok(attempt_id) => attempt_id,
                Err(error) => {
                    self.finish_or_fail(&claim, Err(error.to_string()));
                    return false;
                }
            };
            let root = self
                .data_dir
                .join("gate-checks")
                .join(format!("{}-{}", task.id, attempt_id.0));
            let registry = Arc::new(TaskRegistry::detached(root));
            let running = Arc::new(RunningGate {
                attempt_id,
                registry: Arc::clone(&registry),
                job_id: Mutex::new(None),
                current_index: Mutex::new(None),
                stop_reason: Mutex::new(None),
                cancelled: AtomicBool::new(false),
            });
            self.gate_jobs
                .lock()
                .expect("程序门表锁被毒化")
                .insert(task.id.as_str().to_owned(), Arc::clone(&running));
            let runtime = Arc::clone(self);
            let gate_id = task.id.clone();
            tokio::spawn(async move {
                runtime
                    .run_program_gate(gate_id, attempt_id, registry, running)
                    .await;
            });
            self.finish_or_fail(&claim, Ok(()));
            return false;
        }
        let prepared = match self.prepare(&task) {
            Ok(prepared) => prepared,
            Err(missing) => {
                self.record_missing(&task, missing);
                self.finish_or_fail(&claim, Ok(()));
                return false;
            }
        };
        if self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        // 工作树只在第一次 Attempt 起跑前开一次；之后每个 Attempt 都用任务行里记下的
        // 那一份，不再 git worktree add。
        if task.worktree && task.worktree_state.is_none() {
            match crate::worktree::create_for(&task, &self.data_dir).await {
                Ok(worktree) => match self.store.attach_worktree(&task.id, worktree) {
                    Ok(updated) => task = updated,
                    Err(error) => {
                        self.finish_or_fail(&claim, Err(error.to_string()));
                        return false;
                    }
                },
                Err(crate::worktree::WorktreeIssue::NotRepository) => {
                    self.record_missing(&task, vec![missing_item(WORKTREE_NOT_A_REPO)]);
                    self.finish_or_fail(&claim, Ok(()));
                    return false;
                }
                Err(crate::worktree::WorktreeIssue::Failed(message)) => {
                    self.record_missing(&task, vec![missing_item(message)]);
                    self.finish_or_fail(&claim, Ok(()));
                    return false;
                }
            }
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        if let Err(error) = self.store.record_package_check(&task.id, Vec::new()) {
            self.finish_or_fail(&claim, Err(error.to_string()));
            return false;
        }
        // T053：记下开跑时实际拿到的上游报告；门重检带候选事实。
        let handoffs: Vec<Handoff> = task
            .needs
            .iter()
            .filter_map(|need| need.handoff.clone())
            .collect();
        let trigger = match gate_candidate {
            Some(candidate) => AttemptTrigger::GateRecheck { candidate },
            None => AttemptTrigger::Launch,
        };
        let attempt_id =
            match self
                .store
                .begin_attempt_with(&task.id, Self::now(), trigger, handoffs)
            {
                Ok(attempt_id) => attempt_id,
                Err(error) => {
                    self.finish_or_fail(&claim, Err(error.to_string()));
                    return false;
                }
            };
        let host = match self.open_thin(&task, attempt_id, prepared, None).await {
            Ok(host) => Arc::new(host),
            Err(error) => {
                best_effort(
                    "任务会话没开成的失败报告没写进去",
                    self.store.record_report(
                        &task.id,
                        attempt_id,
                        NewReport {
                            state: ReportState::Failed,
                            origin: ReportOrigin::Executor,
                            summary: format!("任务会话没开成：{error}"),
                            question: None,
                        },
                        Self::now(),
                    ),
                );
                best_effort(
                    "没开成的 Attempt 没收尾",
                    self.store.end_attempt(
                        &task.id,
                        attempt_id,
                        Self::now(),
                        AttemptEnd::NotOpened,
                    ),
                );
                self.finish_or_fail(&claim, Ok(()));
                return false;
            }
        };
        // 每次 await 之后重查：没在关机、任务还在、这次仍是最新的 Attempt。
        let current = !self.shutting_down.load(Ordering::SeqCst)
            && self
                .store
                .task(&task.id)
                .ok()
                .flatten()
                .and_then(|stored| stored.latest_attempt().map(|attempt| attempt.id))
                == Some(attempt_id);
        if !current {
            host.shutdown().await;
            best_effort(
                "重查后失效的 Attempt 没收尾",
                self.store
                    .end_attempt(&task.id, attempt_id, Self::now(), AttemptEnd::NotOpened),
            );
            return false;
        }
        let conversation = host.agent().id().clone();
        if let Err(error) = self.store.attach_session(
            &task.id,
            attempt_id,
            ConversationId::new(conversation.as_str()),
        ) {
            host.shutdown().await;
            self.finish_or_fail(&claim, Err(error.to_string()));
            return false;
        }
        // 先订阅、登记、再发：turn 结束的事实和「会话还开着」都在第一条消息之前就位，
        // 免得报得比登记快的终态报告留下一个永远不释放的会话。
        let events = host.agent().subscribe();
        self.running.lock().expect("thinagent 表锁被毒化").insert(
            conversation.as_str().to_owned(),
            RunningThin {
                host: Arc::clone(&host),
                task_id: task.id.clone(),
                attempt_id,
                retained: false,
            },
        );
        self.monitor(task.id.clone(), attempt_id, conversation.clone(), events);
        self.notify_changed(&task.id);
        let assignment = self.assignment_text(&task, false);
        let sent = host
            .send_message(
                &format!("assignment:{}", task.id),
                UserMessage::text(assignment),
                DeliveryMode::FollowUp,
            )
            .await;
        if let Err(error) = sent {
            self.forget_running(conversation.as_str());
            best_effort(
                "派发失败的失败报告没写进去",
                self.store.record_report(
                    &task.id,
                    attempt_id,
                    NewReport {
                        state: ReportState::Failed,
                        origin: ReportOrigin::Executor,
                        summary: format!("任务派发没送进 ThinAgent：{error}"),
                        question: None,
                    },
                    Self::now(),
                ),
            );
            best_effort(
                "派发失败的 Attempt 没收尾",
                self.store
                    .end_attempt(&task.id, attempt_id, Self::now(), AttemptEnd::Reported),
            );
            self.finish_or_fail(&claim, Ok(()));
            return false;
        }
        self.finish_or_fail(&claim, Ok(()));
        true
    }

    /// 任务包缺项先记事实、再按程序事实写一张 blocked 报告并排 relay；不开任务会话。
    fn record_missing(&self, task: &Task, missing: Vec<MissingItem>) {
        let summary = format!(
            "任务包缺：{}",
            missing
                .iter()
                .map(MissingItem::as_str)
                .collect::<Vec<_>>()
                .join("；")
        );
        if let Err(error) = self.store.record_package_check(&task.id, missing) {
            eprintln!("任务 {} 的任务包缺项没写进去：{error}", task.id);
            return;
        }
        let attempt = match self.store.begin_attempt(&task.id, Self::now()) {
            Ok(attempt) => attempt,
            Err(error) => {
                eprintln!("任务 {} 记缺项 Attempt 失败：{error}", task.id);
                return;
            }
        };
        best_effort(
            "任务包缺项报告没写进去",
            self.store.record_report(
                &task.id,
                attempt,
                NewReport {
                    state: ReportState::Blocked,
                    origin: ReportOrigin::Package,
                    summary,
                    question: None,
                },
                Self::now(),
            ),
        );
        best_effort(
            "任务包缺项的 Attempt 没收尾",
            self.store
                .end_attempt(&task.id, attempt, Self::now(), AttemptEnd::NotOpened),
        );
    }

    /// T052：任务包里「上游结果」一段——按库里上游最新的 ready 报告现查（不只看 needs 上的交接），
    /// launch 和接续 Attempt 共用同一段构造。
    fn upstream_results_for(&self, task: &Task) -> Vec<Value> {
        let mut results = Vec::new();
        for need in &task.needs {
            let Ok(Some(upstream)) = self.store.task(&need.from) else {
                continue;
            };
            let Some(report) = upstream
                .reports
                .iter()
                .rev()
                .find(|report| report.state == ReportState::Ready)
            else {
                continue;
            };
            results.push(json!({
                "task_id": need.from.as_str(),
                "title": upstream.title,
                "report_id": report.id.as_str(),
                "requirement_version": report.version,
                "summary": report.summary,
            }));
        }
        results
    }

    /// T052：接续 Attempt 打开以后，把「上游结果（最新）」随恢复说明送进会话。
    ///
    /// 下游以后的 Attempt 不重发 assignment，靠这条消息重新交接；同一个 Attempt 只送一次
    /// （request id 带 Attempt 编号，重复触发会被会话判重）。
    async fn send_upstream_note(
        &self,
        host: &ServiceHost,
        task: &Task,
        attempt_id: AttemptId,
    ) -> Result<(), String> {
        let results = self.upstream_results_for(task);
        if results.is_empty() {
            return Ok(());
        }
        let mut lines = vec!["上游结果（最新）：".to_owned()];
        for item in results {
            lines.push(format!(
                "- {}（要求第 {} 版）：{}",
                item["title"].as_str().unwrap_or_default(),
                item["requirement_version"],
                item["summary"].as_str().unwrap_or_default()
            ));
        }
        host.send_message(
            &format!("upstream:{}:{}", task.id, attempt_id.0),
            UserMessage::text(lines.join("\n")),
            DeliveryMode::Steer,
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    /// T056.2：接续说明（RESUME_NOTE）连同当前账本；空账本只发说明。
    fn resume_note(&self, task: &Task) -> String {
        let ledger = self.ledger_section(task);
        if ledger.is_empty() {
            RESUME_NOTE.to_owned()
        } else {
            format!(
                "{RESUME_NOTE}

{ledger}"
            )
        }
    }

    /// 起跑前的任务包检查：返回解析好的模型配置和凭证；缺项时返回缺的每一项。
    fn prepare(
        &self,
        task: &Task,
    ) -> Result<(model::DeepSeekConfig, Arc<dyn model::CredentialSource>), Vec<MissingItem>> {
        // 默认角色的 skill、规则和账本（写「无」）是编进本 crate 的常量：缺了是产品装配
        // 错误，不是任务事实，这里不拿常量自证。下面查的是任务本身和凭证的事实。
        let mut missing = Vec::new();
        if task.requirement.trim().is_empty() {
            missing.push(missing_item("任务要求"));
        }
        if task.dod.trim().is_empty() {
            missing.push(missing_item("DoD"));
        }
        // 开了工作树的任务看工作树里那个 cwd；第一次起跑前工作树还没开，看到的是原 cwd。
        if !task.effective_cwd().is_dir() {
            missing.push(missing_item(format!(
                "工作目录不存在：{}",
                task.effective_cwd().display()
            )));
        }
        if !task.writes.is_empty() {
            missing.push(missing_item("写集（只读任务必须为空）"));
        }
        if task.code_mode {
            missing.push(missing_item("CodeMode（只读任务必须为否）"));
        }
        // 工具名单跟着任务的写入范围和 bash 开关走：只读 11 件，写任务 13 件，
        // 开了 bash 的写任务 17 件。
        // T056.2 加账本两件之前冻结的旧名单仍要能接着做（T028.4 加 task_handback 同理），
        // 所以下面按「当前名单」和「历史名单」一起比。
        let (expected, legacy): (BTreeSet<&str>, Vec<BTreeSet<&str>>) =
            match (task.write_scope.is_some(), task.bash) {
                (true, true) => (
                    THIN_WRITE_BASH_TOOLS.into_iter().collect(),
                    vec![
                        THIN_WRITE_BASH_TOOLS
                            .into_iter()
                            .filter(|name| !name.starts_with("task_ledger_"))
                            .collect(),
                    ],
                ),
                (true, false) => (
                    THIN_WRITE_TOOLS.into_iter().collect(),
                    vec![
                        THIN_WRITE_TOOLS
                            .into_iter()
                            .filter(|name| !name.starts_with("task_ledger_"))
                            .collect(),
                    ],
                ),
                (false, _) => (
                    THIN_TOOLS.into_iter().collect(),
                    vec![
                        THIN_TOOLS
                            .into_iter()
                            .filter(|name| !name.starts_with("task_ledger_"))
                            .collect(),
                        THIN_TOOLS
                            .into_iter()
                            .filter(|name| {
                                !name.starts_with("task_ledger_") && *name != "task_handback"
                            })
                            .collect(),
                    ],
                ),
            };
        let actual: BTreeSet<&str> = task.tools.iter().map(String::as_str).collect();
        if actual != expected && !legacy.iter().any(|legacy| legacy == &actual) {
            missing.push(missing_item("工具名单（和写入范围不符）"));
        }
        if task.model.connection.trim().is_empty()
            || task.model.model.trim().is_empty()
            || task.model.effort_wanted.trim().is_empty()
        {
            missing.push(missing_item("模型选择"));
        }
        match crate::ledger::ledger_for(&self.store, task) {
            Ok(view) => {
                let chars = view.section().chars().count();
                if chars > 6000 {
                    missing.push(missing_item(format!(
                        "账本超出上限（{chars} 字，上限 6000）：任务太大，请拆任务"
                    )));
                }
            }
            Err(error) => missing.push(missing_item(format!("账本（读不出来：{error}）"))),
        }
        if !missing.is_empty() {
            return Err(missing);
        }

        let selection = session::ModelSelection {
            connection: task.model.connection.clone(),
            model: task.model.model.clone(),
            effort_wanted: task.model.effort_wanted.clone(),
        };
        let mut config = self.options.model.clone();
        config.model = selection.model.clone();
        config.effort = Effort::new(selection.effort_wanted.clone());
        let credentials = if let Some(factory) = &self.options.model_factory {
            match factory(Some(&selection)) {
                Ok((_connection, resolved, credentials)) => {
                    config = resolved;
                    credentials
                }
                Err(message) => {
                    return Err(vec![missing_item(format!("模型连接：{message}"))]);
                }
            }
        } else {
            self.options.credentials.clone()
        };
        let Some(credentials) = credentials else {
            return Err(vec![missing_item(format!("{} 的凭证", task.model.model))]);
        };
        if let Err(error) = credentials.admit(&task.model.model) {
            return Err(vec![missing_item(format!(
                "{} 的凭证（{}）",
                task.model.model, error
            ))]);
        }
        Ok((config, credentials))
    }

    /// 用冻结的模型选择、任务 cwd、冻结的工具名单和角色规则开一个 ThinAgent。
    ///
    /// 开了工作树的任务用工作树里对应的子目录；写类工具拿换算后的写入范围，
    /// 在 resolve 之后用它核目标。
    /// `resume` 是接着做的原会话；会话 id 不变，日志里的历史照常进这次请求。
    async fn open_thin(
        self: &Arc<Self>,
        task: &Task,
        attempt_id: AttemptId,
        (model_config, credentials): (model::DeepSeekConfig, Arc<dyn model::CredentialSource>),
        resume: Option<SessionId>,
    ) -> Result<ServiceHost, String> {
        let mut extra_tools = crate::root::web_extra_tools(&self.options);
        extra_tools.push(Arc::new(TaskReportTool::new(
            Arc::clone(self),
            task.id.clone(),
            attempt_id,
        )));
        extra_tools.push(Arc::new(TaskHandbackTool::new(
            Arc::clone(self),
            task.id.clone(),
            attempt_id,
        )));
        extra_tools.push(Arc::new(TaskLedgerNoteTool::new(
            Arc::clone(self),
            task.id.clone(),
            attempt_id,
        )));
        extra_tools.push(Arc::new(TaskLedgerHistoryTool::new(
            Arc::clone(self),
            task.id.clone(),
        )));
        let mut agent_config = self.options.config.clone();
        agent_config.tool_keys = crate::root::tool_key_source(&self.options);
        agent_config.write_scope = task.effective_write_scope();
        agent_config.identity_prompt = Some(
            "You are AIOS's executor: you investigate and implement the task Kind hands you."
                .to_owned(),
        );
        agent_config.project_instructions_path_only = false;
        if let Ok(memory) = crate::memory::MemoryRuntime::open(&self.data_dir) {
            let memory = std::sync::Arc::new(memory);
            agent_config.memory_context = Some(memory.thin_provider(task.project_id.clone()));
            agent_config.memory_delivery = Some(memory.delivery_hook());
        }
        let fixed_runtime = Arc::clone(self);
        let fixed_task = task.id.clone();
        agent_config.compaction_fixed_block = Some(std::sync::Arc::new(move || {
            fixed_runtime.compaction_task_block(&fixed_task, attempt_id)
        }));
        let agent = agent::Agent::open(AgentOpenOptions {
            data_dir: self.data_dir.clone(),
            cwd: task.effective_cwd().to_path_buf(),
            resume,
            model: model_config,
            config: agent_config,
            credentials: Some(credentials),
            append_hook_factory: self.options.append_hook_factory.clone(),
            extra_tools,
            tool_policy: ToolPolicy {
                only: Some(task.tools.clone()),
                // ThinAgent 不等命令、不盯后台输出；要等就转后台，结果会叫醒它。
                no_wait: true,
            },
            policy_prompt: Some(thin_policy(task.bash)),
        })
        .await
        .map_err(|error| error.to_string())?;
        Ok(ServiceHost::new(agent))
    }

    /// 一个 Attempt 的会话在本进程里还开着（task_status 要状态用）。
    fn running_for(&self, task_id: &TaskId, attempt_id: AttemptId) -> Option<RunningThin> {
        let running = self.running.lock().expect("thinagent 表锁被毒化");
        running
            .values()
            .find(|entry| entry.task_id == *task_id && entry.attempt_id == attempt_id)
            .map(|entry| RunningThin {
                host: Arc::clone(&entry.host),
                task_id: entry.task_id.clone(),
                attempt_id: entry.attempt_id,
                retained: entry.retained,
            })
    }

    /// 宿主移出在跑表；叫醒等会话锁的续接。
    fn forget_running(&self, conversation: &str) {
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .remove(conversation);
        self.session_relinquished.notify_waiters();
    }

    /// 报告/交回后若还有后台命令，把宿主标 retained（不移除、不占席位）。
    fn retain_after_report(&self, conversation: &str) -> bool {
        let mut running = self.running.lock().expect("thinagent 表锁被毒化");
        let Some(entry) = running.get_mut(conversation) else {
            return false;
        };
        if entry.host.agent().background_turn().is_none() {
            return false;
        }
        entry.host.agent().set_background_only();
        entry.retained = true;
        true
    }

    /// 某个任务当前保留着的后台命令轮号（停止入口用）。
    fn retained_background(&self, task_id: &TaskId) -> Option<(String, u64)> {
        let running = self.running.lock().expect("thinagent 表锁被毒化");
        running.values().find_map(|entry| {
            if entry.task_id != *task_id || !entry.retained {
                return None;
            }
            entry
                .host
                .agent()
                .background_turn()
                .map(|turn| (entry.host.agent().id().as_str().to_owned(), turn))
        })
    }

    /// 保留的后台命令还在跑（TaskReadHeader.backgroundRunning）。
    fn background_running(&self, task_id: &TaskId, attempt_id: AttemptId) -> bool {
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .values()
            .any(|entry| {
                entry.task_id == *task_id
                    && entry.attempt_id == attempt_id
                    && entry.retained
                    && entry.host.agent().background_turn().is_some()
            })
    }

    /// 保留宿主的最后一个后台命令结算了：移除宿主、放会话锁、通知页面。
    fn finish_retained_if_idle(&self, conversation: &str) {
        let task_id = {
            let running = self.running.lock().expect("thinagent 表锁被毒化");
            let Some(entry) = running.get(conversation) else {
                return;
            };
            if !entry.retained || entry.host.agent().background_turn().is_some() {
                return;
            }
            entry.task_id.clone()
        };
        self.forget_running(conversation);
        self.notify_changed(&task_id);
    }

    /// 宿主移除后通知页面重读页头。
    fn notify_changed(&self, task_id: &TaskId) {
        drop(self.changed.send(task_id.as_str().to_owned()));
    }

    fn monitor(
        self: &Arc<Self>,
        task_id: TaskId,
        attempt_id: AttemptId,
        conversation: SessionId,
        mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    ) {
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(AgentEvent::TurnEnded { .. }) => {
                        runtime
                            .turn_ended(&task_id, attempt_id, &conversation)
                            .await;
                    }
                    Ok(AgentEvent::JobCompleted { .. }) => {
                        runtime.finish_retained_if_idle(conversation.as_str());
                    }
                    Ok(AgentEvent::CompactionEnded { .. }) => {
                        // 任务书+账本随压缩固定块在同一次 Replace 里重铺，不再单独 Steer。
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    /// 任务归属与目录：项目 id、用户点名目录都先校验；项目默认目录不存在就报错，
    /// 不偷偷退回对话目录。返回 (cwd, 项目 id, 目录来源)。
    fn resolve_placement(
        &self,
        context: &ToolContext,
        cwd: Option<String>,
        project: Option<String>,
    ) -> Result<(PathBuf, Option<String>, Option<String>), ToolFailure> {
        let project = match project {
            None => None,
            Some(project_id) => {
                let store = self
                    .projects
                    .as_ref()
                    .ok_or_else(|| ToolFailure::plain("项目库没开，不能给任务挂项目"))?;
                let project = store
                    .get(&project_id)
                    .map_err(|error| ToolFailure::plain(format!("项目读不出来：{error}")))?
                    .ok_or_else(|| ToolFailure::plain(format!("项目不存在：{project_id}")))?;
                Some(project)
            }
        };
        if let Some(raw) = cwd {
            let path = PathBuf::from(raw);
            let path = if path.is_absolute() {
                path
            } else {
                context.cwd.join(path)
            };
            if !path.is_dir() {
                return Err(ToolFailure::plain(format!(
                    "cwd 不是已存在的目录：{}",
                    path.display()
                )));
            }
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            return Ok((
                path,
                project.map(|project| project.id),
                Some("user".to_owned()),
            ));
        }
        if let Some(dir) = project.as_ref().and_then(|project| project.dir.as_ref()) {
            if !dir.is_dir() {
                return Err(ToolFailure::plain(format!(
                    "项目默认目录 {} 不存在",
                    dir.display()
                )));
            }
            let path = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
            return Ok((
                path,
                project.as_ref().map(|project| project.id.clone()),
                Some("project".to_owned()),
            ));
        }
        Ok((
            context.cwd.clone(),
            project.map(|project| project.id),
            Some("conversation".to_owned()),
        ))
    }

    /// 核对任务原话；到点轮的核对范围还包括这条约定存下的原文（当前为 condition）。
    fn verify_dispatch_quote(
        &self,
        conversation: &str,
        quote: &str,
    ) -> Result<tasks::QuoteSource, String> {
        match crate::ledger::verify_task_quote(&self.data_dir, conversation, quote) {
            Ok(source) => Ok(source),
            Err(error) => {
                let Some(origin) = self.dispatched_origin(conversation) else {
                    return Err(error);
                };
                let Some(calendar) = self.calendar.as_ref() else {
                    return Err(error);
                };
                let Ok(Some(appointment)) = calendar.appointment(&origin.appointment_id) else {
                    return Err(error);
                };
                if appointment.condition.trim().contains(quote.trim()) {
                    Ok(tasks::QuoteSource::Appointment {
                        appointment_id: origin.appointment_id,
                    })
                } else {
                    Err(error)
                }
            }
        }
    }

    /// Kind 的 task_accept：冻结模型选择与执行策略、落任务和 launch、回任务 id。
    ///
    /// 写入范围为空 = 只读调查（9 件工具）；有范围 = 写任务（加 write、edit）。
    /// bash 开关不在 task_accept 字段里，由服务在创建时按设置冻结：只读任务不给；
    /// 写任务打开时再加 bash 和后台任务三件（15 件）。
    fn accept(
        &self,
        context: &ToolContext,
        call_id: &str,
        args: TaskAcceptArgs,
    ) -> Result<(Task, bool), ToolFailure> {
        let TaskAcceptArgs {
            title,
            quote,
            requirement,
            dod,
            kind_note,
            cwd,
            project,
            write_scope,
            worktree,
            constraints,
        } = args;
        let conversation = context
            .session_id()
            .ok_or_else(|| ToolFailure::plain("没有会话 id，不能受理任务"))?
            .to_owned();
        crate::ledger::verify_user_words(&self.data_dir, &conversation, &constraints)
            .map_err(ToolFailure::plain)?;
        let quote_source = self
            .verify_dispatch_quote(&conversation, &quote)
            .map_err(ToolFailure::plain)?;
        let (cwd, project, cwd_source) = self.resolve_placement(context, cwd, project)?;
        let selection = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .clone()
            .ok_or_else(|| ToolFailure::plain("还没有可用的模型选择，不能派出任务"))?;
        let writing = write_scope
            .as_deref()
            .is_some_and(|scope| !scope.trim().is_empty());
        // 只读任务不给写的手段，也不给 bash：开关只对写任务生效。
        let bash = writing && self.exec_bash.load(Ordering::SeqCst);
        let tools = match (writing, bash) {
            (true, true) => THIN_WRITE_BASH_TOOLS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            (true, false) => THIN_WRITE_TOOLS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            (false, _) => THIN_TOOLS.iter().map(|name| (*name).to_owned()).collect(),
        };
        let draft = NewTask {
            title,
            requirement,
            dod,
            cwd,
            conversation: ConversationId::new(conversation.as_str()),
            model: ModelChoice {
                connection: selection.connection,
                model: selection.model,
                effort_wanted: selection.effort_wanted,
            },
            tools,
            quote: Some(quote),
            quote_source: Some(quote_source),
            kind_note,
            project_id: project.clone(),
            cwd_source: cwd_source.clone(),
        };
        let policy = TaskPolicy {
            write_scope,
            worktree,
            bash,
        };
        let key = format!("{conversation}:{call_id}");
        let (task, created) = self
            .store
            .create_task_once_with_constraints(draft, policy, constraints, &key, Self::now())
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        self.mark_dispatched(&conversation, &task);
        self.wake();
        Ok((task, created))
    }

    /// T052：Kind 的 task_graph_propose：冻结每个子任务的策略、校验并建图；这一步不启动。
    fn propose_graph(
        &self,
        context: &ToolContext,
        call_id: &str,
        args: TaskGraphArgs,
    ) -> Result<(Task, usize), ToolFailure> {
        let TaskGraphArgs {
            title,
            quote,
            requirement,
            dod,
            kind_note,
            cwd,
            project,
            subtasks,
            constraints,
        } = args;
        let conversation = context
            .session_id()
            .ok_or_else(|| ToolFailure::plain("没有会话 id，不能建任务图"))?
            .to_owned();
        crate::ledger::verify_user_words(&self.data_dir, &conversation, &constraints)
            .map_err(ToolFailure::plain)?;
        let quote_source = self
            .verify_dispatch_quote(&conversation, &quote)
            .map_err(ToolFailure::plain)?;
        let (cwd, project, cwd_source) = self.resolve_placement(context, cwd, project)?;
        let selection = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .clone()
            .ok_or_else(|| ToolFailure::plain("还没有可用的模型选择，不能建任务图"))?;
        let model = ModelChoice {
            connection: selection.connection,
            model: selection.model,
            effort_wanted: selection.effort_wanted,
        };
        let count = subtasks.len();
        let mut frozen_subtasks = Vec::with_capacity(count);
        for item in subtasks {
            let writing = item
                .write_scope
                .as_deref()
                .is_some_and(|scope| !scope.trim().is_empty());
            let bash = writing && self.exec_bash.load(Ordering::SeqCst);
            let tools = match (writing, bash) {
                (true, true) => THIN_WRITE_BASH_TOOLS
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
                (true, false) => THIN_WRITE_TOOLS
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
                (false, _) => THIN_TOOLS.iter().map(|name| (*name).to_owned()).collect(),
            };
            frozen_subtasks.push(NewSubtask {
                task: NewTask {
                    quote: Some(quote.clone()),
                    quote_source: Some(quote_source.clone()),
                    kind_note: kind_note.clone(),
                    title: item.title,
                    requirement: item.requirement,
                    dod: item.dod,
                    cwd: cwd.clone(),
                    conversation: ConversationId::new(conversation.as_str()),
                    model: model.clone(),
                    tools,
                    project_id: project.clone(),
                    cwd_source: cwd_source.clone(),
                },
                policy: TaskPolicy {
                    write_scope: item.write_scope,
                    worktree: item.worktree,
                    bash,
                },
                after: item.after,
                milestone: item.milestone,
                program_gate: item.program_gate,
            });
        }
        let graph = NewGraph {
            root: NewTask {
                title,
                requirement,
                dod,
                cwd,
                conversation: ConversationId::new(conversation.as_str()),
                model,
                tools: Vec::new(),
                quote: Some(quote),
                quote_source: Some(quote_source),
                kind_note,
                project_id: project.clone(),
                cwd_source: cwd_source.clone(),
            },
            subtasks: frozen_subtasks,
        };
        // T053：同一个 call id 重试返回同一张图，不建第二张。
        let (root, _created) = self
            .store
            .create_graph_once_with_constraints(graph, constraints, call_id, Self::now())
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        self.mark_dispatched(&conversation, &root);
        self.wake();
        Ok((root, count))
    }

    /// T052：确认任务图：同一个事务里记确认并把没有 needs 的子任务放进 launch。
    pub(crate) fn confirm_graph(&self, root: &TaskId) -> Result<usize, String> {
        let launched = self
            .store
            .confirm_graph(root, Self::now())
            .map_err(|error| error.to_string())?;
        self.wake();
        Ok(launched)
    }

    /// T053：取消一张任务图（暂不 / 被新卡替换 / 本轮没上卡）。
    pub(crate) fn cancel_graph(
        &self,
        root: &TaskId,
        reason: CancellationReason,
    ) -> Result<Cancellation, String> {
        let cancellation = self
            .store
            .cancel_graph(root, reason, Self::now())
            .map_err(|error| error.to_string())?;
        self.wake();
        Ok(cancellation)
    }

    /// T053：还没确认也没取消的图（NoCard 收尾扫用）。
    pub(crate) fn unconfirmed_graph_roots(&self) -> Result<Vec<Task>, String> {
        let mut roots = Vec::new();
        for id in self.store.task_ids().map_err(|error| error.to_string())? {
            let Some(task) = self.store.task(&id).map_err(|error| error.to_string())? else {
                continue;
            };
            if task.arrangement.as_ref().is_some_and(|arrangement| {
                arrangement.confirmed.is_none() && arrangement.cancelled.is_none()
            }) {
                roots.push(task);
            }
        }
        Ok(roots)
    }

    /// Kind 的 task_update：换写入范围、追加要求版本，然后在原会话开新 Attempt。
    ///
    /// 只在用户于 Kind 对话里同意扩大范围后用；本票只改写入范围（D83）。
    async fn update_scope(
        self: &Arc<Self>,
        task_id: &str,
        write_scope: String,
    ) -> Result<String, ToolFailure> {
        let id = TaskId::from(task_id);
        let task = self
            .store
            .task(&id)
            .map_err(|error| ToolFailure::plain(error.to_string()))?
            .ok_or_else(|| ToolFailure::plain(format!("任务不存在：{task_id}")))?;
        if let Some(reason) = task.cancelled_message() {
            return Err(ToolFailure::plain(reason));
        }
        if let Some(reason) = Self::trigger_task_rejection(&task) {
            return Err(ToolFailure::plain(reason));
        }
        if task.write_scope.is_none() {
            return Err(ToolFailure::plain(
                "这个任务是只读调查，没有写入范围；要让它写文件，请重新派一个带写入范围的写任务",
            ));
        }
        let Some(attempt) = task.latest_attempt().cloned() else {
            return Err(ToolFailure::plain(
                "这个任务还没开跑，没有可接着做的会话；请重新派任务",
            ));
        };
        if attempt.ended_at.is_none() {
            return Err(ToolFailure::plain(
                "这个任务这一轮还在跑；等它结束（或先停下）再改写入范围",
            ));
        }
        let updated = self
            .store
            .update_write_scope(&id, &write_scope, Self::now())
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        let scope = updated
            .write_scope
            .clone()
            .expect("update_write_scope 不接受空范围");
        // 给 ThinAgent 的说明用换算进工作树的范围；给 Kind 的回执仍用用户同意的原路径。
        let note_scope = updated
            .effective_write_scope()
            .unwrap_or_else(|| scope.clone());
        let version = updated.requirement_version();
        let opened = match self
            .open_resume(
                &updated,
                &attempt,
                AttemptTrigger::RequirementChanged,
                SlotKind::Wake,
                Vec::new(),
            )
            .await
        {
            Ok(opened) => opened,
            Err(ResumeBlock::Later) => {
                return Err(ToolFailure::plain(format!(
                    "写入范围已改成 {}（要求第 {version} 版），但任务现在接不上（会话在收尾或没有空位）；稍后再在任务对话里说一句就能接着做",
                    scope.display()
                )));
            }
            Err(ResumeBlock::AlreadyStarted) => {
                return Err(ToolFailure::plain(format!(
                    "写入范围已改成 {}（要求第 {version} 版）；任务已经接着开了新的 Attempt",
                    scope.display()
                )));
            }
            Err(ResumeBlock::PackageMissing) => {
                return Err(ToolFailure::plain(
                    "写入范围已更新，但任务包缺东西，接不上；看任务里的缺项",
                ));
            }
            Err(ResumeBlock::NoSession(reason)) => {
                return Err(ToolFailure::plain(format!(
                    "写入范围已改成 {}（要求第 {version} 版），但接不上原会话：{reason}",
                    scope.display()
                )));
            }
        };
        // 第一条输入写明新范围：新 Attempt 的模型从这条消息知道现在能写哪里。
        let mut note = format!(
            "AIOS：用户在 Kind 对话里同意把写入范围改成 {}（要求第 {version} 版）。先看当前的文件再继续；只在这个范围里写，范围外的调 task_handback。",
            note_scope.display()
        );
        let ledger = self.ledger_section(&updated);
        if !ledger.is_empty() {
            note.push_str(
                "

",
            );
            note.push_str(&ledger);
        }
        let request_id = format!("task-update:{id}:{}", opened.attempt_id);
        let sent = opened
            .host
            .send_message(&request_id, UserMessage::text(note), DeliveryMode::Steer)
            .await;
        if let Err(error) = sent {
            // 说明没送出去：别留一个没人做的空 Attempt。
            opened.host.shutdown().await;
            self.forget_running(opened.host.agent().id().as_str());
            best_effort(
                "改范围后的说明没送出去的 Attempt 没收尾",
                self.store
                    .end_attempt(&id, opened.attempt_id, Self::now(), AttemptEnd::NotOpened),
            );
            self.release_seat(id.as_str());
            return Err(ToolFailure::plain(format!(
                "写入范围已改成 {}（要求第 {version} 版），但新 Attempt 的说明没送进去：{error}",
                scope.display()
            )));
        }
        self.wake();
        Ok(format!(
            "写入范围已改成 {}（要求第 {version} 版）；已在原会话开新的 Attempt，第一条输入写明新范围。",
            scope.display()
        ))
    }

    /// 改要求后送进新 Attempt 的说明：原话按版本全列在最前，再接 Kind 的理解。
    pub(crate) fn requirement_change_note(
        &self,
        task: &Task,
        requirement: &str,
        version: u32,
    ) -> String {
        let mut note = self.version_quote_lines(task).join("\n");
        if !note.is_empty() {
            note.push_str("\n\n");
        }
        note.push_str(&format!(
            "AIOS：用户在 Kind 对话里把任务要求改成第 {version} 版：\nKind 的理解（不是用户原话）：{requirement}\nDoD：{}\n写集不变；先按新要求在原来的范围里继续；要改写入范围仍走 task_update。",
            task.dod
        ));
        if let Some(note_text) = self.latest_kind_note(task) {
            note.push_str(&format!(
                "
Kind 的补充（不算用户要求）：{note_text}"
            ));
        }
        note.push_str(
            "
写集不变；先按新要求在原来的范围里继续；要改写入范围仍走 task_update。",
        );
        note
    }

    /// T054.2：Kind 确认改要求后：同一事务改要求并追加版本，然后在原会话开新 Attempt。
    pub(crate) async fn update_requirement(
        self: &Arc<Self>,
        task_id: &str,
        requirement: String,
        dod: Option<String>,
        constraints: Option<Vec<String>>,
        quote: Option<String>,
        quote_source: Option<tasks::QuoteSource>,
        kind_note: Option<String>,
    ) -> Result<String, String> {
        let id = TaskId::from(task_id);
        let updated = self
            .store
            .update_requirement_with_quote(
                &id,
                &requirement,
                dod.as_deref(),
                constraints,
                quote.as_deref(),
                quote_source,
                kind_note.as_deref(),
                Self::now(),
            )
            .map_err(|error| error.to_string())?;
        let Some(attempt) = updated.latest_attempt().cloned() else {
            return Err(
                "要求已经改了，但这个任务还没开跑过，没有可接着做的会话；请重新派任务".to_owned(),
            );
        };
        let version = updated.requirement_version();
        let handoffs = self
            .store
            .current_handoffs(&updated)
            .map_err(|error| error.to_string())?;
        let opened = match self
            .open_resume(
                &updated,
                &attempt,
                AttemptTrigger::RequirementChanged,
                SlotKind::Wake,
                handoffs,
            )
            .await
        {
            Ok(opened) => opened,
            Err(ResumeBlock::Later) => {
                return Err(format!(
                    "要求已改成第 {version} 版，但任务现在接不上（会话在收尾或没有空位）；稍后再在任务对话里说一句就能接着做"
                ));
            }
            Err(ResumeBlock::AlreadyStarted) => {
                return Err(format!(
                    "要求已改成第 {version} 版；任务已经接着开了新的 Attempt"
                ));
            }
            Err(ResumeBlock::PackageMissing) => {
                return Err("要求已改，但任务包缺东西，接不上；看任务里的缺项".to_owned());
            }
            Err(ResumeBlock::NoSession(reason)) => {
                return Err(format!(
                    "要求已改成第 {version} 版，但接不上原会话：{reason}"
                ));
            }
        };
        let note = self.requirement_change_note(&updated, &requirement, version);
        let request_id = format!("task-requirement:{id}:{}", opened.attempt_id);
        if let Err(error) = opened
            .host
            .send_message(&request_id, UserMessage::text(note), DeliveryMode::Steer)
            .await
        {
            opened.host.shutdown().await;
            self.forget_running(opened.host.agent().id().as_str());
            best_effort(
                "改要求后的说明没送出去的 Attempt 没收尾",
                self.store
                    .end_attempt(&id, opened.attempt_id, Self::now(), AttemptEnd::NotOpened),
            );
            self.release_seat(id.as_str());
            return Err(format!(
                "要求已改成第 {version} 版，但新 Attempt 的说明没送进去：{error}"
            ));
        }
        self.wake();
        Ok(format!(
            "要求已改成第 {version} 版；已在原会话开新的 Attempt，第一条输入写明新要求和 DoD。"
        ))
    }

    async fn status_text(&self, task_id: &str) -> Result<String, ToolFailure> {
        let id = TaskId::from(task_id);
        let task = self
            .store
            .task(&id)
            .map_err(|error| ToolFailure::plain(error.to_string()))?
            .ok_or_else(|| ToolFailure::plain(format!("任务不存在：{task_id}")))?;
        // T052：总任务没有执行承载，只报汇总阶段和每个子任务的事实。
        if task.is_graph_root() {
            let stage = self
                .store
                .stage(&id)
                .map_err(|error| ToolFailure::plain(error.to_string()))?;
            let children = self
                .store
                .children(&id)
                .map_err(|error| ToolFailure::plain(error.to_string()))?;
            let mut lines = vec![
                format!("任务 {}（任务图）：{}", task.id, task.title),
                format!("阶段：{}", stage_label(stage)),
                format!("子任务数：{}", children.len()),
            ];
            for (index, version) in task.versions.iter().enumerate() {
                if let Some(quote) = version.quote.as_deref() {
                    lines.push(format!("用户原话（第 {} 版）：{quote}", index + 1));
                }
            }
            for (index, child) in children.iter().enumerate() {
                let child_stage = self
                    .store
                    .stage(&child.id)
                    .map_err(|error| ToolFailure::plain(error.to_string()))?;
                let report = child
                    .latest_report()
                    .map(|report| report_state_key(report.state))
                    .unwrap_or("无");
                lines.push(format!(
                    "{}. {}：{}；最近报告：{}",
                    index + 1,
                    child.title,
                    stage_label(child_stage),
                    report
                ));
            }
            return Ok(lines.join("\n"));
        }
        let stage = self
            .store
            .stage(&id)
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        let attempts = task
            .runs
            .iter()
            .map(|run| run.attempts.len())
            .sum::<usize>();
        let latest = task.latest_report().cloned();
        let mut lines = vec![
            format!("任务 {}：{}", task.id, task.title),
            format!("阶段：{}", stage_label(stage)),
            format!("Attempt 数：{attempts}"),
        ];
        for (index, version) in task.versions.iter().enumerate() {
            if let Some(quote) = version.quote.as_deref() {
                lines.push(format!("用户原话（第 {} 版）：{quote}", index + 1));
            }
        }
        if let Some(report) = &latest {
            lines.push(format!("最近报告（{}）：", report_state_key(report.state)));
            lines.push(report.summary.clone());
            if let Some(question) = &report.question {
                lines.push(format!("问题：{question}"));
            }
        } else {
            lines.push("最近报告：无".to_owned());
        }
        if latest
            .as_ref()
            .is_some_and(|report| report.state == ReportState::Waiting)
            && let Some(attempt) = task.latest_attempt()
        {
            let request_id = format!("status:{}:{}", task.id, attempt.id);
            if let Some(running) = self.running_for(&task.id, attempt.id) {
                match running
                    .host
                    .send_message(
                        &request_id,
                        UserMessage::text(STATUS_REQUEST),
                        DeliveryMode::Steer,
                    )
                    .await
                {
                    Ok(response) if response.duplicate => {
                        lines.push("已经向这个 Attempt 要过一次报告".to_owned());
                    }
                    Ok(_) => lines.push("已向 ThinAgent 要了一次报告".to_owned()),
                    Err(error) => lines.push(format!("要状态没送进去：{error}")),
                }
            } else {
                lines.push("任务会话没在跑，没要到报告".to_owned());
            }
        }
        Ok(lines.join("\n"))
    }

    /// 按 agent 状态决定怎么停；空闲但后台命令还在跑时杀最后一轮。
    fn interrupt_action(is_running: bool, background_turn: Option<u64>) -> Option<AgentInterrupt> {
        if is_running {
            Some(AgentInterrupt::Cancel)
        } else {
            background_turn.map(AgentInterrupt::KillTurn)
        }
    }

    /// 停下任务当前这一轮：只取消会话这一轮，不写 Attempt 结束、不排 relay。
    ///
    /// 下一句话进来时任务库还看到同一个未结束的 Attempt，投递会 Steer 进原会话接着做。
    pub(crate) async fn interrupt_task(
        &self,
        task_id: &str,
    ) -> Result<TaskInterruptResponse, ServiceError> {
        let id = TaskId::from(task_id);
        let task = self
            .store
            .task(&id)
            .map_err(|error| ServiceError::TaskUnavailable(format!("任务读不出来：{error}")))?
            .ok_or_else(|| ServiceError::TaskUnavailable(format!("找不到这个任务：{task_id}")))?;
        let not_running = || TaskInterruptResponse {
            interrupted: false,
            message: NOT_RUNNING.to_owned(),
        };
        let Some(attempt) = task.latest_attempt() else {
            return Ok(not_running());
        };
        if attempt.ended_at.is_some() {
            // 报告后保留的宿主还有后台命令：停止按钮也要停得掉。
            if let Some((conversation, turn)) = self.retained_background(&id)
                && let Some(running) = self.running_for(&id, attempt.id)
            {
                running
                    .host
                    .agent()
                    .kill_turn(turn, "用户停下保留的后台命令")
                    .await;
                self.forget_running(&conversation);
                self.notify_changed(&id);
                return Ok(TaskInterruptResponse {
                    interrupted: true,
                    message: "已停下保留的后台命令".to_owned(),
                });
            }
            return Ok(not_running());
        }
        let Some(running) = self.running_for(&id, attempt.id) else {
            return Ok(not_running());
        };
        // 正在跑一轮：取消这一轮（连同这一轮起的命令）。一轮已结束但后台还在跑：
        // 杀掉最后一轮起的后台命令，并照旧回 interrupted=true，让页面停得下来。
        let agent = running.host.agent();
        match Self::interrupt_action(agent.is_running(), agent.background_turn()) {
            Some(AgentInterrupt::Cancel) => {
                agent.cancel_with(session::AbortCause::User);
                self.cancel_background_limit(attempt.id);
                self.release_seat(task_id);
            }
            Some(AgentInterrupt::KillTurn(turn)) => {
                agent.kill_turn(turn, "用户停下这一轮").await;
                self.cancel_background_limit(attempt.id);
                self.release_seat(task_id);
            }
            None => return Ok(not_running()),
        }
        Ok(TaskInterruptResponse {
            interrupted: true,
            message: "已停下这一轮".to_owned(),
        })
    }

    /// 改任务归属（任务图整张）；只写 Task 字段，不动要求版本和目录。
    pub(crate) fn set_task_project(
        &self,
        task_id: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<String>, tasks::TaskError> {
        let changed = self.store.set_project(&TaskId::from(task_id), project_id)?;
        for id in &changed {
            self.notify_changed(id);
        }
        Ok(changed
            .into_iter()
            .map(|id| id.as_str().to_owned())
            .collect())
    }

    /// 任务页/Kind 卡共用；沿用运行时的任务库与变化通知。
    pub(crate) fn cancel_with_downstream(
        &self,
        task_id: &TaskId,
        source: tasks::CancellationSource,
        from: Option<TaskId>,
        now: i64,
    ) -> Result<Vec<(TaskId, tasks::TaskCancellation)>, tasks::TaskError> {
        let result = self
            .store
            .cancel_with_downstream(task_id, source, from, now);
        if result.is_ok() {
            self.notify_changed(task_id);
            self.wake();
        }
        result
    }

    pub(crate) async fn stop_task(
        self: &Arc<Self>,
        task_id: &str,
        mode: StopMode,
    ) -> Result<TaskStopResponse, ServiceError> {
        let id = TaskId::from(task_id);
        let task = self
            .store
            .task(&id)
            .map_err(|error| ServiceError::TaskUnavailable(format!("任务读不出来：{error}")))?
            .ok_or_else(|| ServiceError::TaskUnavailable(format!("找不到这个任务：{task_id}")))?;
        if let Some(reason) = Self::trigger_task_rejection(&task) {
            return Err(ServiceError::TaskUnavailable(reason));
        }
        let stage = self
            .store
            .stage(&id)
            .map_err(|error| ServiceError::TaskUnavailable(format!("阶段读不出来：{error}")))?;
        let not_running = || TaskStopResponse {
            stopped: false,
            stage: stage_key(stage).to_owned(),
            message: NOT_RUNNING.to_owned(),
        };
        let Some(attempt) = task.latest_attempt() else {
            return Ok(not_running());
        };
        if attempt.ended_at.is_some() {
            // 报告后保留的后台命令，立即停/Kind task_stop 都能停。
            if let Some((conversation, turn)) = self.retained_background(&id)
                && let Some(running) = self.running_for(&id, attempt.id)
            {
                running
                    .host
                    .agent()
                    .kill_turn(turn, "用户停下保留的后台命令")
                    .await;
                self.forget_running(&conversation);
                self.notify_changed(&id);
                self.release_seat(task_id);
                return Ok(TaskStopResponse {
                    stopped: true,
                    stage: stage_key(stage).to_owned(),
                    message: format!("已停下保留的后台命令：任务 {task_id}。"),
                });
            }
            return Ok(not_running());
        }
        // T057：程序门没有 ThinAgent 会话；立即停（含收尾停）都结束进程组、记未确定。
        if task.is_program_gate() {
            return self
                .stop_program_gate(task_id, &id, attempt.id, "被停")
                .await;
        }
        let Some(running) = self.running_for(&id, attempt.id) else {
            return Ok(not_running());
        };
        let attempt_id = attempt.id;
        if mode == StopMode::AfterReport {
            let conversation = running.host.agent().id().clone();
            let request_id = format!("task-stop:{id}:{attempt_id}");
            running
                .host
                .send_message(
                    &request_id,
                    UserMessage::text(AFTER_REPORT_STOP),
                    DeliveryMode::Inject,
                )
                .await
                .map_err(|error| {
                    ServiceError::TaskUnavailable(format!("收尾停没插进会话：{error}"))
                })?;
            running.host.agent().stop_after_report();
            // 这一轮已经结束、没有下一步可等：照「turn 结束还没报」当场记 waiting + stopped。
            let idle = !running.host.agent().is_running();
            if idle {
                self.settle_after_report(&id, attempt_id, &conversation);
            }
            // 停止落在刚收尾的那一轮上时，is_running 可能还没翻成 false，而那次 turn 结束早已
            // 走过 turn_ended：等会话空闲后再收一次。settle 幂等，重复调用什么都不改。
            let runtime = Arc::clone(self);
            let agent = running.host.agent().clone();
            let task = id.clone();
            let settled_conversation = conversation.clone();
            tokio::spawn(async move {
                agent.idle().await;
                runtime.settle_after_report(&task, attempt_id, &settled_conversation);
            });
            let stage = self
                .store
                .stage(&id)
                .map_err(|error| ServiceError::TaskUnavailable(format!("阶段读不出来：{error}")))?;
            let how = if idle {
                "这一轮已经结束且还没报结果，任务已停"
            } else {
                "做完手上这一步、用 task_report 报一次就结束"
            };
            return Ok(TaskStopResponse {
                stopped: true,
                stage: stage_key(stage).to_owned(),
                message: format!(
                    "已收尾停：{how}（任务 {task_id}，阶段：{}）。",
                    stage_label(stage)
                ),
            });
        }
        let outcome = self
            .store
            .stop_attempt(&id, attempt_id, Self::now())
            .map_err(|error| ServiceError::TaskUnavailable(format!("停止没写进任务库：{error}")))?;
        match outcome {
            StopAttempt::Stopped => {}
            StopAttempt::AlreadyStopped | StopAttempt::AlreadyEnded => return Ok(not_running()),
        }
        // 06 篇 4.2 第 1 步：先持久保存停止要求，再掐断会话。让出一次，使取消之前提交的
        // 停止事实对效果泵和日志观察者可见；先后顺序因此能被分辨，而不是同一 poll 里两件事。
        tokio::task::yield_now().await;
        let agent = running.host.agent();
        agent.cancel_with(session::AbortCause::User);
        // 开了 bash 的任务：后台命令也一起停，收到 SIGTERM 而不是留到任务结束之后（D85）。
        agent.stop_background_jobs(STOPPED_JOB_REASON).await;
        self.cancel_background_limit(attempt_id);
        self.forget_running(running.host.agent().id().as_str());
        self.release_seat(task_id);
        self.wake();
        let stage = self
            .store
            .stage(&id)
            .map_err(|error| ServiceError::TaskUnavailable(format!("阶段读不出来：{error}")))?;
        Ok(TaskStopResponse {
            stopped: true,
            stage: stage_key(stage).to_owned(),
            message: format!("已立即停：任务 {task_id}（阶段：{}）。", stage_label(stage)),
        })
    }
}

impl Drop for TaskRuntime {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }
}

/// 记一条不该打断效果泵的失败：留信号，不把主流程吞掉。
fn best_effort<T>(what: &str, result: Result<T, TaskError>) {
    if let Err(error) = result {
        eprintln!("{what}：{error}");
    }
}

fn missing_item(text: impl Into<String>) -> MissingItem {
    MissingItem::new(text).expect("缺项描述非空")
}

fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::NeedsConditions => "needs_conditions",
        Stage::Pending => "pending",
        Stage::Running => "running",
        Stage::ReadyForReview => "ready_for_review",
        Stage::RequirementMet => "requirement_met",
        Stage::Closed => "closed",
    }
}

fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::NeedsConditions => "待补足条件",
        Stage::Pending => "待执行",
        Stage::Running => "执行中",
        Stage::ReadyForReview => "待检查",
        Stage::RequirementMet => "任务要求已满足",
        Stage::Closed => "已关闭",
    }
}

#[cfg(test)]
mod trigger_guard_tests {
    use super::*;

    fn draft(cwd: &std::path::Path, title: &str) -> NewTask {
        NewTask {
            quote: None,
            quote_source: None,
            kind_note: None,
            project_id: None,
            cwd_source: None,
            title: title.to_owned(),
            requirement: "到点做事".to_owned(),
            dod: "做完".to_owned(),
            cwd: cwd.to_path_buf(),
            conversation: ConversationId::new("kind-trigger-guard"),
            model: ModelChoice {
                connection: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                effort_wanted: "high".to_owned(),
            },
            tools: Vec::new(),
        }
    }

    #[test]
    fn four_tools_share_the_same_trigger_rejection() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
        let task = store
            .create_task(draft(directory.path(), "触发"), 1_800_000_000_000)
            .unwrap();
        assert!(TaskRuntime::trigger_task_rejection(&task).is_none());
        store
            .set_origin(
                &task.id,
                AppointmentOrigin {
                    appointment_id: "appointment".to_owned(),
                    run_id: 1,
                    role: AppointmentRole::Trigger,
                },
            )
            .unwrap();
        let task = store.task(&task.id).unwrap().unwrap();
        assert_eq!(
            TaskRuntime::trigger_task_rejection(&task).as_deref(),
            Some("这是约定到点的任务，不能接着做"),
            "task_continue/task_update/task_stop/task_handback 都走这一句"
        );
    }
}

#[cfg(test)]
mod trigger_tool_tests {
    use std::sync::{Arc, Mutex, Weak};

    use model::{DEFAULT_CONTEXT_WINDOW, Effort};
    use tools::{Tool, ToolContext, ToolSessionState};

    use super::*;
    use crate::executor::task_tools::{
        TaskContinueTool, TaskHandbackTool, TaskStopTool, TaskUpdateTool,
    };
    use crate::root::RootSessionOptions;

    fn runtime() -> (tempfile::TempDir, Arc<TaskRuntime>) {
        let directory = tempfile::tempdir().unwrap();
        let options = RootSessionOptions {
            projects: None,
            data_dir: directory.path().to_path_buf(),
            cwd: directory.path().to_path_buf(),
            model: model::DeepSeekConfig {
                provider: "deepseek".to_owned(),
                base_url: "http://127.0.0.1:1".to_owned(),
                model: "deepseek-flash".to_owned(),
                max_tokens: 2048,
                context_window: DEFAULT_CONTEXT_WINDOW,
                effort: Effort::new("high"),
                tool_update: None,
            },
            config: agent::AgentConfig::default(),
            credentials: None,
            identity: None,
            model_factory: None,
            append_hook_factory: None,
            keyring_changed: None,
            actions: Arc::new(crate::actions::ActionRegistry::empty()),
            exa_base_url: None,
            web_listen: None,
            task_runtime: None,
        };
        let runtime =
            TaskRuntime::new(&options, Arc::new(Mutex::new(None)), Weak::new(), None).unwrap();
        (directory, Arc::new(runtime))
    }

    fn trigger_task(directory: &std::path::Path, runtime: &TaskRuntime) -> Task {
        let draft = NewTask {
            quote: None,
            quote_source: None,
            kind_note: None,
            project_id: None,
            cwd_source: None,
            title: "到点任务".to_owned(),
            requirement: "到点办完".to_owned(),
            dod: "办完".to_owned(),
            cwd: directory.to_path_buf(),
            conversation: ConversationId::new("kind-trigger-tool"),
            model: ModelChoice {
                connection: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                effort_wanted: "high".to_owned(),
            },
            tools: Vec::new(),
        };
        let task = runtime
            .store
            .create_trigger_task_once(draft, "tool-reject", TaskRuntime::now())
            .unwrap()
            .0;
        runtime
            .store
            .set_origin(
                &task.id,
                AppointmentOrigin {
                    appointment_id: "appointment".to_owned(),
                    run_id: 1,
                    role: AppointmentRole::Trigger,
                },
            )
            .unwrap();
        runtime.store.task(&task.id).unwrap().unwrap()
    }

    async fn error_of(tool: &dyn Tool, arguments: Value) -> String {
        let directory = tempfile::tempdir().unwrap();
        let context = ToolContext::with_session(
            directory.path(),
            Arc::new(ToolSessionState::new(directory.path(), "kind-trigger-tool")),
        );
        let result = tool
            .execute(
                ToolCall {
                    id: "call".to_owned(),
                    name: tool.name().to_owned(),
                    arguments: arguments.to_string(),
                },
                &context,
            )
            .await;
        assert!(result.is_error, "应当拒绝：{result:?}");
        result.content
    }

    #[tokio::test]
    async fn trigger_tasks_stay_out_of_interrupt_unconfirmed_graph_and_dream_lists() {
        let (directory, runtime) = runtime();
        let task = trigger_task(directory.path(), &runtime);
        assert!(
            !task.is_interrupted(),
            "触发任务没有 Attempt，不会进打断事实"
        );
        assert!(
            runtime.unconfirmed_graph_roots().unwrap().is_empty(),
            "触发任务不是图根，不会进未确认图"
        );
        let dream = crate::memory::task_facts(&runtime, 0).unwrap();
        assert!(
            dream.iter().all(|fact| fact.task_id != task.id.as_str()),
            "触发任务不进做梦事实：{dream:?}"
        );
    }

    #[tokio::test]
    async fn update_and_handback_reject_trigger_tasks() {
        let (directory, runtime) = runtime();
        let task = trigger_task(directory.path(), &runtime);
        let update = TaskUpdateTool::new(runtime.clone());
        let text = error_of(
            &update,
            json!({"task_id": task.id.as_str(), "write_scope": "a.txt"}),
        )
        .await;
        assert!(text.contains("这是约定到点的任务，不能接着做"), "{text}");

        let handback = TaskHandbackTool::new(runtime.clone(), task.id.clone(), AttemptId(1));
        let context = ToolContext::with_session(
            directory.path(),
            Arc::new(ToolSessionState::new(directory.path(), "thin")),
        );
        let result = handback
            .execute(
                ToolCall {
                    id: "call".to_owned(),
                    name: "task_handback".to_owned(),
                    arguments: json!({"reason": "超出范围"}).to_string(),
                },
                &context,
            )
            .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("这是约定到点的任务，不能接着做"),
            "{}",
            result.content
        );
    }

    #[tokio::test]
    async fn continue_and_stop_tools_reject_trigger_tasks() {
        let (directory, runtime) = runtime();
        let task = trigger_task(directory.path(), &runtime);
        let context = ToolContext::with_session(
            directory.path(),
            Arc::new(ToolSessionState::new(directory.path(), "kind")),
        );
        let continue_tool = TaskContinueTool::new(runtime.clone());
        let result = continue_tool
            .execute(
                ToolCall {
                    id: "call".to_owned(),
                    name: "task_continue".to_owned(),
                    arguments: json!({"task_id": task.id.as_str()}).to_string(),
                },
                &context,
            )
            .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("这是约定到点的任务，不能接着做"),
            "{}",
            result.content
        );

        let stop_tool = TaskStopTool::new(runtime.clone());
        let result = stop_tool
            .execute(
                ToolCall {
                    id: "call".to_owned(),
                    name: "task_stop".to_owned(),
                    arguments: json!({"task_id": task.id.as_str(), "mode": "now"}).to_string(),
                },
                &context,
            )
            .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("这是约定到点的任务，不能接着做"),
            "{}",
            result.content
        );
    }
}

#[cfg(test)]
mod interrupt_decision_tests {
    use super::*;

    #[test]
    fn idle_agent_with_background_turns_interrupts_the_last_turn() {
        assert_eq!(
            TaskRuntime::interrupt_action(false, Some(7)),
            Some(AgentInterrupt::KillTurn(7))
        );
        assert_eq!(
            TaskRuntime::interrupt_action(true, Some(7)),
            Some(AgentInterrupt::Cancel)
        );
        assert_eq!(TaskRuntime::interrupt_action(false, None), None);
    }
}
