//! Kind 派发、ThinAgent 执行、上报 relay 的后端闭环（假模型、两个真会话）。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::AgentConfig;
use model::DEFAULT_CONTEXT_WINDOW;
use serde_json::Value;
use service::{
    AttachmentPutRequest, Call, CardActRequest, CardVerb, DeliveryMode, DisabledModelsSetRequest,
    GateJudgeConclusion, GateJudgeRequest, Response, RootHost, SendAttachment, SendRequest,
    SessionModelSelectRequest, SettingsUpdateRequest, StopMode, TaskInterruptRequest,
    TaskReadRequest, TaskSayRequest, TaskStopRequest,
};
use tasks::{
    AttemptEnd, ConversationId, EffectKind, EffectState, ModelChoice, NewReport, NewTask,
    ReportOrigin, ReportState, Stage, Store, Task,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

mod dispatch;
mod events;
mod fake_model;
mod fixture;
mod store_helpers;

use dispatch::*;
use events::*;
use fake_model::*;
use fixture::*;
use store_helpers::*;

mod accept_tests;
mod background_turn_tests;
mod cancel_entry_tests;
mod dialog_tests;
mod misc_tests;
mod pause_tests;
mod project_tests;
mod report_tests;
mod restart_tests;
mod retained_background_tests;
mod stop_tests;
mod trigger_tests;
mod workflow_tests;

struct Fixture {
    _server: MockServer,
    data: tempfile::TempDir,
    cwd: tempfile::TempDir,
    router: Router,
    host: RootHost,
}

impl Fixture {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let router = Router::new();
        mount(&server, &router).await;
        let data = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let host = open_host(&server, data.path(), cwd.path()).await;
        Self {
            _server: server,
            data,
            cwd,
            router,
            host,
        }
    }

    /// 确认卡场景要产品动作登记表 + 自带 skills（propose_action 才有 update_task_requirement）。
    async fn new_with_actions() -> Self {
        let server = MockServer::start().await;
        let router = Router::new();
        mount(&server, &router).await;
        let data = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let host = open_graph_host(&server, data.path(), cwd.path()).await;
        Self {
            _server: server,
            data,
            cwd,
            router,
            host,
        }
    }

    fn data(&self) -> &Path {
        self.data.path()
    }

    /// 让 Kind 受理一个任务，返回任务；pause 时 launch 先不跑。
    async fn accept(&self, pause: bool) -> Task {
        self.accept_with_kind_replies(pause, 0).await
    }

    /// 同上；受理这一轮的回复后面再排几段 Kind 的备用回复。
    ///
    /// 备用回复必须在派发之前进队列：受理之后 launch 立刻开跑，ThinAgent 的 waiting relay
    /// 可能赶在测试推队列之前 Steer 进 Kind，假模型就会收到没有脚本的请求。
    async fn accept_with_kind_replies(&self, pause: bool, spares: usize) -> Task {
        let args = serde_json::json!({
            "title": "查一下",
            "requirement": "找答案",
            "dod": "有答案",
            "cwd": self.cwd.path().to_string_lossy()
        });
        self.accept_args(pause, args, spares).await
    }

    /// 用给定的 task_accept 参数受理一个任务（写任务带 write_scope）。
    async fn accept_args(&self, pause: bool, mut args: serde_json::Value, spares: usize) -> Task {
        if args.get("quote").is_none() {
            args["quote"] = serde_json::json!("改一下");
        }
        if pause {
            self.host
                .task_runtime()
                .expect("runtime")
                .set_pump_paused(true);
        }
        self.router.push_kind(crate::common::tool_use_sse(
            0,
            "call-accept",
            "task_accept",
            &args.to_string(),
        ));
        self.router.push_kind(crate::common::text_sse("已派出"));
        for _ in 0..spares {
            self.router.push_kind(crate::common::text_sse("收到"));
        }
        self.host
            .dispatch(Call::Send(send(&self.host, "kind-1", "改一下")))
            .await
            .expect("发送");
        crate::common::wait_turns(&self.host, self.data(), 1).await;
        let store = tasks_store(self.data());
        one_task(&store)
    }

    /// 受理一个写任务；write_scope 是相对任务 cwd 的一个路径。
    async fn accept_write(&self, pause: bool, write_scope: &str) -> Task {
        let args = serde_json::json!({
            "title": "改一下判重",
            "requirement": "把重复判断改成按 url",
            "dod": "同一个 url 只留一条",
            "write_scope": write_scope
        });
        self.accept_args(pause, args, 0).await
    }
}

/// spec v1 的恢复说明原文：确认接续后第一条输入就是它。
const RECOVERY_NOTE: &str = "AIOS 在这个任务进行中重启了。先看当前的文件和状态再继续；已经做完的外部动作不要再做一遍。把原来的任务做完，然后用 task_report 上报。";

/// 写任务工具表里和 bash 有关的四件（D85：角色开关打开才给）。
const BASH_JOB_TOOLS: [&str; 4] = ["bash", "job_output", "job_list", "job_kill"];

/// 只读 9 件：开关打开也不能给只读任务加。
const READ_ONLY_TOOLS: [&str; 11] = [
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

/// 写任务 13 件（关着 bash）。
const WRITE_TOOLS: [&str; 13] = [
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

/// 写任务 17 件（开着 bash：13 件加 bash 和后台任务三件）。
const WRITE_BASH_TOOLS: [&str; 17] = [
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

fn sorted(names: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
    let mut names: Vec<String> = names.into_iter().map(Into::into).collect();
    names.sort();
    names
}

/// 存一个假的 Exa key：加载 web skill 后两件联网工具才开得出来，工具表才数得准（不联网）。
fn save_exa_key(data: &Path) {
    credentials::Keyring::open(data, Some(&data.join("identity.txt")))
        .expect("keyring")
        .save_exa_key("test-exa-key")
        .expect("存 Exa key");
}

async fn set_bash(host: &RootHost, bash: bool) {
    host.dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
        bash: Some(bash),
        ..Default::default()
    }))
    .await
    .expect("改 bash 开关");
}

fn write_task_args() -> Value {
    serde_json::json!({
        "title": "改一下判重",
        "quote": "改一下",
        "requirement": "把重复判断改成按 url",
        "dod": "同一个 url 只留一条",
        "write_scope": "target.py"
    })
}

/// 请求里加载 web skill 之后的那一份（工具表已经开出条件工具）。
fn loaded_request(router: &Router) -> String {
    router
        .bodies(true)
        .into_iter()
        .find(|body| body.contains("\"name\":\"web_search\""))
        .expect("加载 web skill 后的请求")
}

/// 在临时目录里建一个带一次提交的 git 仓库：sub/app.py 和 root.py 都已跟踪。
fn init_git_repo(root: &Path) {
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("app.py"), "old\n").unwrap();
    std::fs::write(root.join("root.py"), "root\n").unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=AIOS Test",
            "-c",
            "user.email=aios@example.test",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
}

/// 在仓库里跑一条 git；测试的提交配置和用户全局配置隔开（不读用户的签名设置）。
fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        // 关掉 auto-gc：它会自己起后台进程，测试结束时可能还没退，被当成泄漏的进程。
        .args(["-c", "gc.auto=0"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("跑 git");
    assert!(
        output.status.success(),
        "git {args:?} 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// 仓库里登记的工作树路径（canonical）。
fn listed_worktrees(repo: &Path) -> Vec<std::path::PathBuf> {
    git(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path)))
        .collect()
}

/// ThinAgent 收到的 assignment（第一条 user 消息就是 JSON）。
fn assignment(events: &[session::Event]) -> Value {
    events
        .iter()
        .find_map(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return None;
            };
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            let value: Value = serde_json::from_str(&text).ok()?;
            value["task_id"].is_string().then_some(value)
        })
        .expect("ThinAgent 的 assignment")
}

/// 请求体系统提示里的 system 正文。
fn request_system(body: &str) -> String {
    let value: Value = serde_json::from_str(body).expect("请求体 JSON");
    value["system"].as_str().expect("system").to_owned()
}

/// 请求体系统提示里的工作目录（身份第一句按角色，不再固定 coding agent）。
fn request_working_directory(body: &str) -> String {
    let value: Value = serde_json::from_str(body).expect("请求体 JSON");
    let system = value["system"].as_str().expect("system");
    let (_, tail) = system
        .split_once("Your working directory is ")
        .expect("system 里的工作目录");
    tail.split_whitespace()
        .next()
        .expect("工作目录")
        .trim_end_matches('.')
        .to_owned()
}

/// 受理前就能按标题排队 ThinAgent 回复的写任务参数：cwd 用仓库里的 sub，
/// 写入范围是 sub/app.py，开工作树。
fn worktree_task_args() -> Value {
    serde_json::json!({
        "title": "改一下判重",
        "quote": "改一下",
        "requirement": "把重复判断改成按 url",
        "dod": "同一个 url 只留一条",
        "cwd": "sub",
        "write_scope": "app.py",
        "worktree": true
    })
}

/// 任务开出来的工作树根：测试先断言它真的开出来了，再拿 canonical 路径。
fn worktree_root(fixture: &Fixture, task: &Task) -> std::path::PathBuf {
    let root = fixture.data().join("worktrees").join(task.id.as_str());
    assert!(root.is_dir(), "工作树目录没开出来：{}", root.display());
    std::fs::canonicalize(&root).expect("canonicalize 工作树根")
}

/// 工作树里和原 cwd（仓库的 sub）对应的子目录。
fn worktree_cwd(fixture: &Fixture, task: &Task) -> std::path::PathBuf {
    let cwd = worktree_root(fixture, task).join("sub");
    assert!(cwd.is_dir(), "工作树里没有对应的 sub：{}", cwd.display());
    std::fs::canonicalize(&cwd).expect("canonicalize 工作树 cwd")
}

/// 写任务按标题先排好 ThinAgent 回复，暂停受理后开闸；返回任务。
async fn run_write_task(fixture: &Fixture, args: Value, scripts: Vec<String>) -> Task {
    let title = args["title"].as_str().expect("标题").to_owned();
    for script in scripts {
        fixture.router.push_thin_for_title(&title, script);
    }
    let task = fixture.accept_args(true, args, 1).await;
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    task
}

/// 写入范围是任务 cwd 的写任务参数（清单可以有多个文件）。
fn scoped_write_args() -> Value {
    serde_json::json!({
        "title": "改一下判重",
        "requirement": "把重复判断改成按 url",
        "dod": "同一个 url 只留一条",
        "write_scope": "."
    })
}

/// 点结果卡上的「接受」（卡 id 是 relay 的 request id）。
async fn accept_result_card(host: &RootHost, call_id: &str) -> Value {
    let response = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: call_id.to_owned(),
            verb: CardVerb::Confirm,
            option: None,

            note: None,
            report_id: None,
        }))
        .await
        .expect("接受结果卡");
    serde_json::to_value(response).expect("接受结果卡 json")["decision"].clone()
}

fn card_decisions(events: &[session::Event], call_id: &str) -> Vec<session::CardDecision> {
    events
        .iter()
        .filter_map(|event| session::CardDecision::from_event(&event.kind))
        .filter(|decision| decision.call_id == call_id)
        .collect()
}

/// 打开带确认动作登记表的 host（确认卡上的动作要真的登记过）。
async fn open_graph_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
    let actions = Arc::new(service::ActionRegistry::settings(data.to_path_buf(), None));
    open_host_with(server, data, cwd, move |options| {
        options.actions = actions;
        // 图流程要真的加载 aios-tasks skill（propose_action 的动作开出条件）。
        options.config.bundled_skills = tools::bundled_skills().to_vec();
    })
    .await
}

fn graph_root(store: &Store) -> Task {
    let ids = store.task_ids().expect("任务 id");
    let mut roots: Vec<Task> = ids
        .into_iter()
        .filter_map(|id| store.task(&id).expect("读任务"))
        .filter(Task::is_graph_root)
        .collect();
    assert_eq!(roots.len(), 1, "只应有一个总任务：{roots:?}");
    roots.pop().expect("总任务")
}

/// 从根日志里取任务图的确认卡（动态 Kind 轮出的那张）。
fn graph_card(data: &Path) -> Value {
    let events = crate::common::read_root_events(data);
    let card = events.iter().find_map(|event| match &event.kind {
        session::EventKind::ToolResult {
            message,
            meta: Some(meta),
            error: None,
            ..
        } if message.tool_call_id.as_deref() == Some("call-graph-card") => {
            meta.get("card").cloned()
        }
        _ => None,
    });
    if let Some(card) = card {
        return card;
    }
    let calls: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolCall { call_id, name, .. } => Some(format!("{call_id}:{name}")),
            _ => None,
        })
        .collect();
    let results: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolResult {
                message,
                error,
                meta,
                ..
            } => Some(format!(
                "{:?} err={error:?} meta={}",
                message.tool_call_id,
                meta.is_some()
            )),
            _ => None,
        })
        .collect();
    panic!("没有确认卡：calls={calls:?} results={results:?}");
}

/// 门任务和被把关任务按标题找（动态脚本建的名字固定）。
fn gate_children(store: &Store, root: &Task) -> (Task, Task, Task) {
    let children = store.children(&root.id).expect("子任务");
    let guarded = children
        .iter()
        .find(|child| child.title == "先查行数")
        .cloned()
        .expect("先查行数");
    let gate = children
        .iter()
        .find(|child| child.title == "独立核查")
        .cloned()
        .expect("独立核查");
    let downstream = children
        .iter()
        .find(|child| child.title == "再写结果")
        .cloned()
        .expect("再写结果");
    (guarded, gate, downstream)
}

/// 建好门图、确认、B 报回、门报回，返回手头的事实；调用方再判或改动。
async fn reach_gate_ready(
    host: &RootHost,
    router: &Router,
    data: &Path,
) -> (Task, Task, Task, tasks::ReportId) {
    router.enable_gate_kind();
    host.dispatch(Call::Send(send(host, "kind-1", "先查再核再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(host, data, 1).await;
    wait_until("建门图", || {
        tasks_store(data)
            .task_ids()
            .map(|ids| ids.len() == 4)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data);
    let root = graph_root(&store);
    let (guarded, gate, downstream) = gate_children(&store, &root);
    router.push_thin_for(
        guarded.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b-report",
            "task_report",
            r#"{"state":"ready","summary":"第一次结果：3 行"}"#,
        ),
    );
    router.push_thin_for(guarded.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        gate.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-gate-report",
            "task_report",
            r#"{"state":"ready","summary":"核查发现：空标题会漏判"}"#,
        ),
    );
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("好"));
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-gate-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),
        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");
    wait_until("B 报回 ready", || {
        tasks_store(data)
            .task(&guarded.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("门开跑并报回 ready", || {
        tasks_store(data)
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    let store = tasks_store(data);
    let gate = store.task(&gate.id).expect("读门").expect("门存在");
    let report_id = gate.latest_report().expect("门报告").id.clone();
    let guarded = store.task(&guarded.id).expect("读 B").expect("B 存在");
    let downstream = store.task(&downstream.id).expect("读 C").expect("C 存在");
    let root = graph_root(&store);
    (root, guarded, downstream, report_id)
}

/// 等 Kind 根会话真正空闲（没有在跑的轮）再发下一条；
/// 回放脚本按请求顺序消费，忙时发会跟 relay 轮抢脚本。
async fn wait_kind_idle(host: &service::RootHost) {
    for _ in 0..900 {
        let service::Response::Status(status) = host
            .dispatch(Call::Status(service::StatusRequest::default()))
            .await
            .expect("status")
        else {
            panic!("status 响应")
        };
        if !status.running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Kind 根会话一直不空闲");
}

/// 直接造一个 ready 任务（组接受用例的输入；调用面本身才是被测的接缝）。
fn seed_ready_task(
    store: &Store,
    cwd: &std::path::Path,
    title: &str,
    at: i64,
) -> (tasks::TaskId, tasks::ReportId) {
    let task = store
        .create_task(
            NewTask {
                quote: None,
                quote_source: None,
                kind_note: None,
                project_id: None,
                cwd_source: None,
                title: title.to_owned(),
                requirement: format!("{title} 的要求"),
                dod: format!("{title} 的 DoD"),
                cwd: cwd.to_path_buf(),
                conversation: ConversationId::new("kind-work"),
                model: ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: vec!["read".to_owned()],
            },
            at,
        )
        .unwrap();
    let claim = store.claim_effect(EffectKind::Launch, at + 1).unwrap();
    if let Some(claim) = claim {
        store.complete_effect(&claim).unwrap();
    }
    let attempt = store.begin_attempt(&task.id, at + 2).unwrap();
    let report = store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "查好了".to_owned(),
                question: None,
            },
            at + 3,
        )
        .unwrap();
    store
        .end_attempt(&task.id, attempt, at + 4, AttemptEnd::Reported)
        .unwrap();
    (task.id, report)
}
