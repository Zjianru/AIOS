//! T024.3：任务执行者（launch）、ThinAgent 的 task_report、上报 relay，以及 Kind 的两个任务工具。
//!
//! 事实边界：任务聚合和效果表在 tasks；这里把效果泵成真实动作（开 ThinAgent 会话、
//! 把报告 Steer 回发起的 Kind 会话），不自己保存任务状态。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent::{AgentEvent, AgentOpenOptions, ToolPolicy};
use model::{ContentBlock, Effort, UserMessage};
use serde_json::{Value, json};
use session::SessionId;
use tasks::{
    Acceptance, Attempt, AttemptEnd, AttemptId, AttemptTrigger, BootId, BootScan, ConversationId,
    EffectClaim, EffectKind, EffectPayload, Input, InputAttachment, MissingItem, ModelChoice,
    NewInput, NewReport, NewTask, ReportId, ReportOrigin, ReportState, SlotKind, Stage,
    StopAttempt, Store, Task, TaskError, TaskId, TaskPolicy, WorkerSlots,
};
use tokio::sync::broadcast;
use tools::{Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolResult};

use crate::error::ServiceError;
use crate::events::{page_events, service_event};
use crate::input::attachment_error;
use crate::protocol::{
    DeliveryMode, FollowRequest, FollowResponse, ServiceEvent, StopMode, TaskInterruptResponse,
    TaskReadHeader, TaskReadRequest, TaskReadResponse, TaskSayRequest, TaskSayResponse,
    TaskStopResponse,
};
use crate::root::RootSessionOptions;
use crate::session_host::ServiceHost;

/// 任务库文件名（数据目录下）。
pub const TASKS_DB_FILE: &str = "tasks.sqlite";

/// ThinAgent 的只读工具名单（构造期 only 过滤；顺序就是名单顺序）。
pub const THIN_TOOLS: [&str; 9] = [
    "read",
    "glob",
    "grep",
    "skill",
    "todo_write",
    "web_search",
    "web_fetch",
    "task_report",
    "task_handback",
];

/// 写任务的 ThinAgent 工具名单：只读 9 件加 write、edit（D85）。
pub const THIN_WRITE_TOOLS: [&str; 11] = [
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
];

/// 开了 bash 的写任务：写任务 11 件再加 bash 和后台任务三件（D85）。
pub const THIN_WRITE_BASH_TOOLS: [&str; 15] = [
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
When the work is done, report ready with the answer and the evidence it rests on. \
When you cannot continue without a missing tool, permission, file, credential or an answer, report blocked with exactly what is missing and what the user must do; put a direct question in question when only the user can answer it. \
When the work cannot produce the result, report failed with what failed and what you already found. \
Report once, when it is a result: do not report progress, and do not claim work you did not do."
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

/// 确认接续后给 ThinAgent 的第一条输入（照 DSH RECOVERY_NOTE，换成 AIOS 的话；spec v1）。
const RESUME_NOTE: &str = "AIOS 在这个任务进行中重启了。先看当前的文件和状态再继续；已经做完的外部动作不要再做一遍。把原来的任务做完，然后用 task_report 上报。";

/// 重启汇报里一份最近报告最多带多少字（spec v1：last_report ≤ 300）。
const RESTART_REPORT_CHARS: usize = 300;

/// 执行者的一个已开会话；task_status 向 waiting 的 ThinAgent 要报告时用它。
struct RunningThin {
    host: Arc<ServiceHost>,
    task_id: TaskId,
    attempt_id: AttemptId,
}

/// 接上原会话的结果：新 Attempt 和它的宿主；第一条消息由调用方送。
struct OpenedResume {
    host: Arc<ServiceHost>,
    attempt_id: AttemptId,
}

/// Kind 的 task_accept 解析后的参数：建任务并冻结执行策略需要的全部字段。
struct TaskAcceptArgs {
    title: String,
    requirement: String,
    dod: String,
    cwd: Option<String>,
    write_scope: Option<String>,
    worktree: bool,
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

/// 任务执行者：效果泵、ThinAgent 会话和任务工具的共享状态。
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
    /// 测试钩子：下一次 relay 在 Kind append 前「崩掉」——留下效果行，泵退出。
    crash_before_relay: AtomicBool,
}

impl TaskRuntime {
    pub(crate) fn new(
        options: &RootSessionOptions,
        selection: Arc<Mutex<Option<session::ModelSelection>>>,
        inner: Weak<tokio::sync::Mutex<Option<ServiceHost>>>,
    ) -> Result<Self, TaskError> {
        let store = Store::open(options.data_dir.join(TASKS_DB_FILE))?;
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
            exec_paused: AtomicBool::new(exec_paused),
            exec_seats: AtomicUsize::new(exec_seats),
            exec_bash: AtomicBool::new(exec_bash),
            slots: Mutex::new(WorkerSlots::new()),
            running: Mutex::new(std::collections::HashMap::new()),
            crash_before_relay: AtomicBool::new(false),
        })
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
        let task = self
            .store
            .task(&id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("找不到这个任务：{task_id}"))?;
        let Some(report) = task.latest_report() else {
            return Err("这个任务还没有可以接受的结果".to_owned());
        };
        if report.id.as_str() != report_id {
            return Err("这份结果已经不是任务的最新报告，不能再接受".to_owned());
        }
        if report.version != task.requirement_version() {
            return Err(format!(
                "要求已经改过，旧版本（第 {} 版）的报告不能接受",
                report.version
            ));
        }
        if report.state != ReportState::Ready {
            return Err("这份结果不是可以接受的状态".to_owned());
        }
        self.store
            .accept(&id, Self::now())
            .map_err(|error| error.to_string())
    }

    /// 任务对话的只读快照：头部来自任务聚合，事件来自当前 Attempt 的会话日志。
    ///
    /// 只读打开：不创建会话、不拿写者锁；会话日志缺失/损坏时把原因带进可读错误。
    /// 分页照 events 的预算（500 条消息/2 轮/50 条下限），`lastSeq` 始终是日志末端。
    pub(crate) fn task_read(
        &self,
        request: TaskReadRequest,
    ) -> Result<TaskReadResponse, ServiceError> {
        let (task, attempt_id, conversation) = self.task_attempt(&request.task_id)?;
        let session_id = SessionId::parse(conversation.as_str()).map_err(|error| {
            ServiceError::TaskUnavailable(format!("任务对话的会话 id 不合法：{error}"))
        })?;
        let snapshot = session::SessionStore::open_read_only(&self.data_dir, &session_id)
            .map_err(|error| ServiceError::TaskUnavailable(format!("任务对话打不开：{error}")))?;
        let all = snapshot.events();
        let (selected, _) = page_events(all, request.before_seq, 500, None, None);
        Ok(TaskReadResponse {
            task: TaskReadHeader {
                id: task.id.as_str().to_owned(),
                title: task.title.clone(),
                requirement: task.requirement.clone(),
                dod: task.dod.clone(),
                cwd: task.cwd.to_string_lossy().into_owned(),
                write_scope: crate::result_cards::write_scope_display(&task),
                bash: task.bash,
                model: task.model.model.clone(),
                tools: task.tools.clone(),
                attempts: task.runs.iter().map(|run| run.attempts.len() as u64).sum(),
                // 在跑表里还开着、而且这一轮真的在跑，菜单才可点；只开着等报告的会话不算。
                running: self
                    .running_for(&task.id, attempt_id)
                    .is_some_and(|running| running.host.agent().is_running()),
                stopped: task
                    .latest_attempt()
                    .is_some_and(|attempt| attempt.end == Some(AttemptEnd::Stopped)),
                interrupted: task.is_interrupted(),
                // 启动扫描已经打断、但给 Kind 的 restart 汇报还在队里：横幅「待 Kind 核实」。
                // 汇报送到或被收掉以后，任务仍是「被打断了」。
                restart_pending: task.is_interrupted()
                    && self.store.restart_pending(&task.id).map_err(|error| {
                        ServiceError::TaskUnavailable(format!("重启汇报的状态读不出来：{error}"))
                    })?,
                // waiting 不是终态：report 只把会话移出「在跑」，之后 task_status 会起新的一轮；
                // 页面只能拿终态报告的时间收尾没有 turn/end 的最后一轮。
                reported_at: task
                    .latest_report()
                    .filter(|report| {
                        matches!(
                            report.state,
                            ReportState::Ready | ReportState::Blocked | ReportState::Failed
                        )
                    })
                    .map(|report| report.at),
            },
            events: selected.iter().map(|event| service_event(event)).collect(),
            last_seq: all.last().map(|event| event.seq).unwrap_or(0),
        })
    }

    /// 任务当前 Attempt 的会话：读任务聚合、取最近一次 Attempt 和它的会话。
    fn task_attempt(
        &self,
        task_id: &str,
    ) -> Result<(Task, AttemptId, ConversationId), ServiceError> {
        let task = self
            .store
            .task(&TaskId::from(task_id))
            .map_err(|error| ServiceError::TaskUnavailable(format!("任务读不出来：{error}")))?
            .ok_or_else(|| ServiceError::TaskUnavailable(format!("找不到这个任务：{task_id}")))?;
        let (attempt_id, conversation) = {
            let attempt = task.latest_attempt().ok_or_else(|| {
                ServiceError::TaskUnavailable("这个任务还没开跑，没有可看的对话".to_owned())
            })?;
            let conversation = attempt.conversation.clone().ok_or_else(|| {
                ServiceError::TaskUnavailable("这个任务还没开跑，没有可看的对话".to_owned())
            })?;
            (attempt.id, conversation)
        };
        Ok((task, attempt_id, conversation))
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
        let (task, attempt, conversation) = self.task_attempt(task_id)?;
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

    /// 全局暂停的闸门：RootHost 先写设置文件，再改这里的内存值。
    ///
    /// 只挡 launch 准入；relay/notice 照送。排队的效果留在库里，开闸后按原顺序领取。
    pub(crate) fn set_exec_paused(&self, paused: bool) {
        self.exec_paused.store(paused, Ordering::SeqCst);
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

    /// 正在跑一轮的任务（按任务 id）。turn 结束的不算——席位在轮次结束时释放。
    fn running_tasks(&self) -> Vec<String> {
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .values()
            .filter(|entry| entry.host.agent().is_running())
            .map(|entry| entry.task_id.as_str().to_owned())
            .collect()
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

    /// 测试用：模拟进程退出——丢掉还开着的 ThinAgent（放开会话写锁），效果泵不再领取。
    ///
    /// 生产里进程退出由操作系统回收；测试要复现「重启后会话写者已经放开」。
    #[doc(hidden)]
    pub fn crash(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.running.lock().expect("thinagent 表锁被毒化").clear();
        self.wake();
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
                let mut worked = false;
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
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }

    /// 把一条 relay Steer 进发起的 Kind 会话；那个会话不是当前根会话时 Later。
    async fn deliver_relay(self: &Arc<Self>, claim: EffectClaim) {
        let (task, body, request_id) = match self.relay_message(&claim) {
            Ok(Some(parts)) => parts,
            Ok(None) => {
                self.finish_or_fail(&claim, Ok(()));
                return;
            }
            Err(error) => {
                self.finish_or_fail(&claim, Err(error));
                return;
            }
        };
        // 测试钩子：在 Kind append 前模拟进程停掉，效果行原样留给下一次启动。
        if self.crash_before_relay.swap(false, Ordering::SeqCst) {
            self.shutting_down.store(true, Ordering::SeqCst);
            return;
        }
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
        let message = UserMessage::text(body.to_string());
        let sent = host
            .send_message(&request_id, message, DeliveryMode::Steer)
            .await;
        drop(guard);
        match sent {
            Ok(_) => best_effort("relay 完成没写进任务库", self.store.complete_effect(&claim)),
            Err(error) => best_effort(
                "relay 失败没写进任务库",
                self.store
                    .fail_effect(&claim, &error.to_string(), Self::now()),
            ),
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

    /// 把任务里还没送出的用户输入 Steer 进任务会话；request id task-say:<inputId> 判重。
    async fn deliver_delivery(self: &Arc<Self>, claim: EffectClaim) {
        let EffectPayload::Delivery { task_id } = &claim.payload else {
            self.finish_or_fail(&claim, Err("delivery 载荷不是 delivery".to_owned()));
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
        let Some(attempt) = task.latest_attempt() else {
            self.finish_or_fail(&claim, Ok(()));
            return;
        };
        // 最近一次 Attempt 是重启打断的：这次说话就是接续（trigger resume、wake 席位、
        // 先放恢复说明）。别的结束原因照 T028 的说话接续走。
        let mode = if task.is_interrupted() {
            ResumeMode::Interrupted
        } else {
            ResumeMode::Say
        };
        let mut recovery_attempt = None;
        let host = match self.running_for(task_id, attempt.id) {
            Some(running) => running.host,
            // 没在跑：停/结束之后在任务对话里说话，接着原会话开新一轮（T028.2）。
            None => match self.resume_for_delivery(&claim, &task, attempt, mode).await {
                Some(opened) => {
                    if matches!(mode, ResumeMode::Interrupted) {
                        recovery_attempt = Some(opened.attempt_id);
                    }
                    opened.host
                }
                None => return,
            },
        };
        let mut sent = Ok(());
        let inputs = task.inputs.len();
        for (index, input) in task.inputs.iter().enumerate() {
            // 打断后的第一句话：恢复说明和用户的话同一条消息，恢复说明在前；
            // request id 记在这次接续上。
            let recovery = recovery_attempt.filter(|_| index + 1 == inputs);
            let request_id = match recovery {
                Some(attempt_id) => format!("resume:{task_id}:{attempt_id}"),
                None => format!("task-say:{}", input.id.as_str()),
            };
            let mut message = match self.say_message(input) {
                Ok(message) => message,
                Err(error) => {
                    sent = Err(format!(
                        "输入 {} 的附件没读回来：{error}",
                        input.id.as_str()
                    ));
                    break;
                }
            };
            if recovery.is_some() {
                let mut blocks = message.into_message().content;
                blocks.insert(0, ContentBlock::text(RESUME_NOTE));
                message = UserMessage::new(blocks);
            }
            if let Err(error) = host
                .send_message(&request_id, message, DeliveryMode::Steer)
                .await
            {
                sent = Err(format!(
                    "输入 {} 没送进任务会话：{error}",
                    input.id.as_str()
                ));
                break;
            }
        }
        match sent {
            Ok(()) => best_effort(
                "delivery 完成没写进任务库",
                self.store.complete_effect(&claim),
            ),
            Err(error) => best_effort(
                "delivery 失败没写进任务库",
                self.store.fail_effect(&claim, &error, Self::now()),
            ),
        }
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
        let (trigger, slot) = match mode {
            ResumeMode::Say => (AttemptTrigger::UserMessage, SlotKind::Person),
            ResumeMode::Interrupted => (AttemptTrigger::Resume, SlotKind::Wake),
        };
        match self.open_resume(task, attempt, trigger, slot).await {
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
            .open_resume(&task, &attempt, AttemptTrigger::Resume, SlotKind::Wake)
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
        let request_id = format!("resume:{task_id}:{}", opened.attempt_id);
        let sent = opened
            .host
            .send_message(
                &request_id,
                UserMessage::text(RESUME_NOTE),
                DeliveryMode::Steer,
            )
            .await;
        if let Err(error) = sent {
            // 恢复说明没送出去：别留一个没人做的空 Attempt。
            opened.host.shutdown().await;
            self.running
                .lock()
                .expect("thinagent 表锁被毒化")
                .remove(opened.host.agent().id().as_str());
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
        let attempt_id = match self
            .store
            .begin_followup_with(&task.id, trigger, Self::now())
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
        // 上一轮可能还在收尾：会话写锁没让出来之前开不出续接会话，先短暂等它。
        // 只等写者持有这一种；别的错误交给 open 报。
        for _ in 0..100 {
            match session::SessionStore::open(&self.data_dir, &resume) {
                Err(session::SessionError::AlreadyOwned { .. }) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                _ => break,
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
            },
        );
        self.monitor(task.id.clone(), attempt_id, conversation, events);
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

    /// 拼一条 relay 的正文和 request id；报告和停止共用 task_report 的形状。
    ///
    /// 返回 Ok(None)：载荷指向的任务已经不在了，效果行直接收掉。
    fn relay_message(&self, claim: &EffectClaim) -> Result<Option<(Task, Value, String)>, String> {
        let task_id = match &claim.payload {
            EffectPayload::Relay { task_id, .. } | EffectPayload::Stop { task_id, .. } => task_id,
            EffectPayload::Launch { .. }
            | EffectPayload::Delivery { .. }
            | EffectPayload::Notice { .. }
            | EffectPayload::Restart { .. } => {
                return Err("relay 载荷不是 relay".to_owned());
            }
        };
        let Some(task) = self
            .store
            .task(task_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let stage = self
            .store
            .stage(&task.id)
            .map_err(|error| error.to_string())?;
        match &claim.payload {
            EffectPayload::Relay { report_id, .. } => {
                let Some(report) = task.reports.iter().find(|report| report.id == *report_id)
                else {
                    return Err(format!("任务 {} 没有报告 {report_id}", task.id));
                };
                let (summary, chars) = relay_summary(&report.summary);
                let mut body = json!({
                    "kind": "task_report",
                    "task": {
                        "id": task.id.as_str(),
                        "title": task.title,
                        "stage": stage_key(stage),
                    },
                    "origin": report_origin_key(report.origin),
                    "state": report_state_key(report.state),
                    "summary": summary,
                });
                if let Some(chars) = chars {
                    body["chars"] = json!(chars);
                }
                if let Some(question) = &report.question {
                    body["question"] = json!(question);
                }
                // 写任务的结果卡：写入范围、改动清单和固定一行。只有当前版本的 ready 报告
                // 才给「待你核」的卡状态（阶段仍由 task.stage 报待检查）。
                if let Some(scope) = crate::result_cards::write_scope_display(&task) {
                    let changed = crate::result_cards::changed_files(&self.data_dir, &task);
                    body["changed_line"] =
                        json!(crate::result_cards::changed_line(changed.len(), &scope));
                    body["changed"] = json!(changed);
                    body["write_scope"] = json!(scope);
                    if report.state == ReportState::Ready
                        && report.version == task.requirement_version()
                    {
                        body["card_state"] = json!("review");
                    }
                }
                // 开了 bash 的写任务：卡上照实标明 bash 的改动不在写集清单里（D85）。
                if task.bash {
                    body["bash_note"] = json!(BASH_NOTE);
                }
                // 开了工作树：卡上写路径，用户和 Kind 去核时知道改动在哪。
                if let Some(worktree) = &task.worktree_state {
                    body["worktree_path"] = json!(worktree.root.to_string_lossy());
                }
                let request_id = format!("task-report:{}:{}", task.id, report.id);
                Ok(Some((task, body, request_id)))
            }
            EffectPayload::Stop { attempt_id, .. } => {
                let summary = format!(
                    "用户立即停了任务 {} 的第 {} 次 Attempt。",
                    task.id, attempt_id
                );
                let body = json!({
                    "kind": "task_report",
                    "task": {
                        "id": task.id.as_str(),
                        "title": task.title,
                        "stage": stage_key(stage),
                    },
                    "origin": "executor",
                    "state": "stopped",
                    "summary": summary,
                });
                let request_id = format!("task-stop:{}:{}", task.id, attempt_id);
                Ok(Some((task, body, request_id)))
            }
            EffectPayload::Launch { .. }
            | EffectPayload::Delivery { .. }
            | EffectPayload::Notice { .. }
            | EffectPayload::Restart { .. } => Err("relay 载荷不是 relay".to_owned()),
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
        // 上一次已经开成会话：重跑的效果只把行收掉；被重启打断的 Attempt 留给第 4 张票。
        if task
            .latest_attempt()
            .is_some_and(|attempt| attempt.conversation.is_some())
        {
            self.finish_or_fail(&claim, Ok(()));
            return false;
        }
        // 崩溃留下的、没有会话 id 的 Attempt 先收尾，再开新的。
        if let Err(error) = self.store.close_unopened_attempts(&task.id, Self::now()) {
            self.finish_or_fail(&claim, Err(error.to_string()));
            return false;
        }
        if let Ok(Some(reloaded)) = self.store.task(&task.id) {
            task = reloaded;
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
        let attempt_id = match self.store.begin_attempt(&task.id, Self::now()) {
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
            },
        );
        self.monitor(task.id.clone(), attempt_id, conversation.clone(), events);
        let mut assignment = json!({
            "task_id": task.id.as_str(),
            "title": task.title,
            "requirement": task.requirement,
            "dod": task.dod,
            "cwd": task.effective_cwd().to_string_lossy(),
        });
        // 写任务的模型要知道自己能在哪个路径里改；开了工作树时这里写换算后的范围。
        if let Some(scope) = task.effective_write_scope() {
            assignment["write_scope"] = json!(scope.to_string_lossy());
        }
        let assignment = assignment.to_string();
        let sent = host
            .send_message(
                &format!("assignment:{}", task.id),
                UserMessage::text(assignment),
                DeliveryMode::FollowUp,
            )
            .await;
        if let Err(error) = sent {
            self.running
                .lock()
                .expect("thinagent 表锁被毒化")
                .remove(conversation.as_str());
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
        // 工具名单跟着任务的写入范围和 bash 开关走：只读 9 件，写任务 11 件，
        // 开了 bash 的写任务 15 件。
        // T028.4 加 task_handback 之前受理的只读任务冻结的是 8 件；升级后要能接着做，
        // 不因为少这一件新加的工具就判任务包缺项。
        let (expected, legacy): (BTreeSet<&str>, Option<BTreeSet<&str>>) =
            match (task.write_scope.is_some(), task.bash) {
                (true, true) => (THIN_WRITE_BASH_TOOLS.into_iter().collect(), None),
                (true, false) => (THIN_WRITE_TOOLS.into_iter().collect(), None),
                (false, _) => {
                    let legacy = THIN_TOOLS
                        .into_iter()
                        .filter(|name| *name != "task_handback")
                        .collect();
                    (THIN_TOOLS.into_iter().collect(), Some(legacy))
                }
            };
        let actual: BTreeSet<&str> = task.tools.iter().map(String::as_str).collect();
        if actual != expected && legacy.as_ref() != Some(&actual) {
            missing.push(missing_item("工具名单（和写入范围不符）"));
        }
        if task.model.connection.trim().is_empty()
            || task.model.model.trim().is_empty()
            || task.model.effort_wanted.trim().is_empty()
        {
            missing.push(missing_item("模型选择"));
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
        let mut agent_config = self.options.config.clone();
        agent_config.tool_keys = crate::root::tool_key_source(&self.options);
        agent_config.write_scope = task.effective_write_scope();
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
                no_wait: false,
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
            })
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
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    /// turn 结束但还没上报：照 DSH 合成 waiting；有最后一段回复就带它，没有就提示去问。
    async fn turn_ended(&self, task_id: &TaskId, attempt_id: AttemptId, conversation: &SessionId) {
        let Ok(Some(task)) = self.store.task(task_id) else {
            return;
        };
        if task.latest_attempt().map(|attempt| attempt.id) != Some(attempt_id) {
            return;
        }
        // 轮次结束就释放席位；终态报告/停止也走这里，释放幂等。
        self.release_seat(task_id.as_str());
        // 停止已经把这个 Attempt 的结果定死：取消后的收尾不再合成 waiting。
        if task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_some())
        {
            return;
        }
        if task.latest_report().is_some_and(|report| {
            matches!(
                report.state,
                ReportState::Ready | ReportState::Blocked | ReportState::Failed
            )
        }) {
            return;
        }
        // 用户自己停下这一轮：不算 ThinAgent 没上报，不合成 waiting，Attempt 留着接着做。
        if self.last_turn_was_user_aborted(conversation) {
            return;
        }
        // 收尾停：turn 结束还没报，先走 T024 的 waiting 合成，再把这个 Attempt 记成 stopped。
        if self.after_report_requested(conversation) {
            self.settle_after_report(task_id, attempt_id, conversation);
            return;
        }
        self.record_waiting(task_id, attempt_id, conversation);
        self.wake();
    }

    /// 这个会话最后一轮是不是用户取消的（停下这一轮；立即停会先把 Attempt 记成结束，走不到这里）。
    fn last_turn_was_user_aborted(&self, conversation: &SessionId) -> bool {
        let Ok(store) = session::SessionStore::open_read_only(&self.data_dir, conversation) else {
            return false;
        };
        store
            .events()
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                session::EventKind::TurnEnd { reason, .. } => Some(matches!(
                    reason,
                    session::TurnEndReason::Aborted {
                        reason: session::AbortCause::User
                    }
                )),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// 这个会话有没有收尾停标记（在跑表里查；会话摘掉之后就是 false）。
    fn after_report_requested(&self, conversation: &SessionId) -> bool {
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .get(conversation.as_str())
            .is_some_and(|entry| entry.host.agent().stop_after_report_requested())
    }

    /// turn 结束还没报：照 T024 合成 waiting（有最后一段回复就带它，没有就提示去问）。
    fn record_waiting(&self, task_id: &TaskId, attempt_id: AttemptId, conversation: &SessionId) {
        let said = self.last_assistant_text(conversation);
        let summary = if said.trim().is_empty() {
            WAITING_FALLBACK.to_owned()
        } else {
            said
        };
        if let Err(error) = self.store.record_report(
            task_id,
            attempt_id,
            NewReport {
                state: ReportState::Waiting,
                origin: ReportOrigin::Executor,
                summary,
                question: None,
            },
            Self::now(),
        ) {
            eprintln!("任务 {task_id} 的等待报告没写进去：{error}");
        }
    }

    /// 收尾停的收尾：waiting 合成之后把这个 Attempt 记成 stopped，并从在跑表摘掉会话。
    ///
    /// 幂等：Attempt 已经结束、已经有终态报告、或已经不是最新 Attempt 时什么都不做。
    fn settle_after_report(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &SessionId,
    ) {
        let Ok(Some(task)) = self.store.task(task_id) else {
            return;
        };
        if task.latest_attempt().map(|attempt| attempt.id) != Some(attempt_id) {
            return;
        }
        if task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_some())
        {
            return;
        }
        if task.latest_report().is_some_and(|report| {
            matches!(
                report.state,
                ReportState::Ready | ReportState::Blocked | ReportState::Failed
            )
        }) {
            return;
        }
        self.record_waiting(task_id, attempt_id, conversation);
        if let Err(error) =
            self.store
                .end_attempt(task_id, attempt_id, Self::now(), AttemptEnd::Stopped)
        {
            eprintln!("任务 {task_id} 的收尾停没记进任务库：{error}");
            return;
        }
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .remove(conversation.as_str());
        self.release_seat(task_id.as_str());
        self.wake();
    }

    /// ThinAgent 会话里最后一段助手文字（turn 结束时的最后回复）。
    fn last_assistant_text(&self, conversation: &SessionId) -> String {
        let Ok(store) = session::SessionStore::open_read_only(&self.data_dir, conversation) else {
            return String::new();
        };
        store
            .events()
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                session::EventKind::AssistantMessage { message, .. } => Some(
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                ),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// ThinAgent 报了一份报告：同事务写报告 + 排 relay；终态就是这次 Attempt 的结束，关掉会话。
    fn report(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &str,
        report: NewReport,
    ) -> Result<(ReportId, bool), TaskError> {
        let state = report.state;
        let duplicate = self
            .store
            .task(task_id)?
            .and_then(|task| task.latest_report().map(|latest| latest.state))
            == Some(state);
        let id = self
            .store
            .record_report(task_id, attempt_id, report, Self::now())?;
        if matches!(
            state,
            ReportState::Ready | ReportState::Blocked | ReportState::Failed
        ) {
            // 照 spec：报了以后 Attempt 以 reported 结束，卡照报的状态出。报告已经落库，
            // 结束原因写失败不该把它变成工具错误（报过的结果不能被重报）。
            best_effort(
                "task_report 的 Attempt 没收尾",
                self.store
                    .end_attempt(task_id, attempt_id, Self::now(), AttemptEnd::Reported),
            );
            self.running
                .lock()
                .expect("thinagent 表锁被毒化")
                .remove(conversation);
            self.release_seat(task_id.as_str());
        }
        self.wake();
        Ok((id, duplicate))
    }

    /// ThinAgent 交回：同事务写一份 blocked 报告（问题由程序写死）并排 relay；
    /// 这个 Attempt 以 reported 结束。Kind 收到 relay 以后怎么问，由 Kind 自己说。
    fn handback(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &str,
        reason: String,
    ) -> Result<String, ToolFailure> {
        let report = NewReport {
            state: ReportState::Blocked,
            origin: ReportOrigin::Agent,
            summary: reason,
            question: Some(HANDBACK_QUESTION.to_owned()),
        };
        let (id, created) = self
            .store
            .record_handback_report(task_id, attempt_id, report, Self::now())
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        // 同一个 Attempt 只交回一次：已有 blocked 报告（含并行的第一次交回）就是「已交回」。
        if !created {
            return Ok("已交回".to_owned());
        }
        // 和 task_report 一样：报告落库后结束这个 Attempt；结束写失败不改写工具回执。
        best_effort(
            "交回的 Attempt 没收尾",
            self.store
                .end_attempt(task_id, attempt_id, Self::now(), AttemptEnd::Reported),
        );
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .remove(conversation);
        self.release_seat(task_id.as_str());
        self.wake();
        Ok(format!(
            "已交回 Kind（报告 {id}）。Kind 会问用户怎么办；不要再自己做这件事。"
        ))
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
            requirement,
            dod,
            cwd,
            write_scope,
            worktree,
        } = args;
        let conversation = context
            .session_id()
            .ok_or_else(|| ToolFailure::plain("没有会话 id，不能受理任务"))?
            .to_owned();
        let cwd = match cwd {
            Some(raw) => {
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
                std::fs::canonicalize(&path).unwrap_or(path)
            }
            None => context.cwd.clone(),
        };
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
        };
        let policy = TaskPolicy {
            write_scope,
            worktree,
            bash,
        };
        let key = format!("{conversation}:{call_id}");
        let (task, created) = self
            .store
            .create_task_once_with(draft, policy, &key, Self::now())
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        self.wake();
        Ok((task, created))
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
        let note = format!(
            "AIOS：用户在 Kind 对话里同意把写入范围改成 {}（要求第 {version} 版）。先看当前的文件再继续；只在这个范围里写，范围外的调 task_handback。",
            note_scope.display()
        );
        let request_id = format!("task-update:{id}:{}", opened.attempt_id);
        let sent = opened
            .host
            .send_message(&request_id, UserMessage::text(note), DeliveryMode::Steer)
            .await;
        if let Err(error) = sent {
            // 说明没送出去：别留一个没人做的空 Attempt。
            opened.host.shutdown().await;
            self.running
                .lock()
                .expect("thinagent 表锁被毒化")
                .remove(opened.host.agent().id().as_str());
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

    /// Kind 的 task_status：阶段、最近报告全文、Attempt 数；waiting 时向 ThinAgent 要一次。
    async fn status_text(&self, task_id: &str) -> Result<String, ToolFailure> {
        let id = TaskId::from(task_id);
        let task = self
            .store
            .task(&id)
            .map_err(|error| ToolFailure::plain(error.to_string()))?
            .ok_or_else(|| ToolFailure::plain(format!("任务不存在：{task_id}")))?;
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
            return Ok(not_running());
        }
        let Some(running) = self.running_for(&id, attempt.id) else {
            return Ok(not_running());
        };
        // 只取消正在跑的一轮：cancel_with 不动输入队列，也不写 Attempt 结果。
        if !running.host.agent().is_running() {
            return Ok(not_running());
        }
        running.host.agent().cancel_with(session::AbortCause::User);
        Ok(TaskInterruptResponse {
            interrupted: true,
            message: "已停下这一轮".to_owned(),
        })
    }

    /// 停一个任务：立即停走「先持久、再取消」；收尾停走「插固定说明、打标记、等报」。
    ///
    /// 页面（Call::TaskStop）和 Kind 的 task_stop 都调到这一个方法。
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
            return Ok(not_running());
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
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .remove(running.host.agent().id().as_str());
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

    /// task_report 的实际落点：写报告 + 排 relay（store 同一事务），返回回执。
    fn report_from_tool(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &str,
        report: NewReport,
    ) -> Result<String, ToolFailure> {
        let state = report.state;
        let (id, duplicate) = self
            .report(task_id, attempt_id, conversation, report)
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        if duplicate {
            Ok(format!(
                "这个 Attempt 已经报过 {state:?}（报告 {id}）；没有重复上报。"
            ))
        } else {
            Ok(format!(
                "已上报 {state:?}（报告 {id}）。结果会作为 task_report 消息送到 Kind。"
            ))
        }
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
    }
}

fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::NeedsConditions => "待补足条件",
        Stage::Pending => "待执行",
        Stage::Running => "执行中",
        Stage::ReadyForReview => "待检查",
    }
}

fn report_origin_key(origin: ReportOrigin) -> &'static str {
    match origin {
        ReportOrigin::Package => "package",
        ReportOrigin::Agent => "agent",
        ReportOrigin::Executor => "executor",
    }
}

fn report_state_key(state: ReportState) -> &'static str {
    match state {
        ReportState::Ready => "ready",
        ReportState::Blocked => "blocked",
        ReportState::Failed => "failed",
        ReportState::Waiting => "waiting",
    }
}

/// relay 摘要最多 600 字；更长时带原始字数（照 DSH resultRelay）。
fn relay_summary(text: &str) -> (String, Option<usize>) {
    let chars = text.chars().count();
    if chars > 600 {
        (text.chars().take(600).collect(), Some(chars))
    } else {
        (text.to_owned(), None)
    }
}

/// task_accept 的 title 参数说明（T048）：卡片标题的硬上限写进 schema，模型生成时就被约束。
pub(crate) const TASK_ACCEPT_TITLE_DESCRIPTION: &str = "Short label for the card title: at most 20 characters (Chinese characters count as one); this is the card title; longer titles are truncated.";

/// Kind 的 task_accept。
pub(crate) struct TaskAcceptTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskAcceptTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": TASK_ACCEPT_TITLE_DESCRIPTION},
                    "requirement": {"type": "string", "description": "What the task must do, in the user's words."},
                    "dod": {"type": "string", "description": "How the result will be judged complete."},
                    "cwd": {"type": "string", "description": "Directory to work in; omit to use this conversation's directory. It must already exist."},
                    "write_scope": {"type": "string", "description": "One path this task may write: a file, or a directory and everything under it. Relative paths resolve against cwd. Omit for a read-only investigation."},
                    "worktree": {"type": "boolean", "description": "Run the task in a new git worktree under the data directory. Only when the user explicitly asks for a worktree. Defaults to false."}
                },
                "required": ["title", "requirement", "dod"]
            }),
        }
    }
}

impl Tool for TaskAcceptTool {
    fn name(&self) -> &str {
        "task_accept"
    }

    fn description(&self) -> &str {
        "Dispatch a task to a ThinAgent and return its task id. Omit write_scope for a read-only investigation; pass one path for a writing task (a file, or a directory and everything under it). Pass worktree=true only when the user explicitly asked to work in a git worktree. The result arrives later as a task_report message; the user already sees a card, so answer only what is needed now and do not wait for the result."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let Some(title) = string_arg(&args, "title") else {
                return ToolResult::error(call.id, ToolFailure::plain("title 不能为空"));
            };
            let Some(requirement) = string_arg(&args, "requirement") else {
                return ToolResult::error(call.id, ToolFailure::plain("requirement 不能为空"));
            };
            let Some(dod) = string_arg(&args, "dod") else {
                return ToolResult::error(call.id, ToolFailure::plain("dod 不能为空"));
            };
            let cwd = args
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.trim().is_empty())
                .map(str::to_owned);
            let write_scope = match args.get("write_scope") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) => {
                    Some(value.clone()).filter(|value| !value.trim().is_empty())
                }
                Some(_) => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain("write_scope 必须是字符串"),
                    );
                }
            };
            let worktree = match args.get("worktree") {
                None => false,
                Some(Value::Bool(value)) => *value,
                Some(_) => {
                    return ToolResult::error(call.id, ToolFailure::plain("worktree 必须是布尔值"));
                }
            };
            match self.runtime.accept(
                context,
                &call.id,
                TaskAcceptArgs {
                    title,
                    requirement,
                    dod,
                    cwd,
                    write_scope,
                    worktree,
                },
            ) {
                Ok((task, true)) => ToolResult::success(
                    call.id,
                    format!(
                        "已派出任务 {}。结果会作为 task_report 消息送来；用户已经看到卡片，只按需回答。",
                        task.id
                    ),
                ),
                Ok((task, false)) => ToolResult::success(
                    call.id,
                    format!(
                        "已受理过（Already accepted as {}）；没有重复派出。结果会作为 task_report 消息送来。",
                        task.id
                    ),
                ),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// Kind 的 task_status。
pub(crate) struct TaskStatusTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskStatusTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id returned by task_accept."}
                },
                "required": ["task_id"]
            }),
        }
    }
}

impl Tool for TaskStatusTool {
    fn name(&self) -> &str {
        "task_status"
    }

    fn description(&self) -> &str {
        "Read a task's stage, its latest report in full, and its Attempt count. While the task is waiting for a report, this also asks its ThinAgent once for a status report."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let Some(task_id) = args.get("task_id").and_then(Value::as_str) else {
                return ToolResult::error(call.id, ToolFailure::plain("task_id 不能为空"));
            };
            match self.runtime.status_text(task_id).await {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// Kind 的 task_stop：页面 TaskStop 调的是同一个 stop_task。
pub(crate) struct TaskStopTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskStopTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id returned by task_accept."},
                    "mode": {
                        "type": "string",
                        "enum": ["now", "after_report"],
                        "description": "now stops the running Attempt immediately. after_report lets it finish the current step, then only task_report is allowed until it reports and ends."
                    }
                },
                "required": ["task_id", "mode"]
            }),
        }
    }
}

impl Tool for TaskStopTool {
    fn name(&self) -> &str {
        "task_stop"
    }

    fn description(&self) -> &str {
        "Stop a running task. mode=now cancels its current Attempt immediately. mode=after_report lets it finish the current step and report once, then it ends; from then on only task_report is allowed. Call it only when the user explicitly asks to stop a task."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let Some(task_id) = args.get("task_id").and_then(Value::as_str) else {
                return ToolResult::error(call.id, ToolFailure::plain("task_id 不能为空"));
            };
            let mode = match args.get("mode").and_then(Value::as_str) {
                Some("now") => StopMode::Now,
                Some("after_report") => StopMode::AfterReport,
                _ => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain("mode 必须是 now 或 after_report"),
                    );
                }
            };
            match self.runtime.stop_task(task_id, mode).await {
                Ok(result) => ToolResult::success(call.id, result.message),
                Err(error) => ToolResult::error(call.id, ToolFailure::plain(error.to_string())),
            }
        })
    }
}

/// Kind 的 task_update：用户同意后换写入范围，出新要求版本并接回原任务会话。
pub(crate) struct TaskUpdateTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskUpdateTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id returned by task_accept."},
                    "write_scope": {"type": "string", "description": "One path the task may now write: a file, or a directory and everything under it. Relative paths resolve against the task's cwd."}
                },
                "required": ["task_id", "write_scope"]
            }),
        }
    }
}

impl Tool for TaskUpdateTool {
    fn name(&self) -> &str {
        "task_update"
    }

    fn description(&self) -> &str {
        "Change a writing task's write scope after the user agreed in this Kind conversation. Records a new requirement version and resumes the task in its own conversation with the new scope. Only the write scope can change; anything else needs a new task."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let Some(task_id) = args.get("task_id").and_then(Value::as_str) else {
                return ToolResult::error(call.id, ToolFailure::plain("task_id 不能为空"));
            };
            let Some(write_scope) = args
                .get("write_scope")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|scope| !scope.is_empty())
                .map(str::to_owned)
            else {
                return ToolResult::error(call.id, ToolFailure::plain("write_scope 不能为空"));
            };
            match self.runtime.update_scope(task_id, write_scope).await {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// ThinAgent 的 task_handback：把超出任务范围、属于别的任务或要改要求/DoD 的话交回 Kind。
///
/// 只 ThinAgent 装；Kind 的工具表里没有它。
struct TaskHandbackTool {
    runtime: Arc<TaskRuntime>,
    task_id: TaskId,
    attempt_id: AttemptId,
    parameters: Value,
}

impl TaskHandbackTool {
    fn new(runtime: Arc<TaskRuntime>, task_id: TaskId, attempt_id: AttemptId) -> Self {
        Self {
            runtime,
            task_id,
            attempt_id,
            parameters: json!({
                "type": "object",
                "properties": {
                    "reason": {"type": "string", "maxLength": 300, "description": "One sentence saying what the user asked for that is outside this task's scope (at most 300 characters)."}
                },
                "required": ["reason"]
            }),
        }
    }
}

impl Tool for TaskHandbackTool {
    fn name(&self) -> &str {
        "task_handback"
    }

    fn description(&self) -> &str {
        "Hand this task back to Kind when what the user asks is outside the task's scope, belongs to another task, or asks to change the requirement or the DoD. Give one sentence saying what exceeded the task. Do not do that work yourself; the user decides with Kind what happens next."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let Some(reason) = args
                .get("reason")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .map(str::to_owned)
            else {
                return ToolResult::error(call.id, ToolFailure::plain("reason 不能为空"));
            };
            if reason.chars().count() > 300 {
                return ToolResult::error(call.id, ToolFailure::plain("reason 最多 300 字"));
            }
            let conversation = context.session_id().unwrap_or_default().to_owned();
            match self
                .runtime
                .handback(&self.task_id, self.attempt_id, &conversation, reason)
            {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// ThinAgent 的 task_report：一次调用写一条报告并排一条 relay。
struct TaskReportTool {
    runtime: Arc<TaskRuntime>,
    task_id: TaskId,
    attempt_id: AttemptId,
    parameters: Value,
}

impl TaskReportTool {
    fn new(runtime: Arc<TaskRuntime>, task_id: TaskId, attempt_id: AttemptId) -> Self {
        Self {
            runtime,
            task_id,
            attempt_id,
            parameters: json!({
                "type": "object",
                "properties": {
                    "state": {"type": "string", "enum": ["ready", "blocked", "failed"], "description": "ready: the DoD is met; blocked: cannot continue without the user or a missing condition; failed: the work cannot produce the result."},
                    "summary": {"type": "string", "description": "The answer or the reason, with the evidence it rests on."},
                    "question": {"type": "string", "description": "One concrete question for the user, only with state=blocked and only when the user must answer it (at most 1000 characters)."}
                },
                "required": ["state", "summary"]
            }),
        }
    }
}

impl Tool for TaskReportTool {
    fn name(&self) -> &str {
        "task_report"
    }

    fn description(&self) -> &str {
        "Report the task's final state once: ready with the result, blocked with what is missing and what the user must do, or failed with what failed. Do not report progress. The user sees it as a card and the Kind conversation receives it as a message."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let state = match args.get("state").and_then(Value::as_str) {
                Some("ready") => ReportState::Ready,
                Some("blocked") => ReportState::Blocked,
                Some("failed") => ReportState::Failed,
                _ => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain("state 必须是 ready、blocked 或 failed"),
                    );
                }
            };
            let Some(summary) = args
                .get("summary")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                return ToolResult::error(call.id, ToolFailure::plain("summary 不能为空"));
            };
            let question = args
                .get("question")
                .and_then(Value::as_str)
                .filter(|question| !question.trim().is_empty())
                .map(str::to_owned);
            let conversation = context.session_id().unwrap_or_default().to_owned();
            let report = NewReport {
                state,
                origin: ReportOrigin::Agent,
                summary,
                question,
            };
            match self.runtime.report_from_tool(
                &self.task_id,
                self.attempt_id,
                &conversation,
                report,
            ) {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn parse_object(arguments: &str) -> Result<serde_json::Map<String, Value>, ToolFailure> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| ToolFailure::plain("arguments must be a JSON object"))
}

fn string_arg(args: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

/// T048：task_accept 的 title 说明必须明确给出卡片标题字数上限（生成时就约束，UI 截断只是兜底）。
#[cfg(test)]
mod t048_title_limit_tests {
    use super::TASK_ACCEPT_TITLE_DESCRIPTION;

    #[test]
    fn task_accept_title_description_states_the_card_title_limit() {
        assert!(
            TASK_ACCEPT_TITLE_DESCRIPTION.contains("at most 20 characters"),
            "title 说明要写明确上限 20 characters：{TASK_ACCEPT_TITLE_DESCRIPTION}"
        );
        assert!(
            TASK_ACCEPT_TITLE_DESCRIPTION.contains("Chinese characters count as one"),
            "中文按一字计：{TASK_ACCEPT_TITLE_DESCRIPTION}"
        );
        assert!(
            TASK_ACCEPT_TITLE_DESCRIPTION.contains("card title"),
            "说明这是卡片标题：{TASK_ACCEPT_TITLE_DESCRIPTION}"
        );
        assert!(
            TASK_ACCEPT_TITLE_DESCRIPTION.contains("truncated"),
            "超长会被截断：{TASK_ACCEPT_TITLE_DESCRIPTION}"
        );
    }
}

/// T049：requirement/DoD 指名的具体工具/命令/做法属于任务范围；缺工具（如 bash 关着）要在第一步就报
/// blocked，不许用别的工具替代或模拟它的输出，也不许替代完成后才在 ready 报告里附一句说明。
#[cfg(test)]
mod t049_missing_tool_policy_tests {
    use super::thin_policy;

    fn assert_missing_tool_rule(policy: &str) {
        for needle in [
            "names a specific tool, command, or approach",
            "report blocked on the first step",
            "do not substitute another tool",
            "do not simulate its output",
            "do not wait until the ready report",
        ] {
            assert!(
                policy.contains(needle),
                "thin_policy 缺关键句「{needle}」：{policy}"
            );
        }
    }

    #[test]
    fn bash_off_policy_makes_a_missing_tool_a_first_step_blocked() {
        let policy = thin_policy(false);
        assert!(
            policy.contains("You cannot run shell commands"),
            "bash 关着的事实还在"
        );
        assert_missing_tool_rule(&policy);
    }

    #[test]
    fn bash_on_policy_states_the_same_rule_for_other_missing_tools() {
        assert_missing_tool_rule(&thin_policy(true));
    }
}
