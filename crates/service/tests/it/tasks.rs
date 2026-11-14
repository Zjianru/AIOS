//! T024.3：Kind 派发、ThinAgent 执行、上报 relay 的后端闭环（假模型、两个真会话）。

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

/// 假模型按请求体路由：工具表里有 task_accept 的是 Kind，有 task_report 的是 ThinAgent。
#[derive(Clone)]
struct Router {
    state: Arc<RouterState>,
}

/// T054.2：改要求 Kind 的动态脚本：按请求内容（触发词/skill 结果）出 skill→propose_action，
/// 不依赖静态队列顺序（relay 轮和用户消息的先后会抢脚本）。
struct UpdateProposal {
    trigger: String,
    skill_call: String,
    card_call: String,
    requirement: String,
    skill_emitted: bool,
    card_emitted: bool,
}

struct UpdateKind {
    task_id: String,
    proposals: Vec<UpdateProposal>,
}

struct RouterState {
    kind: Mutex<VecDeque<String>>,
    thin: Mutex<VecDeque<String>>,
    /// 按任务的 ThinAgent 脚本：多任务同时跑时，各自按 assignment 里的 task_id 取。
    thin_by_task: Mutex<HashMap<String, VecDeque<String>>>,
    /// 任务 id 要受理后才知道；测试先按标题排脚本（assignment 里也有标题）。
    thin_by_title: Mutex<HashMap<String, VecDeque<String>>>,

    bodies: Mutex<Vec<(bool, String)>>,
    unexpected: Mutex<Vec<(bool, String)>>,
    /// ThinAgent 的响应延迟：live 测试要在任务还在跑时接上事件流。
    thin_delay: Mutex<Option<Duration>>,
    /// T052：Kind 的动态脚本阶段（0=出确认卡、1=task_status、2=收尾）；None = 用静态队列。
    graph_kind: Mutex<Option<u8>>,
    /// T053：带里程碑门的任务图动态脚本阶段；None = 用静态队列。
    gate_kind: Mutex<Option<u8>>,
    /// T053.4：只提图不上的动态脚本（NoCard 收尾）。
    nocard_kind: Mutex<Option<u8>>,
    /// T053.4：连续提两张图两张卡（替换取消）。
    two_graph_kind: Mutex<Option<u8>>,
    /// T054.2：按请求内容驱动的改要求 Kind 脚本。
    update_kind: Mutex<Option<UpdateKind>>,
}

impl Router {
    fn new() -> Self {
        Self {
            state: Arc::new(RouterState {
                kind: Mutex::new(VecDeque::new()),
                thin: Mutex::new(VecDeque::new()),
                thin_by_task: Mutex::new(HashMap::new()),
                thin_by_title: Mutex::new(HashMap::new()),
                bodies: Mutex::new(Vec::new()),
                unexpected: Mutex::new(Vec::new()),
                thin_delay: Mutex::new(None),
                graph_kind: Mutex::new(None),
                gate_kind: Mutex::new(None),
                nocard_kind: Mutex::new(None),
                two_graph_kind: Mutex::new(None),
                update_kind: Mutex::new(None),
            }),
        }
    }

    /// T052：让 Kind 的第二轮起用动态脚本（先 propose_action 出卡，再 task_status，最后收尾）。
    fn enable_graph_kind(&self) {
        *self.state.graph_kind.lock().unwrap() = Some(0);
    }

    /// T053：Kind 用带里程碑门的动态脚本（skill → 提议三子任务图 → 出确认卡 → 收尾）。
    fn enable_gate_kind(&self) {
        *self.state.gate_kind.lock().unwrap() = Some(0);
    }

    /// T053.4：只提图不出的动态脚本（下一轮开始时按 NoCard 取消）。
    fn enable_nocard_kind(&self) {
        *self.state.nocard_kind.lock().unwrap() = Some(0);
    }

    /// T053.4：两轮各提一张图一张卡（第二张卡接受时第一张按 Replaced 取消）。
    fn enable_two_graph_kind(&self) {
        *self.state.two_graph_kind.lock().unwrap() = Some(0);
    }

    /// T053.4：第二轮脚本（下一轮再提一张图一张卡；同一轮仍只让出一张卡）。
    fn enable_two_graph_second(&self) {
        *self.state.two_graph_kind.lock().unwrap() = Some(10);
    }

    /// T054.2：改要求 Kind 的动态脚本（trigger, skill_call, card_call, requirement）。
    fn enable_update_kind(&self, task_id: &str, proposals: Vec<(String, String, String, String)>) {
        let proposals = proposals
            .into_iter()
            .map(
                |(trigger, skill_call, card_call, requirement)| UpdateProposal {
                    trigger,
                    skill_call,
                    card_call,
                    requirement,
                    skill_emitted: false,
                    card_emitted: false,
                },
            )
            .collect();
        *self.state.update_kind.lock().unwrap() = Some(UpdateKind {
            task_id: task_id.to_owned(),
            proposals,
        });
    }

    /// 按请求体内容决定这一步回什么：触发词在 → skill 调用；skill 结果在 → propose_action；
    /// 其余（受理/relay/收尾）都回一句普通文字。
    fn take_update_kind_script(&self, body: &str) -> Option<String> {
        let mut guard = self.state.update_kind.lock().unwrap();
        let update = guard.as_mut()?;
        for (index, proposal) in update.proposals.iter_mut().enumerate() {
            if !proposal.skill_emitted && body.contains(&proposal.trigger) {
                proposal.skill_emitted = true;
                return Some(crate::common::tool_use_sse(
                    index,
                    &proposal.skill_call,
                    "skill",
                    &serde_json::json!({"name": "aios-tasks"}).to_string(),
                ));
            }
            if !proposal.card_emitted && body.contains(&proposal.skill_call) {
                proposal.card_emitted = true;
                let args = serde_json::json!({
                    "title": "改任务要求",
                    "brief": "确认后同一个任务接着做。",
                    "options": [{
                        "id": "confirm",
                        "action": "update_task_requirement",
                        "label": "改任务要求",
                        "details": {
                            "task_id": update.task_id,
                            "requirement": proposal.requirement,
                        }
                    }]
                })
                .to_string();
                return Some(crate::common::tool_use_sse(
                    index + 10,
                    &proposal.card_call,
                    "propose_action",
                    &args,
                ));
            }
        }
        Some(crate::common::text_sse("收到。"))
    }

    fn set_thin_delay(&self, delay: Duration) {
        *self.state.thin_delay.lock().unwrap() = Some(delay);
    }

    fn clear_thin_delay(&self) {
        *self.state.thin_delay.lock().unwrap() = None;
    }

    fn push_kind(&self, body: impl Into<String>) {
        self.state.kind.lock().unwrap().push_back(body.into());
    }

    fn push_thin(&self, body: impl Into<String>) {
        self.state.thin.lock().unwrap().push_back(body.into());
    }

    /// 只给这个任务的 ThinAgent 排回复（按 assignment 里的 task_id 分）。
    fn push_thin_for(&self, task_id: &str, body: impl Into<String>) {
        self.state
            .thin_by_task
            .lock()
            .unwrap()
            .entry(task_id.to_owned())
            .or_default()
            .push_back(body.into());
    }

    /// 按标题排 ThinAgent 回复；受理前就能排，第一个请求不会落空。
    fn push_thin_for_title(&self, title: &str, body: impl Into<String>) {
        self.state
            .thin_by_title
            .lock()
            .unwrap()
            .entry(title.to_owned())
            .or_default()
            .push_back(body.into());
    }

    fn take_thin_script(&self, body: &str) -> Option<String> {
        let (task_id, title) = assignment_identity(body)?;
        if let Some(script) = self
            .state
            .thin_by_task
            .lock()
            .unwrap()
            .get_mut(&task_id)
            .and_then(VecDeque::pop_front)
        {
            return Some(script);
        }
        self.state
            .thin_by_title
            .lock()
            .unwrap()
            .get_mut(&title)
            .and_then(VecDeque::pop_front)
    }

    fn bodies(&self, thin: bool) -> Vec<String> {
        self.state
            .bodies
            .lock()
            .unwrap()
            .iter()
            .filter(|(is_thin, _)| *is_thin == thin)
            .map(|(_, body)| body.clone())
            .collect()
    }
}

impl Respond for Router {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = String::from_utf8_lossy(&request.body).to_string();
        let thin =
            !body.contains("\"name\":\"task_accept\"") && body.contains("\"name\":\"task_report\"");
        self.state.bodies.lock().unwrap().push((thin, body.clone()));
        // 多任务同时跑：先按 assignment 的 task_id、再按标题取这个任务的脚本；没有就落到公共队列。
        let per_task = if thin {
            self.take_thin_script(&body)
        } else {
            None
        };
        if !thin && let Some(sse) = self.take_update_kind_script(&body) {
            return ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse);
        }
        // T052：任务图的动态 Kind 轮：图 id 在 task_graph_propose 的工具结果里，静态回放拿不到。
        if !thin
            && let Some(step) = {
                let mut phase = self.state.graph_kind.lock().unwrap();
                let step = phase.take();
                if let Some(step) = step {
                    *phase = Some(step + 1);
                }
                step
            }
        {
            let sse = match step {
                0 => crate::common::tool_use_sse(
                    0,
                    "call-graph-skill",
                    "skill",
                    &serde_json::json!({"name": "aios-tasks"}).to_string(),
                ),
                1 => {
                    let graph = serde_json::json!({
                        "title": "查行数再写结果",
                        "requirement": "先查 notes.md 有几行，再把行数写进 other.txt",
                        "dod": "other.txt 里写着行数",
                        "subtasks": [
                            {"title": "先查行数", "requirement": "查 notes.md 有几行", "dod": "给出行数"},
                            {"title": "再写结果", "requirement": "把行数写进 other.txt", "dod": "other.txt 里是行数", "write_scope": "other.txt", "after": [0]}
                        ]
                    })
                    .to_string();
                    crate::common::tool_use_sse(
                        1,
                        "call-graph-propose",
                        "task_graph_propose",
                        &graph,
                    )
                }
                2 => {
                    let root = graph_root_id(&body).expect("请求里要有「已建任务图 <id>」");
                    let args = serde_json::json!({
                        "title": "任务图",
                        "brief": "两步按先后顺序跑；确认后第一项开跑，第二项等第一项的结果。",
                        "options": [{
                            "id": "confirm",
                            "action": "start_task_graph",
                            "label": "确认并开跑",
                            "details": {"task_id": root}
                        }]
                    })
                    .to_string();
                    crate::common::tool_use_sse(2, "call-graph-card", "propose_action", &args)
                }
                3 => {
                    let root = graph_root_id(&body).expect("请求里要有「已建任务图 <id>」");
                    let args = serde_json::json!({"task_id": root}).to_string();
                    crate::common::tool_use_sse(3, "call-graph-status", "task_status", &args)
                }
                _ => crate::common::text_sse("任务图已确认。"),
            };
            return ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse);
        }
        // T053：里程碑门的动态 Kind 轮。
        if !thin
            && let Some(step) = {
                let mut phase = self.state.gate_kind.lock().unwrap();
                let step = phase.take();
                if let Some(step) = step {
                    *phase = Some(step + 1);
                }
                step
            }
        {
            let sse = match step {
                0 => crate::common::tool_use_sse(
                    0,
                    "call-gate-skill",
                    "skill",
                    &serde_json::json!({"name": "aios-tasks"}).to_string(),
                ),
                1 => {
                    let graph = serde_json::json!({
                        "title": "查行数再写结果",
                        "requirement": "先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果",
                        "dod": "核查通过了再写结果",
                        "subtasks": [
                            {"title": "先查行数", "requirement": "查 notes.md 有几行", "dod": "给出行数"},
                            {"title": "独立核查", "requirement": "独立核查行数结果", "dod": "给出核查发现", "milestone": true, "after": [0]},
                            {"title": "再写结果", "requirement": "把行数写进 other.txt", "dod": "other.txt 里是行数", "write_scope": "other.txt", "after": [0]}
                        ]
                    })
                    .to_string();
                    crate::common::tool_use_sse(
                        1,
                        "call-gate-propose",
                        "task_graph_propose",
                        &graph,
                    )
                }
                2 => {
                    let root = graph_root_id(&body).expect("请求里要有「已建任务图 <id>」");
                    let args = serde_json::json!({
                        "title": "任务图",
                        "brief": "三步：先查、独立核查、核查通过后再写；核查结果由用户在卡上判。",
                        "options": [{
                            "id": "confirm",
                            "action": "start_task_graph",
                            "label": "确认并开跑",
                            "details": {"task_id": root}
                        }]
                    })
                    .to_string();
                    crate::common::tool_use_sse(2, "call-gate-card", "propose_action", &args)
                }
                _ => crate::common::text_sse("收到。"),
            };
            return ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse);
        }
        // T053.4：只提图不上卡（NoCard）。
        if !thin
            && let Some(step) = {
                let mut phase = self.state.nocard_kind.lock().unwrap();
                let step = phase.take();
                if let Some(step) = step {
                    *phase = Some(step + 1);
                }
                step
            }
        {
            let sse = match step {
                0 => crate::common::tool_use_sse(
                    0,
                    "call-nocard-skill",
                    "skill",
                    &serde_json::json!({"name": "aios-tasks"}).to_string(),
                ),
                1 => {
                    let graph = serde_json::json!({
                        "title": "第一张图",
                        "requirement": "先查行数再写结果",
                        "dod": "结果写好了",
                        "subtasks": [
                            {"title": "先查行数", "requirement": "查行数", "dod": "给出行数"},
                            {"title": "再写结果", "requirement": "写结果", "dod": "写好了", "after": [0]}
                        ]
                    })
                    .to_string();
                    crate::common::tool_use_sse(
                        1,
                        "call-nocard-propose",
                        "task_graph_propose",
                        &graph,
                    )
                }
                _ => crate::common::text_sse("先这样。"),
            };
            return ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse);
        }
        // T053.4：两轮各提一张图一张卡（第一张被第二张替换取消）。
        if !thin
            && let Some(step) = {
                let mut phase = self.state.two_graph_kind.lock().unwrap();
                let step = phase.take();
                if let Some(step) = step {
                    *phase = Some(step + 1);
                }
                step
            }
        {
            let sse = match step {
                0 => crate::common::tool_use_sse(
                    0,
                    "call-two-skill",
                    "skill",
                    &serde_json::json!({"name": "aios-tasks"}).to_string(),
                ),
                1 => {
                    let graph = serde_json::json!({
                        "title": "第一张图",
                        "requirement": "先查行数再写结果",
                        "dod": "结果写好了",
                        "subtasks": [
                            {"title": "先查行数", "requirement": "查行数", "dod": "给出行数"},
                            {"title": "再写结果", "requirement": "写结果", "dod": "写好了", "after": [0]}
                        ]
                    })
                    .to_string();
                    crate::common::tool_use_sse(
                        1,
                        "call-two-propose-1",
                        "task_graph_propose",
                        &graph,
                    )
                }
                2 => {
                    let root = graph_root_id(&body).expect("第一张图 id");
                    let args = serde_json::json!({
                        "title": "第一张卡",
                        "brief": "第一张图的确认卡。",
                        "options": [{
                            "id": "confirm",
                            "action": "start_task_graph",
                            "label": "确认并开跑",
                            "details": {"task_id": root}
                        }]
                    })
                    .to_string();
                    crate::common::tool_use_sse(2, "call-two-card-1", "propose_action", &args)
                }
                // 第一轮只出一张卡就收尾；第二张图和卡留到下一轮（同轮只许一张卡）。
                3 => crate::common::text_sse("先这样。"),
                10 => crate::common::tool_use_sse(
                    10,
                    "call-two-skill-2",
                    "skill",
                    &serde_json::json!({"name": "aios-tasks"}).to_string(),
                ),
                11 => {
                    let graph = serde_json::json!({
                        "title": "第二张图",
                        "requirement": "先查行数再写结果（第二版）",
                        "dod": "结果写好了",
                        "subtasks": [
                            {"title": "先查行数", "requirement": "查行数", "dod": "给出行数"},
                            {"title": "再写结果", "requirement": "写结果", "dod": "写好了", "after": [0]}
                        ]
                    })
                    .to_string();
                    crate::common::tool_use_sse(
                        11,
                        "call-two-propose-2",
                        "task_graph_propose",
                        &graph,
                    )
                }
                12 => {
                    let root = graph_root_id_last(&body).expect("第二张图 id");
                    let args = serde_json::json!({
                        "title": "第二张卡",
                        "brief": "第二张图的确认卡。",
                        "options": [{
                            "id": "confirm",
                            "action": "start_task_graph",
                            "label": "确认并开跑",
                            "details": {"task_id": root}
                        }]
                    })
                    .to_string();
                    crate::common::tool_use_sse(12, "call-two-card-2", "propose_action", &args)
                }
                4 => {
                    let root = graph_root_id_last(&body).expect("第二张图 id");
                    let args = serde_json::json!({
                        "title": "第二张卡",
                        "brief": "第二张图的确认卡。",
                        "options": [{
                            "id": "confirm",
                            "action": "start_task_graph",
                            "label": "确认并开跑",
                            "details": {"task_id": root}
                        }]
                    })
                    .to_string();
                    crate::common::tool_use_sse(4, "call-two-card-2", "propose_action", &args)
                }
                _ => crate::common::text_sse("收到。"),
            };
            return ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse);
        }
        let sse = match per_task {
            Some(sse) => sse,
            None => {
                let queue = if thin {
                    &self.state.thin
                } else {
                    &self.state.kind
                };
                let Some(sse) = queue.lock().unwrap().pop_front() else {
                    self.state
                        .unexpected
                        .lock()
                        .unwrap()
                        .push((thin, body.chars().take(400).collect()));
                    return ResponseTemplate::new(500);
                };
                sse
            }
        };
        let template = ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse);
        match (thin, *self.state.thin_delay.lock().unwrap()) {
            (true, Some(delay)) => template.set_delay(delay),
            _ => template,
        }
    }
}

/// T052：从 Kind 请求体里「已建任务图 <id>」取出图 id（动态回放用；多个时取第一个）。
fn graph_root_id(body: &str) -> Option<String> {
    let rest = body.split("已建任务图 ").nth(1)?;
    let id: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    (!id.is_empty()).then_some(id)
}

/// T053.4：同一请求里出现多张图时取最后一个（第二轮那张图）。
fn graph_root_id_last(body: &str) -> Option<String> {
    let rest = body.rsplit("已建任务图 ").next()?;
    let id: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    (!id.is_empty()).then_some(id)
}

/// 从 ThinAgent 请求里第一条 assignment 取出任务 id 和标题；别的请求没有就是 None。
fn assignment_identity(body: &str) -> Option<(String, String)> {
    let value: Value = serde_json::from_str(body).ok()?;
    for message in value["messages"].as_array()? {
        for block in message["content"].as_array()? {
            let Some(text) = block["text"].as_str() else {
                continue;
            };
            let Ok(assignment) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            if let (Some(id), Some(title)) =
                (assignment["task_id"].as_str(), assignment["title"].as_str())
            {
                return Some((id.to_owned(), title.to_owned()));
            }
        }
    }
    None
}

async fn mount(server: &MockServer, router: &Router) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(router.clone())
        .mount(server)
        .await;
}

async fn open_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
    open_host_with(server, data, cwd, |_| {}).await
}

async fn open_host_with(
    server: &MockServer,
    data: &Path,
    cwd: &Path,
    configure: impl FnOnce(&mut service::RootSessionOptions),
) -> RootHost {
    let mut options = crate::common::options(data);
    options.cwd = cwd.to_path_buf();
    options.model.base_url = server.uri();
    options.config = AgentConfig {
        instruction_roots: vec![data.join("agents-home")],
        skill_roots: vec![data.join("skills-home")],
        ..AgentConfig::default()
    };
    configure(&mut options);
    RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await
}

fn send(_host: &RootHost, request_id: &str, text: &str) -> SendRequest {
    SendRequest {
        request_id: request_id.to_owned(),
        content: text.to_owned(),
        delivery: DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
    }
}

fn tasks_store(data: &Path) -> Store {
    Store::open(data.join(service::TASKS_DB_FILE)).expect("任务库")
}

fn one_task(store: &Store) -> Task {
    let ids = store.task_ids().expect("任务 id");
    assert_eq!(ids.len(), 1, "只应有一个任务：{ids:?}");
    store.task(&ids[0]).expect("读任务").expect("任务存在")
}

async fn wait_until(label: &str, mut check: impl FnMut() -> bool) {
    // T054：续跑的下一张结果卡可能晚几秒才 Steer 进 Kind；给足 18s。
    for _ in 0..900 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等不到：{label}");
}

fn tool_names(body: &str) -> Vec<String> {
    let value: Value = serde_json::from_str(body).expect("请求体 JSON");
    value["tools"]
        .as_array()
        .expect("tools 数组")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

fn relay_messages(events: &[session::Event]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return None;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return None;
            };
            let rpc_id = rpc_id.as_deref()?;
            if !rpc_id.starts_with("task-report:") {
                return None;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            Some((rpc_id.to_owned(), serde_json::from_str(&text).ok()?))
        })
        .collect()
}

/// Kind 收到的抄送：rpc id 是 task-cc:<inputId>，正文是 task_cc JSON。
///
/// Inject 不唤醒时消息只落 agent/inbox/spliced（还没领取），被下一次请求领走后才
/// 变成 user/message；两处都收，按 rpc id 去重。
fn cc_messages(events: &[session::Event]) -> Vec<(String, Value)> {
    let mut found: Vec<(String, Value)> = Vec::new();
    for event in events {
        let messages: Vec<&model::Message> = match &event.kind {
            session::EventKind::UserMessage { message, .. } => vec![message],
            session::EventKind::InboxSpliced { inserted, .. } => inserted.iter().collect(),
            _ => continue,
        };
        for message in messages {
            let model::MessageSource::User { rpc_id } = &message.source else {
                continue;
            };
            let Some(rpc_id) = rpc_id.as_deref() else {
                continue;
            };
            if !rpc_id.starts_with("task-cc:") {
                continue;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            let Ok(body) = serde_json::from_str(&text) else {
                continue;
            };
            if let Some(entry) = found.iter_mut().find(|(id, _)| id == rpc_id) {
                entry.1 = body;
            } else {
                found.push((rpc_id.to_owned(), body));
            }
        }
    }
    found
}

/// Kind 收到的停止 relay：rpc id 是 task-stop:<taskId>:<attemptId>。
fn stopped_relays(events: &[session::Event]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return None;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return None;
            };
            let rpc_id = rpc_id.as_deref()?;
            if !rpc_id.starts_with("task-stop:") {
                return None;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            Some((rpc_id.to_owned(), serde_json::from_str(&text).ok()?))
        })
        .collect()
}

fn last_turn_end(events: &[session::Event]) -> Option<session::TurnEndReason> {
    events.iter().rev().find_map(|event| match &event.kind {
        session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
        _ => None,
    })
}

/// 页面「停下这一轮」的调用；测试只关心响应。
async fn interrupt_task(host: &RootHost, task: &Task) -> service::TaskInterruptResponse {
    let response = host
        .dispatch(Call::TaskInterrupt(TaskInterruptRequest {
            task_id: task.id.as_str().to_owned(),
        }))
        .await
        .expect("停下这一轮");
    let Response::TaskInterrupt(interrupt) = response else {
        panic!("TaskInterrupt 的响应形状不对");
    };
    interrupt
}

/// 页面和 Kind 的 task_stop 都走这一个调用；测试只关心响应。
async fn stop_task(fixture: &Fixture, task: &Task, mode: StopMode) -> service::TaskStopResponse {
    let response = fixture
        .host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode,
        }))
        .await
        .expect("停止");
    let Response::TaskStop(stop) = response else {
        panic!("TaskStop 的响应形状不对");
    };
    stop
}

/// 这次 Attempt 的会话最后一件事是 turn/end：报告之后那段收尾也结束了，会话写者马上放开。
fn session_turn_ended(data: &Path, attempt: &tasks::Attempt) -> bool {
    let Some(conversation) = attempt.conversation.clone() else {
        return false;
    };
    let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
        return false;
    };
    let Ok(store) = session::SessionStore::open_read_only(data, &id) else {
        return false;
    };
    matches!(
        store.events().last().map(|event| &event.kind),
        Some(session::EventKind::TurnEnd { .. })
    )
}

/// 一个 Attempt 的 ThinAgent 会话日志（只读）。
fn thin_events(data: &Path, attempt: &tasks::Attempt) -> Vec<session::Event> {
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).expect("会话 id");
    session::SessionStore::open_read_only(data, &session_id)
        .expect("只读打开任务会话")
        .events()
        .to_vec()
}

/// 收尾停按 request id 幂等：同一个 id 的消息在会话日志里只该有一条。
fn injected_stop_notes(events: &[session::Event], task: &Task, attempt: &tasks::Attempt) -> usize {
    let request_id = format!("task-stop:{}:{}", task.id, attempt.id);
    events
        .iter()
        .filter(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => matches!(
                &message.source,
                model::MessageSource::User { rpc_id: Some(rpc) } if rpc == &request_id
            ),
            _ => false,
        })
        .count()
}

/// 一条 tool/result 的正文和 is_error。
fn tool_result(events: &[session::Event], call_id: &str) -> (String, Option<bool>) {
    events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. }
                if message.tool_call_id.as_deref() == Some(call_id) =>
            {
                Some((
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                    message.is_error,
                ))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{call_id} 的 tool/result"))
}

/// 一个 Attempt 以什么理由结束；还没结束或没有 Attempt 时是 None。
fn attempt_end(store: &Store, task_id: &tasks::TaskId) -> Option<tasks::AttemptEnd> {
    store
        .task(task_id)
        .ok()
        .flatten()
        .and_then(|task| task.latest_attempt().and_then(|attempt| attempt.end))
}

#[tokio::test]
async fn accept_freezes_the_task_and_the_launch_uses_it() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 存 Exa key 只是为了加载 web skill 后两件联网工具能开出（条件照旧），不真的联网。
    credentials::Keyring::open(data.path(), Some(&data.path().join("identity.txt")))
        .unwrap()
        .save_exa_key("test-exa-key")
        .unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let args = serde_json::json!({
        "title": "查一下 Exa 怎么用",
        "requirement": "读官方文档，给出能操作的例子",
        "dod": "给出例子和出处"
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    // 同一个 ToolCall id 再放一次：受理要幂等。
    router.push_kind(crate::common::tool_use_sse(
        1,
        "call-accept",
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));

    host.dispatch(Call::Send(send(&host, "kind-1", "查一下 Exa 怎么用")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;

    let store = tasks_store(data.path());
    let task = one_task(&store);
    let root_id = host.info().session_id.expect("根会话 id");
    assert_eq!(task.conversation.as_str(), root_id);
    assert_eq!(task.cwd, std::fs::canonicalize(cwd.path()).unwrap());
    assert_eq!(task.model.model, "deepseek-flash");
    assert_eq!(task.tools, service::THIN_TOOLS.map(str::to_owned).to_vec());

    let events = crate::common::read_root_events(data.path());
    let results: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(results[0].contains("已派出任务"), "{results:?}");
    assert!(
        results[1].contains("Already accepted as") && results[1].contains(task.id.as_str()),
        "{results:?}"
    );

    // Kind 的工具表：有 task_accept/task_status，没有 task_report。
    let kind_bodies = router.bodies(false);
    let names = tool_names(&kind_bodies[0]);
    assert!(names.contains(&"task_accept".to_owned()), "{names:?}");
    assert!(names.contains(&"task_status".to_owned()), "{names:?}");
    assert!(!names.contains(&"task_report".to_owned()), "{names:?}");
    assert!(
        !kind_bodies[0].contains("investigator"),
        "默认角色的 skill 不能进 Kind"
    );

    // 受理后改默认模型；launch 仍要用受理时冻结的那个。
    host.dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
        connection: None,
        model: Some("deepseek-pro".to_owned()),
        effort_wanted: None,
    }))
    .await
    .expect("换模型");
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(false);
    router.push_thin(crate::common::tool_use_sse(
        2,
        "call-skill",
        "skill",
        &serde_json::json!({"name": "web"}).to_string(),
    ));
    router.push_thin(crate::common::tool_use_sse(
        3,
        "call-report",
        "task_report",
        &serde_json::json!({"state": "ready", "summary": "官方文档给了一个例子。"}).to_string(),
    ));
    router.push_thin(crate::common::text_sse("done"));

    let store = tasks_store(data.path());
    wait_until("ready 报告", || {
        let task = store.task(&task.id).ok().flatten();
        task.and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("Kind 收到 relay", || {
        !relay_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;

    let task = store.task(&task.id).unwrap().unwrap();
    let attempt = task.latest_attempt().expect("Attempt");
    let conversation = attempt.conversation.clone().expect("会话 id");
    let id = session::SessionId::parse(conversation.as_str()).unwrap();
    let thin_session = session::SessionStore::open_read_only(data.path(), &id).expect("会话日志");
    let header: Value = serde_json::from_str(
        std::fs::read_to_string(thin_session.path())
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        header["cwd"],
        std::fs::canonicalize(cwd.path()).unwrap().to_str().unwrap()
    );
    assert!(
        thin_session.events().iter().any(|event| {
            matches!(
                &event.kind,
                session::EventKind::UserMessage { message, .. }
                    if matches!(&message.source, model::MessageSource::User { rpc_id: Some(id) } if id == &format!("assignment:{}", task.id))
            )
        }),
        "第一条消息的 request id 是 assignment:<taskId>"
    );

    let thin_bodies = router.bodies(true);
    assert!(!thin_bodies.is_empty(), "ThinAgent 至少发了一次请求");
    for body in &thin_bodies {
        let request: Value = serde_json::from_str(body).unwrap();
        assert_eq!(request["model"], "deepseek-flash", "冻结的模型");
    }
    // 第一份请求只开出无条件的 7 件；加载 web skill（Exa key 已存）后开出 9 件。
    let loaded = thin_bodies
        .iter()
        .find(|body| body.contains("\"name\":\"web_search\""))
        .expect("加载 web 后的请求");
    let mut names = tool_names(loaded);
    names.sort();
    let mut expected: Vec<String> = service::THIN_TOOLS.map(str::to_owned).to_vec();
    expected.sort();
    assert_eq!(names, expected, "ThinAgent 的工具表正好 9 件");
    let first_names = tool_names(&thin_bodies[0]);
    assert!(!first_names.contains(&"bash".to_owned()), "{first_names:?}");
    assert!(
        !first_names.contains(&"write".to_owned()),
        "{first_names:?}"
    );
    assert!(
        thin_bodies[0].contains("investigator"),
        "默认角色的 skill 要进 ThinAgent"
    );

    let relays = relay_messages(&crate::common::read_root_events(data.path()));
    assert_eq!(relays.len(), 1, "Kind 只收到一条 relay：{relays:?}");
    let body = &relays[0].1;
    assert_eq!(body["kind"], "task_report");
    assert_eq!(body["state"], "ready");
    assert_eq!(body["task"]["id"], task.id.as_str());
    assert_eq!(body["task"]["stage"], "ready_for_review");
    assert_eq!(body["summary"], "官方文档给了一个例子。");
}

#[tokio::test]
async fn task_status_returns_the_full_report_and_attempts() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;

    // 任务的发起会话不是根会话，relay 会 Later，不会在断言前 Steer 醒 Kind。
    router.push_thin(crate::common::tool_use_sse(
        0,
        "call-report",
        "task_report",
        &serde_json::json!({"state": "ready", "summary": "完整答案：第一行\n第二行"}).to_string(),
    ));
    router.push_thin(crate::common::text_sse(""));
    let store = tasks_store(data.path());
    let task = store
        .create_task(
            tasks::NewTask {
                title: "查一下".to_owned(),
                requirement: "找答案".to_owned(),
                dod: "有答案".to_owned(),
                cwd: data.path().to_path_buf(),
                conversation: tasks::ConversationId::new("another-kind-session"),
                model: tasks::ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: service::THIN_TOOLS.map(str::to_owned).to_vec(),
            },
            1_700_000_000_000,
        )
        .unwrap();
    wait_until("ready 报告", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;

    let status_args = serde_json::json!({"task_id": task.id.as_str()}).to_string();
    router.push_kind(crate::common::tool_use_sse(
        1,
        "call-status",
        "task_status",
        &status_args,
    ));
    router.push_kind(crate::common::text_sse("我看一下"));
    host.dispatch(Call::Send(send(&host, "kind-1", "任务怎么样了")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;

    let events = crate::common::read_root_events(data.path());
    let result = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .find(|text| text.contains("Attempt 数"))
        .expect("task_status 结果");
    assert!(result.contains("阶段：待检查"), "{result}");
    assert!(result.contains("Attempt 数：1"), "{result}");
    assert!(result.contains("完整答案：第一行\n第二行"), "{result}");
    let unexpected = router.state.unexpected.lock().unwrap();
    assert!(
        unexpected.is_empty(),
        "假模型收到没有安排的请求：{unexpected:?}"
    );
}

#[tokio::test]
async fn a_missing_credential_opens_no_task_session_and_relays_what_is_missing() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let args = serde_json::json!({
        "title": "查一下",
        "requirement": "找答案",
        "dod": "有答案"
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));
    host.dispatch(Call::Send(send(&host, "kind-1", "查一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;

    let store = tasks_store(data.path());
    let task = one_task(&store);
    host.dispatch(Call::DisabledModelsSet(DisabledModelsSetRequest {
        models: vec!["deepseek-flash".to_owned()],
    }))
    .await
    .expect("屏蔽模型");
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(false);

    wait_until("缺项报告", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| !task.missing.is_empty())
    })
    .await;
    wait_until("缺项 relay", || {
        !relay_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;

    let task = store.task(&task.id).unwrap().unwrap();
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    assert!(
        task.missing
            .iter()
            .any(|item| item.as_str().contains("凭证")),
        "{:?}",
        task.missing
    );
    assert!(router.bodies(true).is_empty(), "缺项不能开任务会话");
    let session_files = std::fs::read_dir(data.path().join("sessions"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        })
        .count();
    assert_eq!(session_files, 1, "只有 Kind 的根会话");
    let relays = relay_messages(&crate::common::read_root_events(data.path()));
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "blocked");
    assert_eq!(relays[0].1["task"]["stage"], "needs_conditions");
    // 程序按任务包缺项写的 blocked：origin=package；缺项文本在程序写好的摘要里。
    assert_eq!(relays[0].1["origin"], "package");
    let summary = relays[0].1["summary"].as_str().unwrap();
    assert!(summary.contains("任务包缺"), "{summary}");
    assert!(summary.contains("deepseek-flash"), "{summary}");
}

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

    /// T054.2：确认卡场景要产品动作登记表 + 自带 skills（propose_action 才有 update_task_requirement）。
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
            "dod": "有答案"
        });
        self.accept_args(pause, args, spares).await
    }

    /// 用给定的 task_accept 参数受理一个任务（写任务带 write_scope）。
    async fn accept_args(&self, pause: bool, args: serde_json::Value, spares: usize) -> Task {
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

fn report_sse(
    index: usize,
    call: &str,
    state: &str,
    summary: &str,
    question: Option<&str>,
) -> String {
    let mut args = serde_json::json!({"state": state, "summary": summary});
    if let Some(question) = question {
        args["question"] = serde_json::json!(question);
    }
    crate::common::tool_use_sse(index, call, "task_report", &args.to_string())
}

fn waiting_relays(data: &Path) -> Vec<(String, Value)> {
    relay_messages(&crate::common::read_root_events(data))
}

#[tokio::test]
async fn a_turn_end_with_a_reply_synthesizes_waiting_then_a_status_report_replaces_it() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查，别急"));
    wait_until("waiting relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;

    let relays = waiting_relays(fixture.data());
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "waiting");
    assert_eq!(relays[0].1["summary"], "我还在查，别急");
    assert_eq!(relays[0].1["task"]["stage"], "running");
    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);

    // 要状态那次 turn 直接报 ready：waiting 能被终态覆盖，不报冲突。
    fixture.router.push_thin(report_sse(
        1,
        "call-report",
        "ready",
        "答案在第二行
有出处",
        None,
    ));
    fixture.router.push_thin(crate::common::text_sse("done"));
    let status_args = serde_json::json!({"task_id": task.id.as_str()}).to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        2,
        "call-status-1",
        "task_status",
        &status_args,
    ));
    fixture.router.push_kind(crate::common::tool_use_sse(
        3,
        "call-status-2",
        "task_status",
        &status_args,
    ));
    fixture.router.push_kind(crate::common::text_sse("好"));
    // relay 可能 Steer 进下一 step，多备一段回复。
    fixture.router.push_kind(crate::common::text_sse("收到"));

    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-2", "怎么样了")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;
    wait_until("ready relay", || waiting_relays(fixture.data()).len() == 2).await;

    let relays = waiting_relays(fixture.data());
    assert_eq!(relays[1].1["state"], "ready");
    assert_eq!(
        relays[1].1["summary"],
        "答案在第二行
有出处"
    );
    assert_eq!(relays[1].1["task"]["stage"], "ready_for_review");

    // 同一个 Attempt 只要一次报告：Kind 的两次 task_status 只落一条 status 请求。
    let task = store.task(&task.id).unwrap().unwrap();
    let attempt = task.latest_attempt().unwrap();
    let id = session::SessionId::parse(attempt.conversation.as_ref().unwrap().as_str()).unwrap();
    let thin = session::SessionStore::open_read_only(fixture.data(), &id).unwrap();
    let status_requests = thin
        .events()
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                session::EventKind::UserMessage { message, .. }
                    if matches!(&message.source, model::MessageSource::User { rpc_id: Some(rpc) } if rpc == &format!("status:{}:{}", task.id, attempt.id))
            )
        })
        .count();
    assert_eq!(status_requests, 1, "同一个 Attempt 只有一条状态请求");

    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);
}

#[tokio::test]
async fn a_turn_end_without_a_reply_relays_the_status_hint() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture.router.push_thin(crate::common::text_sse(""));
    wait_until("waiting relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;
    let relays = waiting_relays(fixture.data());
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "waiting");
    assert_eq!(
        relays[0].1["summary"],
        "ThinAgent 没上报，用 task_status 去问"
    );
    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);
}

#[tokio::test]
async fn a_blocked_report_carries_its_question() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture.router.push_thin(report_sse(
        1,
        "call-report",
        "blocked",
        "缺一份只有你能给的版本号",
        Some("用哪个版本？"),
    ));
    fixture.router.push_thin(crate::common::text_sse("等回答"));
    wait_until("blocked relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;
    let relays = waiting_relays(fixture.data());
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "blocked");
    assert_eq!(relays[0].1["question"], "用哪个版本？");
    assert_eq!(relays[0].1["task"]["stage"], "needs_conditions");
    // 缺项报告同样是 blocked + needs_conditions；.4 只靠 origin 区分「卡住了」和「没开跑」。
    assert_eq!(relays[0].1["origin"], "agent");
    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    assert!(
        store.task(&task.id).unwrap().unwrap().missing.is_empty(),
        "agent 报的 blocked 不是任务包缺项"
    );
}

#[tokio::test]
async fn task_handback_relays_one_blocked_card_with_the_fixed_question() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-handback",
        "task_handback",
        &serde_json::json!({"reason": "用户要改任务要求，这超出了任务范围"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::text_sse("交回了"));
    // 交回的 relay 由效果泵异步送进 Kind 根会话：等 relay 本身落进根日志再断言。
    // 不等 ThinAgent 的第二次请求——那时 relay 可能还没写，读到的会是 0 条（时红时绿）。
    wait_until("交回的 relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "交回只出恰一条 relay：{relays:?}");
    let body = &relays[0].1;
    assert_eq!(body["kind"], "task_report");
    assert_eq!(body["state"], "blocked");
    // 页面按 origin=agent 把 blocked 画成「卡住了」；包缺项才是「没开跑」。
    assert_eq!(body["origin"], "agent");
    assert_eq!(body["question"], "这超出了任务范围，要不要改任务？");
    assert_eq!(body["summary"], "用户要改任务要求，这超出了任务范围");
    assert_eq!(body["task"]["stage"], "needs_conditions");

    let store = tasks_store(fixture.data());
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    assert_eq!(
        attempt.end,
        Some(AttemptEnd::Reported),
        "交回也以 reported 结束这个 Attempt"
    );
    let events = thin_events(fixture.data(), &attempt);
    let (text, error) = tool_result(&events, "call-handback");
    assert_eq!(error, None, "{text}");
    assert!(text.contains("已交回"), "{text}");
}

#[tokio::test]
async fn task_handback_twice_returns_already_handed_back_without_a_second_relay() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-handback-1",
        "task_handback",
        &serde_json::json!({"reason": "第一次超出了任务范围"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::tool_use_sse(
        2,
        "call-handback-2",
        "task_handback",
        &serde_json::json!({"reason": "第二次也要交回"}).to_string(),
    ));
    fixture
        .router
        .push_thin(crate::common::text_sse("交回了两次"));
    wait_until("第二次交回的回执", || {
        let store = tasks_store(fixture.data());
        let Some(task_row) = store.task(&task.id).ok().flatten() else {
            return false;
        };
        let Some(attempt) = task_row.latest_attempt() else {
            return false;
        };
        if attempt.conversation.is_none() {
            return false;
        }
        thin_events(fixture.data(), attempt).iter().any(|event| {
            matches!(
                &event.kind,
                session::EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some("call-handback-2")
            )
        })
    })
    .await;

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "第二次交回不再出 relay：{relays:?}");
    let store = tasks_store(fixture.data());
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    let (first, first_error) = tool_result(&events, "call-handback-1");
    assert_eq!(first_error, None, "{first}");
    assert!(first.contains("已交回"), "{first}");
    let (second, second_error) = tool_result(&events, "call-handback-2");
    assert_eq!(second_error, None, "{second}");
    assert_eq!(second, "已交回");
    let reports: Vec<&tasks::Report> = task_row
        .reports
        .iter()
        .filter(|report| report.attempt_id == attempt.id)
        .collect();
    assert_eq!(reports.len(), 1, "同一个 Attempt 只留一份报告：{reports:?}");
    assert_ne!(reports[0].summary, "第二次也要交回");
}

#[tokio::test]
async fn thin_agent_handback_rule_and_tool_stay_off_kind() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查完了", None));
    fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 请求", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let thin_bodies = fixture.router.bodies(true);
    let thin: Value = serde_json::from_str(&thin_bodies[0]).unwrap();
    let thin_tools = tool_names(&thin_bodies[0]);
    assert!(
        thin_tools.contains(&"task_handback".to_owned()),
        "{thin_tools:?}"
    );
    let thin_system = thin["system"].as_str().expect("ThinAgent 系统提示");
    assert!(
        thin_system.contains("outside this task's scope"),
        "交回规则要进 ThinAgent 的 policy_prompt：{thin_system}"
    );
    assert!(
        thin_system.contains("do not do that work yourself"),
        "规则要写明不自己做：{thin_system}"
    );
    assert!(
        !thin_system.contains("You are AIOS's coordinator"),
        "Kind 的派任务说明不能进 ThinAgent：{thin_system}"
    );
    assert!(
        !thin_system.contains("这超出了任务范围，要不要改任务？"),
        "固定问句只能由程序写进 relay，不能抄进角色规则"
    );

    let kind_bodies = fixture.router.bodies(false);
    let kind_tools = tool_names(&kind_bodies[0]);
    assert!(
        !kind_tools.contains(&"task_handback".to_owned()),
        "task_handback 只给 ThinAgent：{kind_tools:?}"
    );
    let kind: Value = serde_json::from_str(&kind_bodies[0]).unwrap();
    let kind_system = kind["system"].as_str().expect("Kind 系统提示");
    assert!(!kind_system.contains("task_handback"), "{kind_system}");
    assert!(!kind_system.contains("这超出了任务范围，要不要改任务？"));
    assert_eq!(service::THIN_TOOLS.len(), 11, "ThinAgent 的工具表 11 件");
}

/// spec v1「实现决定」对照表逐条写成的英文；快照按它断言，多一句少一句都算红。
const EXPECTED_KIND_POLICY: &str = concat!(
    "You are AIOS's coordinator: you talk with the user, verify facts, and hand work to ThinAgent. ThinAgent investigates and implements.",
    "\nWhen the user wants a file changed (even a single typo), a command run, or an investigation that reads more than one or two files, call task_accept to dispatch a task; then reply in the user's language with what you dispatched and the result you expect.",
    "\nWrite the task's requirement in the user's own words, with no implementation plan or expanded scope; write its dod as the criteria for judging that requirement met.",
    "\nWrite the task's title as a short card label of at most 20 characters (Chinese characters count as one); it is the card title, so do not write a long sentence.",
    "\nIf one message contains several unrelated things, dispatch them as separate tasks, one task per thing; do not split one thing into several.",
    "\nResults arrive as task_report: do not wait or poll; finish the turn once the work is dispatched and the reply is sent.",
    "\nRead only until you can answer the user or state the task clearly (for example, confirm that a file exists); do not investigate on ThinAgent's behalf.",
    "\nWhen ThinAgent is blocked or hands back, relay briefly what it is missing or asking; the user decides, so do not make the decision for them.",
    "\nBefore dispatching a write task, stopping a task, or changing its write scope, load the aios-tasks skill first.",
    "\nWhen the user's request has steps that must happen in order (a later step needs an earlier step's result), call task_graph_propose once with the subtasks and their after links, then call propose_action once with action start_task_graph and the graph id it returned; the user confirms on the card. Sub-tasks whose upstream has a ready result start automatically: do not dispatch them again.",
    "\nDispatch tasks that are independent of each other separately with task_accept; only ordered work belongs in a task graph.",
    "\nWhen the user asks to look at something before continuing (for example \"我要先看一眼\", \"我来拍板\", \"过了再…\"), add a read-only milestone step to the graph: set milestone true on that subtask, leave write_scope empty, and make its after point at exactly the one step it checks. Only the user decides on the milestone result card; never judge, pass or fix it yourself.",
    "\nWhen a milestone card comes back needing fixes, call task_continue once for the step it checks and pass the user's words as note; do not dispatch that step again. If the user's words go beyond the original requirement, do not call task_continue: ask the user to change the requirement first.",
    "\nWhen a notice says an upstream task has a new result, call task_continue once for the downstream task it names; do not ask the user and do not dispatch it again: it only checks its existing result against the new upstream result.",
    "\nWhen the user's message carries a result card and asks for a change, call propose_action once with action update_task_requirement: put the user's own words into the new requirement and change nothing else; widening the write scope still goes through task_update.",
    "\nIf a message with a result card only asks a question about that result, answer it directly and do not show a card.",
    "\nAccepting a result is only done by the user on the card: never accept for them and never say it has been accepted.",
);

/// 装了任务执行者：Kind 第一个请求的 system 以派任务说明结尾。
#[tokio::test]
async fn the_kind_system_prompt_ends_with_the_task_policy() {
    let fixture = Fixture::new().await;
    fixture.router.push_kind(crate::common::text_sse("好"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-1", "你好")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;

    let bodies = fixture.router.bodies(false);
    assert_eq!(bodies.len(), 1, "Kind 第一个请求");
    let system = request_system(&bodies[0]);
    assert!(
        system.ends_with(EXPECTED_KIND_POLICY),
        "Kind 的系统提示要以内核策略结尾：{system}"
    );
}

/// 没装任务执行者（CLI/老测试）：不给这段说明，它提到 Kind 没有的工具。
#[tokio::test]
async fn a_root_session_without_a_task_runtime_has_no_task_policy() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut options = crate::common::options(data.path());
    options.cwd = cwd.path().to_path_buf();
    options.model.base_url = server.uri();
    options.config = AgentConfig {
        instruction_roots: vec![data.path().join("agents-home")],
        skill_roots: vec![data.path().join("skills-home")],
        ..AgentConfig::default()
    };
    let root = service::open_root_session(options).await.expect("根会话");
    let host = service::ServiceHost::new(root.agent.clone());
    router.push_kind(crate::common::text_sse("好"));
    host.send(SendRequest {
        request_id: "kind-1".to_owned(),
        content: "你好".to_owned(),
        delivery: DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
    })
    .await
    .expect("发送");
    root.agent.idle().await;

    let bodies = router.bodies(false);
    assert_eq!(bodies.len(), 1, "根会话只发一次请求");
    let system = request_system(&bodies[0]);
    assert!(
        !system.contains("You are AIOS's coordinator"),
        "没有任务执行者时不能提派任务：{system}"
    );
    assert!(
        system.starts_with("You are AIOS, a coding agent. Your working directory is "),
        "系统提示开头不动：{system}"
    );
}

#[tokio::test]
async fn task_handback_rejects_a_reason_longer_than_300_characters() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    let too_long = "长".repeat(301);
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-handback-long",
        "task_handback",
        &serde_json::json!({"reason": too_long}).to_string(),
    ));
    fixture.router.push_thin(crate::common::text_sse("没交回"));
    wait_until("超长交回的回执", || {
        let store = tasks_store(fixture.data());
        let Some(task_row) = store.task(&task.id).ok().flatten() else {
            return false;
        };
        let Some(attempt) = task_row.latest_attempt() else {
            return false;
        };
        if attempt.conversation.is_none() {
            return false;
        }
        thin_events(fixture.data(), attempt).iter().any(|event| {
            matches!(
                &event.kind,
                session::EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some("call-handback-long")
            )
        })
    })
    .await;

    let store = tasks_store(fixture.data());
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    let (text, error) = tool_result(&events, "call-handback-long");
    assert_eq!(error, Some(true), "超长理由要当工具错误：{text}");
    assert!(text.contains("300"), "{text}");
    assert!(
        !task_row
            .reports
            .iter()
            .any(|report| report.attempt_id == attempt.id && report.state == ReportState::Blocked),
        "超长理由不能写成交回报告"
    );
    assert_eq!(attempt.end, None, "没交回，Attempt 不结束");
}

#[tokio::test]
async fn a_task_frozen_with_the_eight_tool_list_still_launches() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "旧任务也能跑", None));
    let store = tasks_store(fixture.data());
    let mut tools: Vec<String> = service::THIN_TOOLS.map(str::to_owned).to_vec();
    tools.retain(|name| name != "task_handback" && !name.starts_with("task_ledger_"));
    let draft = tasks::NewTask {
        title: "升级前受理的任务".to_owned(),
        requirement: "找答案".to_owned(),
        dod: "有答案".to_owned(),
        cwd: fixture.data().to_path_buf(),
        conversation: tasks::ConversationId::new("legacy-kind-session"),
        model: tasks::ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools,
    };
    let task = store.create_task(draft, 1_700_000_000_000).unwrap();
    wait_until("升级前任务开跑", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            !task.missing.is_empty()
                || task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    assert!(
        task_row.missing.is_empty(),
        "旧任务冻结的 8 件工具不能算缺项：{:?}",
        task_row.missing
    );
    assert_eq!(
        task_row.latest_report().map(|report| report.state),
        Some(ReportState::Ready)
    );
}

#[tokio::test]
async fn a_failed_report_relays_and_leaves_the_task_open() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    let long = "文档站打不开，已经试过两次。".repeat(70);
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "failed", &long, None));
    fixture.router.push_thin(crate::common::text_sse("失败"));
    wait_until("failed relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;
    let relays = waiting_relays(fixture.data());
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "failed");
    assert_eq!(relays[0].1["task"]["stage"], "needs_conditions");
    let summary = relays[0].1["summary"].as_str().unwrap();
    assert_eq!(summary.chars().count(), 600, "摘要截到 600 字");
    assert_eq!(
        relays[0].1["chars"].as_u64(),
        Some(long.chars().count() as u64)
    );
    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    assert!(store.task(&task.id).unwrap().is_some(), "failed 不关闭任务");
}

#[tokio::test]
async fn a_relay_for_another_kind_conversation_stays_later() {
    let fixture = Fixture::new().await;
    fixture.router.push_thin(report_sse(
        1,
        "call-report",
        "ready",
        "另一个对话的答案",
        None,
    ));
    fixture.router.push_thin(crate::common::text_sse("done"));
    let store = tasks_store(fixture.data());
    let draft = tasks::NewTask {
        title: "另一个对话派的任务".to_owned(),
        requirement: "找答案".to_owned(),
        dod: "有答案".to_owned(),
        cwd: fixture.data().to_path_buf(),
        conversation: tasks::ConversationId::new("another-kind-session"),
        model: tasks::ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: service::THIN_TOOLS.map(str::to_owned).to_vec(),
    };
    let task = store.create_task(draft, 1_700_000_000_000).unwrap();
    wait_until("report ready", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        waiting_relays(fixture.data()).is_empty(),
        "发起的 Kind 会话不是当前根会话：先 Later"
    );
    let task = store.task(&task.id).unwrap().unwrap();
    let report = task.latest_report().unwrap();
    assert!(
        store
            .effect_pending(tasks::EffectKind::Relay, report.id.as_str())
            .unwrap(),
        "relay 还在等 Kind 打开"
    );
}

#[tokio::test]
async fn a_relay_interrupted_before_kind_append_is_delivered_once_after_restart() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;
    fixture.router.push_thin(report_sse(
        1,
        "call-report",
        "ready",
        "重启后只送一次的答案",
        None,
    ));
    fixture.router.push_thin(crate::common::text_sse("done"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .crash_before_next_relay();
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("report ready", || {
        let store = tasks_store(fixture.data());
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    let store = tasks_store(fixture.data());
    let report = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .unwrap()
        .id
        .clone();
    wait_until("relay left queued", || {
        store
            .effect_pending(tasks::EffectKind::Relay, report.as_str())
            .unwrap()
    })
    .await;
    assert!(waiting_relays(fixture.data()).is_empty(), "崩溃前不能送到");

    let Fixture {
        _server: server,
        data,
        cwd,
        router,
        host,
    } = fixture;
    drop(host);
    router.push_kind(crate::common::text_sse("收到"));
    let host = open_host(&server, data.path(), cwd.path()).await;
    wait_until("restart relay", || waiting_relays(data.path()).len() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(waiting_relays(data.path()).len(), 1, "重启后仍然只有一条");
    assert_eq!(
        waiting_relays(data.path())[0].1["summary"],
        "重启后只送一次的答案"
    );
    let store = tasks_store(data.path());
    assert!(
        !store
            .effect_pending(tasks::EffectKind::Relay, report.as_str())
            .unwrap()
    );
    drop(host);
}

#[tokio::test]
async fn task_stream_is_live_while_the_task_is_running() {
    let server = MockServer::start().await;
    let router = Router::new();
    // ThinAgent 的响应慢一点，让测试能在任务跑着的时候接上 live。
    router.set_thin_delay(Duration::from_millis(800));
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;

    let args = serde_json::json!({
        "title": "查一下 hello",
        "requirement": "读 hello.txt",
        "dod": "说出内容"
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));
    router.push_thin(crate::common::tool_use_sse(
        1,
        "call-read",
        "read",
        &serde_json::json!({"file_path": "hello.txt"}).to_string(),
    ));
    router.push_thin(crate::common::text_sse("查完了"));

    host.dispatch(Call::Send(send(&host, "kind-live", "查一下 hello")))
        .await
        .expect("发送");
    let store = tasks_store(data.path());
    wait_until("任务建好", || {
        store.task_ids().map(|ids| ids.len()).unwrap_or(0) == 1
    })
    .await;
    let mut ids = store.task_ids().unwrap();
    let task_id = ids.remove(0);

    let mut opened = None;
    for _ in 0..200 {
        if let Ok(stream) = host.task_stream(task_id.as_str(), 0).await
            && let Some(live) = stream.live
        {
            opened = Some((stream.follow.last_seq, live));
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (last, mut live) = opened.expect("任务在跑时要能接上 live 订阅");

    let event = tokio::time::timeout(Duration::from_secs(5), live.events.recv())
        .await
        .expect("在跑的任务要送来 live 事件")
        .expect("live 订阅还开着");
    assert!(
        event.seq > last,
        "live 事件在打开时的补齐之后：{} <= {last}",
        event.seq
    );
}

#[tokio::test]
async fn stopping_a_running_task_persists_the_stop_before_cancelling_and_relays_once() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    // 钩子观察：ThinAgent 的取消收尾写进日志时，任务库里已经能读到停止事实。
    let seen = Arc::new(Mutex::new(Vec::new()));
    let task_slot: Arc<Mutex<Option<tasks::TaskId>>> = Arc::new(Mutex::new(None));
    let hook_data = data.path().to_path_buf();
    let hook_seen = Arc::clone(&seen);
    let hook_task = Arc::clone(&task_slot);
    let hook: session::AppendHookFactory = Arc::new(move |_id| {
        let data = hook_data.clone();
        let seen = Arc::clone(&hook_seen);
        let task_slot = Arc::clone(&hook_task);
        Arc::new(move |events: &[session::Event]| {
            let aborted = events.iter().any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::TurnEnd {
                        reason: session::TurnEndReason::Aborted { .. },
                        ..
                    }
                )
            });
            if !aborted {
                return;
            }
            let Some(task_id) = task_slot.lock().unwrap().clone() else {
                return;
            };
            let Ok(store) = Store::open(data.join(service::TASKS_DB_FILE)) else {
                return;
            };
            let Ok(Some(task)) = store.task(&task_id) else {
                return;
            };
            let Some(attempt) = task.latest_attempt() else {
                return;
            };
            let key = format!("stop:{task_id}:{}", attempt.id);
            let pending = store
                .effect_pending(EffectKind::Relay, &key)
                .unwrap_or(false);
            seen.lock().unwrap().push((attempt.end, pending));
        })
    });

    let host = open_host_with(&server, data.path(), cwd.path(), |options| {
        options.append_hook_factory = Some(hook);
    })
    .await;
    // 第一轮响应慢一点：停止要在 ThinAgent 真正跑着的时候发生。
    router.set_thin_delay(Duration::from_millis(2_000));
    router.push_thin(crate::common::text_sse("我还在查"));

    let args = serde_json::json!({
        "title": "查一下",
        "requirement": "找答案",
        "dod": "有答案"
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));
    host.dispatch(Call::Send(send(&host, "kind-stop-1", "查一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;

    let store = tasks_store(data.path());
    let task = one_task(&store);
    *task_slot.lock().unwrap() = Some(task.id.clone());
    wait_until("ThinAgent 的请求在途", || {
        !router.bodies(true).is_empty()
    })
    .await;

    // 页头事实：这次 Attempt 正在跑、还没停；这决定「任务」菜单能不能点。
    let read = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(read) = read else {
        panic!("TaskRead 的响应形状不对");
    };
    assert!(
        read.task.running,
        "在跑的 Attempt 页头是执行中：{:?}",
        read.task
    );
    assert!(!read.task.stopped, "还没停下：{:?}", read.task);

    // relay 先不投递：停止后能查效果行还在排队。
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let response = host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停止");
    let Response::TaskStop(stop) = response else {
        panic!("TaskStop 的响应形状不对");
    };
    assert!(stop.stopped, "{stop:?}");
    assert_eq!(stop.stage, "needs_conditions");

    // 停下落库后页头固定：「已停止」、不再在跑；刷新也读得到。
    let read = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(read) = read else {
        panic!("TaskRead 的响应形状不对");
    };
    assert!(!read.task.running, "停了就不再跑：{:?}", read.task);
    assert!(read.task.stopped, "页头固定已停止：{:?}", read.task);

    let task = store.task(&task.id).unwrap().unwrap();
    let attempt = task.latest_attempt().expect("Attempt");
    assert_eq!(attempt.end, Some(AttemptEnd::Stopped));
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::NeedsConditions,
        "停止后阶段是待补足条件"
    );
    assert!(
        store
            .effect_pending(
                EffectKind::Relay,
                &format!("stop:{}:{}", task.id, attempt.id)
            )
            .unwrap(),
        "停止 relay 还在排队"
    );

    // 取消后的收尾：ThinAgent 最后一轮是 aborted。
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("ThinAgent 的轮收尾", || {
        session::SessionStore::open_read_only(data.path(), &session_id).is_ok_and(|store| {
            store
                .events()
                .iter()
                .any(|event| matches!(&event.kind, session::EventKind::TurnEnd { .. }))
        })
    })
    .await;
    let thin = session::SessionStore::open_read_only(data.path(), &session_id).unwrap();
    assert_eq!(
        last_turn_end(thin.events()),
        Some(session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        })
    );

    // 顺序：取消的收尾写日志时，库里停止事实已提交、relay 已排队。
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "只应观察到一次取消收尾：{seen:?}");
        assert_eq!(
            seen[0],
            (Some(AttemptEnd::Stopped), true),
            "先持久后 cancel：收尾写日志时停止已在库里"
        );
    }
    assert!(
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_report()
            .is_none(),
        "停止不合成 waiting 报告"
    );

    host.task_runtime().expect("runtime").set_pump_paused(false);
    router.push_kind(crate::common::text_sse("收到"));
    wait_until("Kind 收到停止 relay", || {
        stopped_relays(&crate::common::read_root_events(data.path())).len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let relays = stopped_relays(&crate::common::read_root_events(data.path()));
    assert_eq!(relays.len(), 1, "Kind 只收到一条停止 relay：{relays:?}");
    let body = &relays[0].1;
    assert_eq!(body["kind"], "task_report");
    assert_eq!(body["state"], "stopped");
    assert_eq!(body["origin"], "executor");
    assert_eq!(body["task"]["id"], task.id.as_str());
    assert_eq!(body["task"]["stage"], "needs_conditions");
    assert!(
        body["summary"]
            .as_str()
            .is_some_and(|text| text.contains("立即停")),
        "{body:?}"
    );
}

#[tokio::test]
async fn kind_task_stop_now_stops_the_running_attempt_like_the_page() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(2_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查"));
    let task = fixture.accept(false).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let args = serde_json::json!({"task_id": task.id.as_str(), "mode": "now"}).to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        1,
        "call-stop",
        "task_stop",
        &args,
    ));
    fixture.router.push_kind(crate::common::text_sse("停了"));
    // 停止 relay 可能在这轮之后才到，预置它唤醒的那一轮的回复。
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-stop-2", "把任务停了")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;

    let events = crate::common::read_root_events(fixture.data());
    let (text, is_error) = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. }
                if message.tool_call_id.as_deref() == Some("call-stop") =>
            {
                Some((
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                    message.is_error,
                ))
            }
            _ => None,
        })
        .expect("task_stop 的 tool/result");
    assert!(!is_error.unwrap_or(false), "{text}");
    assert!(text.contains("已立即停"), "{text}");

    let store = tasks_store(fixture.data());
    let task = store.task(&task.id).unwrap().unwrap();
    let attempt = task.latest_attempt().expect("Attempt");
    assert_eq!(attempt.end, Some(AttemptEnd::Stopped));
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("ThinAgent 的轮收尾", || {
        session::SessionStore::open_read_only(fixture.data(), &session_id).is_ok_and(|store| {
            store
                .events()
                .iter()
                .any(|event| matches!(&event.kind, session::EventKind::TurnEnd { .. }))
        })
    })
    .await;
    let thin = session::SessionStore::open_read_only(fixture.data(), &session_id).unwrap();
    assert_eq!(
        last_turn_end(thin.events()),
        Some(session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        })
    );

    wait_until("Kind 收到停止 relay", || {
        stopped_relays(&crate::common::read_root_events(fixture.data())).len() == 1
    })
    .await;
    assert_eq!(
        stopped_relays(&crate::common::read_root_events(fixture.data()))[0].1["state"],
        "stopped"
    );
}

#[tokio::test]
async fn task_stop_for_a_task_that_is_not_running_reports_the_fixed_reason() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;

    let response = fixture
        .host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停止");
    let Response::TaskStop(stop) = response else {
        panic!("TaskStop 的响应形状不对");
    };
    assert!(!stop.stopped, "{stop:?}");
    assert_eq!(stop.message, "这个任务没在跑");
    assert_eq!(stop.stage, "pending");

    let store = tasks_store(fixture.data());
    let task = store.task(&task.id).unwrap().unwrap();
    assert!(
        task.latest_attempt().is_none(),
        "没开跑的任务不该有 Attempt"
    );
}

#[tokio::test]
async fn kind_task_stop_reports_not_running_for_both_modes() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;

    let now_args = serde_json::json!({"task_id": task.id.as_str(), "mode": "now"}).to_string();
    let after_args =
        serde_json::json!({"task_id": task.id.as_str(), "mode": "after_report"}).to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        1,
        "call-stop-now",
        "task_stop",
        &now_args,
    ));
    fixture.router.push_kind(crate::common::tool_use_sse(
        2,
        "call-stop-after",
        "task_stop",
        &after_args,
    ));
    fixture.router.push_kind(crate::common::text_sse("没停成"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-stop-3", "停一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;

    let events = crate::common::read_root_events(fixture.data());
    let result = |call: &str| {
        events
            .iter()
            .find_map(|event| match &event.kind {
                session::EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some(call) =>
                {
                    Some((
                        message
                            .content
                            .iter()
                            .filter_map(model::ContentBlock::as_text)
                            .collect::<String>(),
                        message.is_error,
                    ))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{call} 的 tool/result"))
    };
    let (not_running, is_error) = result("call-stop-now");
    assert!(!is_error.unwrap_or(false), "{not_running}");
    assert_eq!(not_running, "这个任务没在跑");

    // 两种停法对没在跑的任务回同一句固定原因，都不是工具错误。
    let (after_report, is_error) = result("call-stop-after");
    assert!(!is_error.unwrap_or(false), "{after_report}");
    assert_eq!(after_report, "这个任务没在跑");
}

/// 停下这一轮：会话这一轮 aborted、Attempt 没结束、不出 relay/waiting；
/// 再说一句还是同一个 Attempt，接着原会话跑。
#[tokio::test]
async fn interrupting_a_running_turn_keeps_the_attempt_and_the_next_sentence_continues_it() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(1_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;
    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("第一轮 Attempt");

    let interrupt = interrupt_task(&fixture.host, &task).await;
    assert!(interrupt.interrupted, "{interrupt:?}");
    assert_eq!(interrupt.message, "已停下这一轮");

    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("这一轮以用户取消收尾", || {
        session::SessionStore::open_read_only(fixture.data(), &session_id).is_ok_and(|store| {
            store.events().iter().any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::TurnEnd {
                        reason: session::TurnEndReason::Aborted {
                            reason: session::AbortCause::User,
                        },
                        ..
                    }
                )
            })
        })
    })
    .await;
    let thin = session::SessionStore::open_read_only(fixture.data(), &session_id).unwrap();
    assert_eq!(
        last_turn_end(thin.events()),
        Some(session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        })
    );

    // 只停这一轮：Attempt 不结束、不合成 waiting、不排 relay。
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = store.task(&task.id).unwrap().unwrap();
    let kept = after.latest_attempt().cloned().expect("Attempt 还在");
    assert_eq!(kept.id, attempt.id);
    assert!(kept.ended_at.is_none(), "停下这一轮不写 Attempt 结束");
    assert!(
        after.latest_report().is_none(),
        "停下这一轮不合成 waiting 报告"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);
    let root_events = crate::common::read_root_events(fixture.data());
    assert!(
        relay_messages(&root_events).is_empty(),
        "停下这一轮不出结果 relay"
    );
    assert!(
        stopped_relays(&root_events).is_empty(),
        "停下这一轮不是立即停，没有停止 relay"
    );
    let read = fixture
        .host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(read) = read else {
        panic!("TaskRead 的响应形状不对");
    };
    assert!(!read.task.running, "这一轮已经停下");
    assert!(!read.task.stopped, "任务没停，只是停下这一轮");

    // 再说一句：同一 Attempt 接着做，请求里带着下一句。
    fixture.router.clear_thin_delay();
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture.router.push_thin_for(
        task.id.as_str(),
        report_sse(1, "call-report", "ready", "接着做完了", None),
    );
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收尾"));
    let said = say_in_task_dialog(&fixture.host, &task, "再接着说").await;
    assert_eq!(said.new_attempt, None, "同一个 Attempt 接着做，不开新的");
    wait_until("下一句进了任务请求", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("再接着说"))
    })
    .await;
    let resumed = store.task(&task.id).unwrap().unwrap();
    assert_eq!(attempt_count(&resumed), 1, "Attempt 数不变");
    let resumed_attempt = resumed.latest_attempt().cloned().expect("Attempt");
    assert_eq!(resumed_attempt.id, attempt.id);
    assert_eq!(
        resumed_attempt.conversation.as_ref(),
        Some(&conversation),
        "还是原会话"
    );
    wait_until("接着做的这一轮报 ready", || {
        reported_ready(&store, &task.id)
    })
    .await;
    wait_until("Kind 收到结果 relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data())).len() == 1
    })
    .await;
    let final_task = store.task(&task.id).unwrap().unwrap();
    let states: Vec<_> = final_task
        .reports
        .iter()
        .map(|report| report.state)
        .collect();
    assert_eq!(
        states,
        vec![ReportState::Ready],
        "停下这一轮没有留下 waiting 报告"
    );
}

/// 停下这一轮不丢已经排队的输入：取消前 Steer 进去的那句，下一句来时一起被领取。
#[tokio::test]
async fn interrupting_keeps_the_queued_sentence_for_the_next_turn() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(500));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;
    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("第一轮 Attempt");

    // 在跑时说一句：delivery 把它 Steer 进当前这一轮的输入队列。
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收到排队的那句"));
    let said = say_in_task_dialog(&fixture.host, &task, "排队的一句").await;
    assert_eq!(said.new_attempt, None);
    wait_until("这句话已经送进会话输入队列", || {
        !store
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap()
    })
    .await;

    let interrupt = interrupt_task(&fixture.host, &task).await;
    assert!(interrupt.interrupted, "{interrupt:?}");
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("这一轮以用户取消收尾", || {
        session::SessionStore::open_read_only(fixture.data(), &session_id).is_ok_and(|store| {
            store.events().iter().any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::TurnEnd {
                        reason: session::TurnEndReason::Aborted {
                            reason: session::AbortCause::User,
                        },
                        ..
                    }
                )
            })
        })
    })
    .await;

    // 取消不会把排队的那句冲掉：下一句进来时，两句话一起进请求；Attempt 还是同一个。
    fixture.router.clear_thin_delay();
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收到两句"));
    let said = say_in_task_dialog(&fixture.host, &task, "再接着说").await;
    assert_eq!(said.new_attempt, None, "还是同一个 Attempt");
    wait_until("下一句进来时排队的那句也一起送到", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("排队的一句") && body.contains("再接着说"))
    })
    .await;
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(attempt_count(&after), 1, "不丢输入也不开新 Attempt");
    assert_eq!(
        after.latest_attempt().map(|attempt| attempt.id),
        Some(attempt.id)
    );
}

/// 没在跑时停下这一轮：返回固定原因，不动任务。
#[tokio::test]
async fn interrupting_a_task_that_is_not_running_reports_the_fixed_reason() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;

    let interrupt = interrupt_task(&fixture.host, &task).await;
    assert!(!interrupt.interrupted, "{interrupt:?}");
    assert_eq!(interrupt.message, "这个任务没在跑");

    let store = tasks_store(fixture.data());
    let task = store.task(&task.id).unwrap().unwrap();
    assert!(
        task.latest_attempt().is_none(),
        "没开跑的任务不该多出 Attempt"
    );
    assert!(task.latest_report().is_none(), "没在跑不写报告");
}

/// 立即停仍走 T027.2：停下这一轮之后点立即停，Attempt 照样结束并排停止 relay。
#[tokio::test]
async fn stopping_now_after_an_interrupt_still_ends_the_attempt_and_relays() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(1_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;

    let interrupt = interrupt_task(&fixture.host, &task).await;
    assert!(interrupt.interrupted, "{interrupt:?}");
    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("Attempt");
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("这一轮以用户取消收尾", || {
        session::SessionStore::open_read_only(fixture.data(), &session_id).is_ok_and(|store| {
            store.events().iter().any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::TurnEnd {
                        reason: session::TurnEndReason::Aborted {
                            reason: session::AbortCause::User,
                        },
                        ..
                    }
                )
            })
        })
    })
    .await;

    fixture.router.push_kind(crate::common::text_sse("收到"));
    let stop = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stop.stopped, "{stop:?}");
    assert_eq!(
        attempt_end(&store, &task.id),
        Some(AttemptEnd::Stopped),
        "立即停照样结束 Attempt"
    );
    wait_until("Kind 收到停止 relay", || {
        stopped_relays(&crate::common::read_root_events(fixture.data())).len() == 1
    })
    .await;
}

#[tokio::test]
async fn the_tasks_skill_is_bundled_for_kind_only() {
    let fixture = Fixture::new().await;
    fixture.router.push_thin(crate::common::text_sse("查完"));
    let args = serde_json::json!({
        "title": "查一下",
        "requirement": "找答案",
        "dod": "有答案"
    })
    .to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    fixture.router.push_kind(crate::common::text_sse("已派出"));
    // ThinAgent 的 waiting relay 可能插进受理这一轮，也可能之后唤醒一轮；预置一条回复。
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-skill", "查一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    wait_until("ThinAgent 请求", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let kind_body = &fixture.router.bodies(false)[0];
    assert!(
        kind_body.contains("aios-tasks"),
        "Kind 的 skill 目录要有 aios-tasks"
    );
    assert!(
        tool_names(kind_body).contains(&"task_stop".to_owned()),
        "Kind 的工具表要有 task_stop"
    );
    let thin_body = &fixture.router.bodies(true)[0];
    assert!(
        !thin_body.contains("aios-tasks"),
        "ThinAgent 看不到 aios-tasks"
    );
}

#[tokio::test]
async fn task_stop_after_report_for_a_task_that_is_not_running_reports_the_fixed_reason() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;

    let stop = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(!stop.stopped, "{stop:?}");
    assert_eq!(stop.message, "这个任务没在跑");
    assert_eq!(stop.stage, "pending");
}

#[tokio::test]
async fn after_report_stop_injects_the_note_denies_other_tools_and_ends_reported() {
    let fixture = Fixture::new().await;
    // 收尾停要落在 ThinAgent 跑着的时候：把第一次响应拖慢，留出插进去的时机。
    fixture.router.set_thin_delay(Duration::from_millis(600));
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-read",
        "read",
        &serde_json::json!({"file_path": "not-there.txt"}).to_string(),
    ));
    fixture.router.push_thin(report_sse(
        2,
        "call-report",
        "ready",
        "查完了，答案在第二行",
        None,
    ));
    fixture.router.push_thin(crate::common::text_sse("报完了"));
    // ready relay 会 Steer 进 Kind：备用回复在受理派发前就排好。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let stop = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(stop.stopped, "{stop:?}");
    assert!(stop.message.contains("已收尾停"), "{stop:?}");

    // 插进去的固定说明在下一步请求里。
    wait_until("ThinAgent 的下一步请求", || {
        fixture.router.bodies(true).len() >= 2
    })
    .await;
    let thin_bodies = fixture.router.bodies(true);
    assert!(
        thin_bodies[1].contains(
            "用户要求收尾停：做完手上这一步，用 task_report 报一次当前结果，然后结束。不再调其他工具。"
        ),
        "下一个请求要带固定停止说明：{}",
        thin_bodies[1]
    );

    let store = tasks_store(fixture.data());
    wait_until("ready 报告", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    assert_eq!(
        attempt.end,
        Some(AttemptEnd::Reported),
        "报了以后 Attempt 以 reported 结束"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);

    // 会话日志：固定说明一条；read 先记 tool/call 再给错误 result；task_report 放行。
    let events = thin_events(fixture.data(), &attempt);
    assert_eq!(
        injected_stop_notes(&events, &task, &attempt),
        1,
        "固定说明按 request id 只插一条"
    );
    let (read_text, read_error) = tool_result(&events, "call-read");
    assert_eq!(read_error, Some(true), "{read_text}");
    assert_eq!(read_text, "Error: 已收尾停，只能用 task_report 上报");
    let call_position = events
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                session::EventKind::ToolCall { call_id, .. } if call_id == "call-read"
            )
        })
        .expect("read 的 tool/call");
    let result_position = events
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                session::EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some("call-read")
            )
        })
        .expect("read 的 tool/result");
    assert!(
        call_position < result_position,
        "先记 tool/call，再给错误的 tool/result"
    );
    let (report_text, report_error) = tool_result(&events, "call-report");
    assert_eq!(report_error, None, "task_report 放行：{report_text}");
    assert!(report_text.contains("已上报"), "{report_text}");

    // 卡照报的状态出：Kind 收到一条 ready relay，阶段是待检查。
    wait_until("Kind 收到 ready relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "ready");
    assert_eq!(relays[0].1["task"]["stage"], "ready_for_review");
}

#[tokio::test]
async fn after_report_stop_without_a_report_synthesizes_waiting_then_records_stopped() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(600));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查，别急"));
    // 插进去的说明会开下一步；这一步 ThinAgent 没报，turn 才收尾。
    fixture.router.push_thin(crate::common::text_sse("不报了"));
    // waiting relay 会 Steer 进 Kind：备用回复在受理派发前就排好。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let stop = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(stop.stopped, "{stop:?}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    let latest = task_row.latest_report().expect("waiting 报告");
    assert_eq!(latest.state, ReportState::Waiting, "没报就走 waiting 合成");
    assert_eq!(latest.summary, "不报了");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);

    wait_until("Kind 收到 waiting relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "waiting");
    assert_eq!(relays[0].1["task"]["stage"], "needs_conditions");
}

#[tokio::test]
async fn after_report_stop_on_an_idle_task_records_stopped_without_another_relay() {
    let fixture = Fixture::new().await;
    fixture.router.push_thin(crate::common::text_sse("还在查"));
    // waiting relay 会 Steer 进 Kind：备用回复在受理派发前就排好，免得撞上没脚本的请求。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("waiting relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;

    // 这一轮已经结束：收尾停落在空闲的会话上，没有下一步可等。
    let stop = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(stop.stopped, "{stop:?}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        task_row.latest_report().map(|report| report.state),
        Some(ReportState::Waiting),
        "已有的 waiting 报告就是收尾停时的事实"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        relay_messages(&crate::common::read_root_events(fixture.data())).len(),
        1,
        "没多出一条 relay"
    );
    assert!(
        fixture.router.state.unexpected.lock().unwrap().is_empty(),
        "没有请求落在脚本之外"
    );
}

#[tokio::test]
async fn stopping_after_report_twice_injects_the_fixed_note_once() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(600));
    fixture.router.push_thin(crate::common::text_sse("还在查"));
    // 插进去的说明会开下一步；这一步 ThinAgent 没报，turn 才收尾。
    fixture.router.push_thin(crate::common::text_sse("不报了"));
    // waiting relay 会 Steer 进 Kind：备用回复在受理派发前就排好。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let first = stop_task(&fixture, &task, StopMode::AfterReport).await;
    let second = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(first.stopped, "{first:?}");
    assert!(second.stopped, "{second:?}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    assert_eq!(
        injected_stop_notes(&events, &task, &attempt),
        1,
        "重复点收尾停用同一个 request id：只插一条"
    );
}

#[tokio::test]
async fn stopping_now_after_an_after_report_stop_uses_the_immediate_stop() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(2_000));
    fixture.router.push_thin(crate::common::text_sse("还在查"));
    // 立即停的 relay 会 Steer 进 Kind：备用回复在受理派发前就排好。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let after_report = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(after_report.stopped, "{after_report:?}");
    let now = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(now.stopped, "{now:?}");
    assert!(now.message.contains("已立即停"), "{now:?}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    wait_until("Kind 收到停止 relay", || {
        stopped_relays(&crate::common::read_root_events(fixture.data())).len() == 1
    })
    .await;
    let relays = stopped_relays(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays[0].1["state"], "stopped");

    // 立即停照 T027.2：会话最后一轮以取消收尾，不再合成 waiting 报告。
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    // 取消的收尾由会话驱动异步写日志：等它落盘再读，别抢在停止 relay 前面。
    wait_until("会话这一轮以取消收尾", || {
        last_turn_end(&thin_events(fixture.data(), &attempt))
            == Some(session::TurnEndReason::Aborted {
                reason: session::AbortCause::User,
            })
    })
    .await;
    let events = thin_events(fixture.data(), &attempt);
    assert_eq!(
        last_turn_end(&events),
        Some(session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        })
    );
    assert_eq!(
        task_row.latest_report().map(|report| report.state),
        None,
        "立即停不合成 waiting 报告"
    );
}

#[tokio::test]
async fn kind_task_stop_after_report_stops_after_the_next_turn() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(600));
    fixture.router.push_thin(crate::common::text_sse("还在查"));
    // 插进去的说明会开下一步；这一步 ThinAgent 没报，turn 才收尾。
    fixture.router.push_thin(crate::common::text_sse("不报了"));
    let task = fixture.accept(false).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let args = serde_json::json!({"task_id": task.id.as_str(), "mode": "after_report"}).to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        1,
        "call-stop-after",
        "task_stop",
        &args,
    ));
    fixture.router.push_kind(crate::common::text_sse("好"));
    // waiting relay 唤醒的轮、以及 relay 插进 step 的备用回复。
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-after-1", "收尾停")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;

    let events = crate::common::read_root_events(fixture.data());
    let (text, is_error) = tool_result(&events, "call-stop-after");
    assert!(!is_error.unwrap_or(false), "{text}");
    assert!(text.contains("已收尾停"), "{text}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        task_row.latest_report().map(|report| report.state),
        Some(ReportState::Waiting),
        "没报：waiting 合成后记 stopped"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
}

fn skill_sse(call_id: &str, name: &str) -> String {
    crate::common::tool_use_sse(
        0,
        call_id,
        "skill",
        &serde_json::json!({ "name": name }).to_string(),
    )
}

/// Kind 的 set_paused 确认卡：详情只有一个布尔，事实行由服务端填。
fn pause_card_sse(call_id: &str, paused: bool) -> String {
    let args = serde_json::json!({
        "title": "全局暂停",
        "brief": if paused {
            "打开以后新任务先排队，在跑的这一轮跑完。"
        } else {
            "关掉以后排队的任务按原顺序开跑。"
        },
        "options": [{
            "id": "one",
            "action": "set_paused",
            "label": if paused { "暂停新任务" } else { "继续开跑" },
            "details": { "paused": paused },
        }],
    })
    .to_string();
    crate::common::tool_use_sse(1, call_id, "propose_action", &args)
}

/// 走一轮 Kind：先把这一轮的回复排好，再发消息，等 turn 收尾。
async fn kind_turn(
    host: &RootHost,
    router: &Router,
    data: &Path,
    request_id: &str,
    text: &str,
    turns: usize,
    scripts: Vec<String>,
) {
    for script in scripts {
        router.push_kind(script);
    }
    host.dispatch(Call::Send(send(host, request_id, text)))
        .await
        .expect("发送");
    crate::common::wait_turns(host, data, turns).await;
}

async fn confirm_pause_card(host: &RootHost, call_id: &str) -> Value {
    let response = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: call_id.to_owned(),
            verb: CardVerb::Confirm,
            option: Some("one".to_owned()),

            note: None,
            report_id: None,
        }))
        .await
        .expect("确认暂停卡");
    serde_json::to_value(response).expect("确认暂停卡 json")["decision"].clone()
}

/// 从提议工具结果的 meta 里取第一项的事实行。
fn card_facts(events: &[session::Event], call_id: &str) -> Vec<Value> {
    let meta = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult {
                message,
                meta: Some(meta),
                ..
            } if message.tool_call_id.as_deref() == Some(call_id) => Some(meta.clone()),
            _ => None,
        })
        .expect("提议工具结果带 meta");
    meta["card"]["options"][0]["facts"]
        .as_array()
        .expect("事实数组")
        .clone()
}

/// 受理一个任务并返回它。
async fn accept_task(
    host: &RootHost,
    router: &Router,
    data: &Path,
    request_id: &str,
    call_id: &str,
    turns: usize,
    title: &str,
) -> Task {
    let args = serde_json::json!({
        "title": title,
        "requirement": "找答案",
        "dod": "有答案",
    })
    .to_string();
    kind_turn(
        host,
        router,
        data,
        request_id,
        "派一个任务",
        turns,
        vec![
            crate::common::tool_use_sse(0, call_id, "task_accept", &args),
            crate::common::text_sse("已派出"),
        ],
    )
    .await;
    let store = tasks_store(data);
    store
        .task_ids()
        .expect("任务 id")
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(|task| task.title == title)
        .expect("刚受理的任务")
}

/// 派一个任务并等它出现在任务库；不等 Kind 那一轮收尾。
///
/// ThinAgent 轮次结束会合成等待报告、把 relay Steer 回 Kind，Kind 那一轮可能因此多一步；
/// 只断言任务自己的 Attempt 和效果时不该被这个时序拖住。
async fn dispatch_task(
    host: &RootHost,
    router: &Router,
    data: &Path,
    request_id: &str,
    call_id: &str,
    title: &str,
) -> Task {
    let args = serde_json::json!({
        "title": title,
        "requirement": "找答案",
        "dod": "有答案",
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        call_id,
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));
    host.dispatch(Call::Send(send(host, request_id, "派一个任务")))
        .await
        .expect("发送");
    let store = tasks_store(data);
    wait_until("任务出现在任务库", || {
        store
            .task_ids()
            .map(|ids| {
                ids.iter().any(|id| {
                    store
                        .task(id)
                        .ok()
                        .flatten()
                        .is_some_and(|task| task.title == title)
                })
            })
            .unwrap_or(false)
    })
    .await;
    store
        .task_ids()
        .expect("任务 id")
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(|task| task.title == title)
        .expect("刚受理的任务")
}

/// 卡片测试的宿主：动作登记表走产品装配（set_paused、resume_task 属于 aios-tasks）。
async fn open_actions_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
    open_host_with(server, data, cwd, |options| {
        options.actions = Arc::new(service::ActionRegistry::settings(
            data.to_path_buf(),
            options.identity.clone(),
        ));
    })
    .await
}

/// 任务上报 ready 的两步回复（工具调用 + 最终回答）。
fn ready_scripts() -> [String; 2] {
    [
        crate::common::tool_use_sse(
            2,
            "call-report",
            "task_report",
            &serde_json::json!({"state": "ready", "summary": "查好了"}).to_string(),
        ),
        crate::common::text_sse("done"),
    ]
}

/// 全局暂停：Kind 用确认卡开/关。开着时派的两个任务都不起跑、排队不清；
/// 关掉后按受理顺序开跑，relay 照常送到 Kind。
#[tokio::test]
async fn global_pause_holds_new_tasks_then_runs_the_queue_in_acceptance_order() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());

    // 第一轮：Kind 加载 aios-tasks，出确认卡；事实行是 关 → 开。
    kind_turn(
        &host,
        &router,
        data.path(),
        "kind-pause",
        "暂停新任务",
        1,
        vec![
            skill_sse("call-load-tasks", "aios-tasks"),
            pause_card_sse("call-pause", true),
            crate::common::text_sse("暂停了。"),
        ],
    )
    .await;
    let events = crate::common::read_root_events(data.path());
    assert_eq!(
        card_facts(&events, "call-pause"),
        vec![serde_json::json!({ "name": "全局暂停", "value": "关 → 开" })]
    );
    let decision = confirm_pause_card(&host, "call-pause").await;
    assert_eq!(decision["phase"], "confirmed");
    assert!(
        service::settings::load(data.path()).exec.paused,
        "确认后设置文件里是开"
    );

    // 暂停时派两个任务：都只留 launch 效果，不开会话。
    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        2,
        "甲",
    )
    .await;
    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        3,
        "乙",
    )
    .await;
    assert!(router.bodies(true).is_empty(), "暂停时不能开任务会话");
    for task in [&first, &second] {
        assert!(task.latest_attempt().is_none(), "暂停时不能记 Attempt");
        assert!(
            store
                .effect_pending(EffectKind::Launch, task.id.as_str())
                .unwrap(),
            "排队留着，不取消不清队"
        );
    }
    assert!(
        relay_messages(&crate::common::read_root_events(data.path())).is_empty(),
        "没起跑就没有 relay"
    );

    // 关掉暂停；排队的按受理顺序开跑。两个任务各自的回复、两条 relay 的 Kind 轮都先排好。
    for script in ready_scripts() {
        router.push_thin_for(first.id.as_str(), script);
    }
    for script in ready_scripts() {
        router.push_thin_for(second.id.as_str(), script);
    }
    kind_turn(
        &host,
        &router,
        data.path(),
        "kind-resume",
        "继续开跑",
        4,
        vec![
            skill_sse("call-load-tasks", "aios-tasks"),
            pause_card_sse("call-resume", false),
            crate::common::text_sse("开跑了。"),
            crate::common::text_sse("收到"),
            crate::common::text_sse("收到"),
        ],
    )
    .await;
    let decision = confirm_pause_card(&host, "call-resume").await;
    assert_eq!(decision["phase"], "confirmed");
    assert!(!service::settings::load(data.path()).exec.paused);

    wait_until("两个任务都报 ready", || {
        [&first, &second].iter().all(|task| {
            store
                .task(&task.id)
                .ok()
                .flatten()
                .and_then(|task| task.latest_report().map(|report| report.state))
                == Some(ReportState::Ready)
        })
    })
    .await;
    wait_until("两条 relay 到 Kind", || {
        relay_messages(&crate::common::read_root_events(data.path())).len() == 2
    })
    .await;

    let bodies = router.bodies(true);
    assert!(
        bodies.iter().any(|body| body.contains("甲"))
            && bodies.iter().any(|body| body.contains("乙")),
        "两个任务都开了会话：{bodies:?}"
    );
    // 请求到达的先后由假模型的两个会话各自排程，不能当开跑顺序；开跑顺序看库里
    // 每次 Attempt 的开始时间（效果泵按效果表 seq 逐个领取、逐个开）。
    let started = |task: &Task| {
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .map(|attempt| attempt.started_at)
    };
    let first_started = started(&first).expect("甲有 Attempt");
    let second_started = started(&second).expect("乙有 Attempt");
    assert!(
        first_started <= second_started,
        "按受理顺序开跑：{first_started} / {second_started}"
    );
    let relays = relay_messages(&crate::common::read_root_events(data.path()));
    assert!(
        relays.iter().all(|(_, body)| body["state"] == "ready"),
        "{relays:?}"
    );
}

/// 保存在 settings.json 里的暂停在重启后生效：任务只排队，不开会话。
#[tokio::test]
async fn a_saved_pause_holds_launches_after_a_restart() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut saved = service::settings::load(data.path());
    saved.exec.paused = true;
    service::settings::save(data.path(), &saved).expect("保存暂停设置");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;

    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    assert!(task.latest_attempt().is_none());
    assert!(router.bodies(true).is_empty(), "重启后暂停仍然生效");
    let store = tasks_store(data.path());
    assert!(
        store
            .effect_pending(EffectKind::Launch, task.id.as_str())
            .unwrap(),
        "任务在排队"
    );
}

/// 全局暂停不掐断在跑的一轮：暂停时这一轮照常交出报告，relay 也照常送到 Kind；
/// 暂停期间新派的任务排队。
#[tokio::test]
async fn global_pause_lets_the_running_turn_finish_and_relays_still_arrive() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    // 让 ThinAgent 的回复慢到暂停确认之后，好在「这一轮在跑」时按下暂停。
    router.set_thin_delay(Duration::from_millis(1_500));
    for script in ready_scripts() {
        router.push_thin(script);
    }

    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    wait_until("ThinAgent 的请求在途", || {
        !router.bodies(true).is_empty()
    })
    .await;

    kind_turn(
        &host,
        &router,
        data.path(),
        "kind-pause",
        "暂停新任务",
        2,
        vec![
            skill_sse("call-load-tasks", "aios-tasks"),
            pause_card_sse("call-pause", true),
            crate::common::text_sse("暂停了。"),
            // 这一轮的 relay 会 Steer 进 Kind，备一条回复。
            crate::common::text_sse("收到"),
        ],
    )
    .await;
    let decision = confirm_pause_card(&host, "call-pause").await;
    assert_eq!(decision["phase"], "confirmed");

    // 确认暂停时这一轮还没交出报告：暂停没有取消它。
    let in_flight = store.task(&first.id).unwrap().unwrap();
    assert!(
        in_flight
            .latest_attempt()
            .and_then(|attempt| attempt.conversation.clone())
            .is_some(),
        "任务会话还开着"
    );
    assert_eq!(attempt_end(&store, &first.id), None, "这一轮还没结束");

    wait_until("Kind 收到 relay", || {
        relay_messages(&crate::common::read_root_events(data.path())).len() == 1
    })
    .await;
    assert!(
        service::settings::load(data.path()).exec.paused,
        "relay 到达时暂停还开着"
    );
    assert_eq!(attempt_end(&store, &first.id), Some(AttemptEnd::Reported));
    assert_eq!(
        store
            .task(&first.id)
            .unwrap()
            .unwrap()
            .latest_report()
            .map(|report| report.state),
        Some(ReportState::Ready)
    );

    // 暂停期间再派一个任务：照样排队，不开会话。
    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        4,
        "乙",
    )
    .await;
    assert!(second.latest_attempt().is_none());
    assert!(
        store
            .effect_pending(EffectKind::Launch, second.id.as_str())
            .unwrap(),
        "暂停期间的任务在排队"
    );
    assert!(
        !router.bodies(true).iter().any(|body| body.contains("乙")),
        "暂停期间没有第二个任务会话"
    );
}

/// 任务最近一份报告是不是 ready。
fn reported_ready(store: &Store, task_id: &tasks::TaskId) -> bool {
    store
        .task(task_id)
        .ok()
        .flatten()
        .and_then(|task| task.latest_report().map(|report| report.state))
        == Some(ReportState::Ready)
}

/// 任务当前 Attempt 还开着，而且已经有任务会话。
fn attempt_in_flight(store: &Store, task_id: &tasks::TaskId) -> bool {
    let Ok(Some(task)) = store.task(task_id) else {
        return false;
    };
    task.latest_attempt()
        .is_some_and(|attempt| attempt.ended_at.is_none() && attempt.conversation.is_some())
}

/// 设置 › 执行 的暂停入口（SettingsUpdate）真的关闸：开着时派的任务只排队、不开会话；
/// 关掉后按排队顺序开跑。不是只写 settings.json。
#[tokio::test]
async fn settings_update_pauses_launch_admission() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());

    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        paused: Some(true),
        ..Default::default()
    }))
    .await
    .expect("暂停");

    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    assert!(task.latest_attempt().is_none(), "暂停时不开跑");
    assert!(
        store
            .effect_pending(EffectKind::Launch, task.id.as_str())
            .unwrap(),
        "暂停时 launch 留在队列里"
    );
    assert!(router.bodies(true).is_empty(), "暂停时不开任务会话");

    for script in ready_scripts() {
        router.push_thin_for(task.id.as_str(), script);
    }
    router.push_kind(crate::common::text_sse("收到"));
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        paused: Some(false),
        ..Default::default()
    }))
    .await
    .expect("开闸");
    wait_until("开闸后任务开跑", || {
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
}

/// 席位 1：后两个任务在各自前一个报完前只排队，排队不算效果失败；
/// 释放后按受理顺序开跑。席位从 settings.json 重启读回，再进 launch 准入。
#[tokio::test]
async fn seats_one_holds_the_second_task_until_the_first_reports() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut saved = service::settings::load(data.path());
    saved.exec.seats = 1;
    service::settings::save(data.path(), &saved).expect("保存席位");
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    router.set_thin_delay(Duration::from_millis(1_500));
    for title in ["甲", "乙", "丙"] {
        for script in ready_scripts() {
            router.push_thin_for_title(title, script);
        }
    }

    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    wait_until("第一个任务开跑", || {
        store
            .task(&first.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;

    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        2,
        "乙",
    )
    .await;
    assert!(
        second.latest_attempt().is_none(),
        "席位满时第二个任务只排队"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, second.id.as_str())
            .unwrap(),
        "第二个任务的 launch 还在队列里"
    );
    assert_eq!(
        store.effect_health(EffectKind::Launch).unwrap().attempts,
        0,
        "满席位返回 Later，不算效果失败"
    );
    assert!(
        !router.bodies(true).iter().any(|body| body.contains("乙")),
        "第二个任务还没开会话"
    );

    let third = accept_task(
        &host,
        &router,
        data.path(),
        "kind-c",
        "call-accept-c",
        3,
        "丙",
    )
    .await;
    assert!(
        third.latest_attempt().is_none(),
        "席位 1 时第三个任务也排队"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, third.id.as_str())
            .unwrap(),
        "第三个任务的 launch 还在队列里"
    );
    assert!(
        !router.bodies(true).iter().any(|body| body.contains("丙")),
        "第三个任务还没开会话"
    );

    for _ in 0..3 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    // 第一个任务的回复已经在途（延迟在请求时定住）；后面的任务不必再慢。
    router.clear_thin_delay();
    wait_until("第一个任务报 ready", || {
        reported_ready(&store, &first.id)
    })
    .await;
    let first_report_at = store
        .task(&first.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .map(|report| report.at)
        .expect("第一个任务的报告");
    wait_until("第二个任务开跑", || {
        store
            .task(&second.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
    let second_started_at = store
        .task(&second.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .map(|attempt| attempt.started_at)
        .expect("第二个任务开跑");
    assert!(
        second_started_at >= first_report_at,
        "第二个任务在第一个报完之后才开：{second_started_at} / {first_report_at}"
    );
    wait_until("第二个任务报 ready", || {
        reported_ready(&store, &second.id)
    })
    .await;
    let second_report_at = store
        .task(&second.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .map(|report| report.at)
        .expect("第二个任务的报告");
    wait_until("第三个任务开跑", || {
        store
            .task(&third.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
    let third_started_at = store
        .task(&third.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .map(|attempt| attempt.started_at)
        .expect("第三个任务开跑");
    assert!(
        third_started_at >= second_report_at,
        "释放后按受理顺序开跑：{third_started_at} / {second_report_at}"
    );
    wait_until("三个任务都报 ready", || {
        reported_ready(&store, &first.id)
            && reported_ready(&store, &second.id)
            && reported_ready(&store, &third.id)
    })
    .await;
    wait_until("三条 relay 到 Kind", || {
        relay_messages(&crate::common::read_root_events(data.path())).len() == 3
    })
    .await;
    assert_eq!(attempt_end(&store, &second.id), Some(AttemptEnd::Reported));
    assert_eq!(attempt_end(&store, &third.id), Some(AttemptEnd::Reported));
}

/// 改小席位不掐断在跑的任务，只影响之后的准入：两个在跑的照常报完，
/// 改小后派的第三个按新上限排队。
#[tokio::test]
async fn lowering_seats_does_not_stop_running_tasks_and_gates_the_next_launch() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    router.set_thin_delay(Duration::from_millis(900));
    for title in ["甲", "乙", "丙"] {
        for script in ready_scripts() {
            router.push_thin_for_title(title, script);
        }
    }

    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        2,
        "乙",
    )
    .await;
    wait_until("两个任务都开跑", || {
        store
            .task(&first.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
            && store
                .task(&second.id)
                .unwrap()
                .unwrap()
                .latest_attempt()
                .is_some()
    })
    .await;

    host.set_exec_seats(1).expect("改小席位");
    assert!(
        attempt_in_flight(&store, &first.id) && attempt_in_flight(&store, &second.id),
        "改小席位不掐断在跑的两个任务"
    );

    let third = accept_task(
        &host,
        &router,
        data.path(),
        "kind-c",
        "call-accept-c",
        3,
        "丙",
    )
    .await;
    assert!(
        third.latest_attempt().is_none(),
        "改小后新任务按 1 个席位准入"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, third.id.as_str())
            .unwrap(),
        "第三个任务在排队"
    );

    for _ in 0..3 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    wait_until("在跑的两个都报 ready", || {
        reported_ready(&store, &first.id) && reported_ready(&store, &second.id)
    })
    .await;
    wait_until("最后一个任务开跑", || {
        store
            .task(&third.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
    wait_until("三个任务都报 ready", || {
        reported_ready(&store, &first.id)
            && reported_ready(&store, &second.id)
            && reported_ready(&store, &third.id)
    })
    .await;
    wait_until("三条 relay 到 Kind", || {
        relay_messages(&crate::common::read_root_events(data.path())).len() == 3
    })
    .await;
    assert_eq!(attempt_end(&store, &first.id), Some(AttemptEnd::Reported));
    assert_eq!(attempt_end(&store, &second.id), Some(AttemptEnd::Reported));
}

/// 席位只收 1–16：越界拒存，已有的值不动。
#[tokio::test]
async fn set_exec_seats_accepts_one_through_sixteen_and_rejects_outside() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;

    host.set_exec_seats(1).expect("保存 1 个席位");
    assert_eq!(service::settings::load(data.path()).exec.seats, 1);
    host.set_exec_seats(16).expect("保存 16 个席位");
    assert_eq!(service::settings::load(data.path()).exec.seats, 16);

    for seats in [0_u8, 17] {
        let error = host.set_exec_seats(seats).expect_err("越界拒存");
        assert!(error.to_string().contains("1–16"), "{error}");
        assert_eq!(
            service::settings::load(data.path()).exec.seats,
            16,
            "拒存不改已保存的值"
        );
    }
}

/// 设置写不进去时只报错、不改内存闸门：暂停没生效，任务照常开跑。
#[tokio::test]
async fn a_failed_settings_write_keeps_the_pause_gate_open() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 设置文件本身可读；临时文件路径被目录占住，原子写一定失败（不靠权限位）。
    let saved = service::settings::load(data.path());
    service::settings::save(data.path(), &saved).expect("先写一份可读的设置");
    std::fs::create_dir(data.path().join("settings.json.tmp")).expect("占住临时文件路径");
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    router.push_thin_for_title("甲", crate::common::text_sse("还没上报"));

    let error = host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            paused: Some(true),
            ..Default::default()
        }))
        .await
        .expect_err("写盘失败要报错");
    assert!(error.to_string().contains("设置没保存"), "{error}");
    assert!(
        !service::settings::load(data.path()).exec.paused,
        "文件没被改"
    );

    // 内存里也没改成暂停：写盘失败后派的第一个任务照常开跑。
    let task = dispatch_task(&host, &router, data.path(), "kind-a", "call-accept-a", "甲").await;
    wait_until("写盘失败后任务照常开跑", || {
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
}

/// 席位写不进去时只报错、不改内存上限：还是 1 个席位，第二个任务排队。
#[tokio::test]
async fn a_failed_settings_write_keeps_the_seat_limit() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut saved = service::settings::load(data.path());
    saved.exec.seats = 1;
    service::settings::save(data.path(), &saved).expect("先保存 1 个席位");
    std::fs::create_dir(data.path().join("settings.json.tmp")).expect("占住临时文件路径");
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    router.set_thin_delay(Duration::from_millis(3_000));
    router.push_thin_for_title("甲", crate::common::text_sse("还没上报"));

    let error = host.set_exec_seats(2).expect_err("写盘失败要报错");
    assert!(error.to_string().contains("设置没保存"), "{error}");
    assert_eq!(
        service::settings::load(data.path()).exec.seats,
        1,
        "文件没被改"
    );

    let first = dispatch_task(&host, &router, data.path(), "kind-a", "call-accept-a", "甲").await;
    wait_until("第一个任务开跑", || {
        store
            .task(&first.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
    let second = dispatch_task(&host, &router, data.path(), "kind-b", "call-accept-b", "乙").await;
    assert!(
        second.latest_attempt().is_none(),
        "内存上限还是 1 个席位，第二个任务只排队"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, second.id.as_str())
            .unwrap(),
        "第二个任务的 launch 留在队列里"
    );
}

/// 在任务对话里说一句（和页面同一个调用）。
async fn say_in_task_dialog(host: &RootHost, task: &Task, text: &str) -> service::TaskSayResponse {
    let response = host
        .dispatch(Call::TaskSay(service::TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: text.to_owned(),
            pastes: Vec::new(),
            attachments: Vec::new(),
        }))
        .await
        .expect("任务对话里说一句");
    let Response::TaskSay(said) = response else {
        panic!("TaskSay 的响应形状不对");
    };
    said
}

fn attempt_count(task: &Task) -> usize {
    task.runs.iter().map(|run| run.attempts.len()).sum()
}

/// 受理一个任务并跑完第一轮 ready 报告；说话类测试的起点。
async fn first_report_fixture(fixture: &Fixture, kind_spares: usize) -> Task {
    fixture.router.push_thin_for_title(
        "查一下",
        report_sse(2, "call-report", "ready", "第一轮答案", None),
    );
    fixture
        .router
        .push_thin_for_title("查一下", crate::common::text_sse("第一轮收尾"));
    let task = fixture.accept_with_kind_replies(false, kind_spares).await;
    let store = tasks_store(fixture.data());
    wait_until("第一轮报 ready", || reported_ready(&store, &task.id)).await;
    wait_until("Kind 收到第一轮 relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    // 给 relay 叫醒的 Kind 轮一点时间收尾，之后说话时 Kind 应当是空闲的。
    tokio::time::sleep(Duration::from_millis(200)).await;
    task
}

/// 停/结束后在任务对话里说话：接在原会话上开新 Attempt，请求带着原来的历史。
#[tokio::test]
async fn saying_after_a_report_resumes_the_same_conversation_and_carries_history() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    for _ in 0..3 {
        fixture.router.push_kind(crate::common::text_sse("收到"));
    }
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "第一轮的答案", None));
    fixture
        .router
        .push_thin(crate::common::text_sse("第一轮收尾"));
    let store = tasks_store(fixture.data());
    wait_until("第一轮报 ready", || reported_ready(&store, &task.id)).await;

    let before = store.task(&task.id).unwrap().unwrap();
    let first = before.latest_attempt().cloned().expect("第一轮 Attempt");
    let conversation = first.conversation.clone().expect("会话 id");
    let attempts_before = attempt_count(&before);

    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("接着做"));
    let said = say_in_task_dialog(&fixture.host, &task, "再接着找").await;
    assert_eq!(
        said.new_attempt,
        Some(first.id.0 + 1),
        "回执预览下一次 Attempt"
    );

    wait_until("接着做开了新 Attempt", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) > attempts_before)
    })
    .await;
    let after = store.task(&task.id).unwrap().unwrap();
    let second = after.latest_attempt().expect("第 2 次 Attempt");
    assert_eq!(second.id.0, first.id.0 + 1);
    assert_eq!(
        second.conversation.as_ref(),
        Some(&conversation),
        "会话 id 不变"
    );
    assert_eq!(second.trigger, tasks::AttemptTrigger::UserMessage);
    assert!(
        second.ended_at.is_none(),
        "新 Attempt 还没结束（{:?} / {:?}）",
        second.end,
        store.effect_health(EffectKind::Delivery).unwrap()
    );

    wait_until("接着做的请求带上新句子", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("再接着找"))
    })
    .await;
    let resumed = fixture
        .router
        .bodies(true)
        .last()
        .cloned()
        .expect("接着做的请求");
    assert!(
        resumed.contains("第一轮的答案"),
        "接着做的请求带着原来的历史"
    );
}

/// 席位 1 时 person 叫醒能用多出来的 1 个席位；第二个停任务按 Later 留在库里。
#[tokio::test]
async fn a_person_wake_uses_the_extra_seat_and_the_second_stopped_task_waits() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    for title in ["甲", "乙", "丙"] {
        for script in ready_scripts() {
            router.push_thin_for_title(title, script);
        }
    }
    // 派发先压住：三次受理的 Kind 轮次会互相挤脚本，脚本都排好了再一起开跑。
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        2,
        "乙",
    )
    .await;
    let third = accept_task(
        &host,
        &router,
        data.path(),
        "kind-c",
        "call-accept-c",
        3,
        "丙",
    )
    .await;
    for _ in 0..3 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.task_runtime().expect("runtime").set_pump_paused(false);
    wait_until("三个任务都报 ready", || {
        reported_ready(&store, &first.id)
            && reported_ready(&store, &second.id)
            && reported_ready(&store, &third.id)
    })
    .await;

    // 报告那一轮之后执行者还要写最后一段回答；那段没跑完，会话写者不放开，叫醒的投递会一直 Later。
    for task in [&first, &second, &third] {
        let attempt = store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .cloned()
            .expect("报告过的 Attempt");
        wait_until("报告那一轮收尾", || {
            session_turn_ended(data.path(), &attempt)
        })
        .await;
    }

    host.set_exec_seats(1).expect("席位改成 1");
    // 甲、乙被叫醒后的这两轮要一直挂在跑：席位在 turn 结束就放（turn_ended），
    // 丙的那句必须落进「两个人都在跑」的窗口。泵串行开两个会话本身可能要 1–2s，
    // 用固定 1.5s 延迟时丙会合法地占上空出来的席位（并行跑偶发红）；这里挂 30s，
    // 收尾直接立刻停掉这两轮，不等延迟。
    router.set_thin_delay(Duration::from_secs(30));
    router.push_thin_for(first.id.as_str(), crate::common::text_sse("接着做甲"));
    router.push_thin_for(second.id.as_str(), crate::common::text_sse("接着做乙"));

    let said = say_in_task_dialog(&host, &first, "甲接着做").await;
    assert_eq!(said.new_attempt, Some(2));
    wait_until("甲被叫醒", || attempt_in_flight(&store, &first.id)).await;

    let said = say_in_task_dialog(&host, &second, "乙接着做").await;
    assert_eq!(said.new_attempt, Some(2));
    wait_until("乙用多出来的席位起来", || {
        attempt_in_flight(&store, &second.id)
    })
    .await;

    let third_before = store.task(&third.id).unwrap().unwrap();
    let said = say_in_task_dialog(&host, &third, "丙接着做").await;
    assert_eq!(said.new_attempt, Some(2), "回执只是预览，不代表真开");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let third_after = store.task(&third.id).unwrap().unwrap();
    assert_eq!(
        attempt_count(&third_after),
        attempt_count(&third_before),
        "席位满的丙不开新 Attempt"
    );
    assert!(
        third_after
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_some()),
        "丙还停在上一轮结束的状态"
    );
    assert!(
        store
            .effect_pending(EffectKind::Delivery, third.id.as_str())
            .unwrap(),
        "丙的 delivery 留在库里等席位腾出来"
    );
    assert_eq!(
        store.effect_health(EffectKind::Delivery).unwrap().attempts,
        0,
        "满席位是 Later，不算效果失败"
    );

    // 收尾：立刻停掉甲、乙，把挂着的模型请求掐掉，不等 30s 的延迟。
    for _ in 0..2 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    for task in [&first, &second] {
        host.dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停止");
    }
    wait_until("甲、乙停下", || {
        attempt_end(&store, &first.id) == Some(AttemptEnd::Stopped)
            && attempt_end(&store, &second.id) == Some(AttemptEnd::Stopped)
    })
    .await;
}

/// 立即停之后在任务对话里说话：新 Attempt 接在原会话上，请求带着停之前的历史。
#[tokio::test]
async fn saying_after_a_stop_resumes_the_same_conversation() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;
    // 第一轮一直挂着，好在跑着的时候立即停。
    fixture.router.set_thin_delay(Duration::from_millis(800));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;
    fixture.router.push_kind(crate::common::text_sse("收到"));
    let stop = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stop.stopped, "{stop:?}");
    fixture.router.clear_thin_delay();
    wait_until("第一轮停住", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let stopped = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("停住的 Attempt");
    let conversation = stopped.conversation.clone().expect("会话 id");

    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("接着做"));
    let said = say_in_task_dialog(&fixture.host, &task, "停后再说一句").await;
    assert_eq!(said.new_attempt, Some(stopped.id.0 + 1));
    wait_until("停后说话开了新 Attempt", || {
        attempt_end(&store, &task.id).is_none()
    })
    .await;
    let resumed = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("新 Attempt");
    assert_eq!(resumed.id.0, stopped.id.0 + 1);
    assert_eq!(
        resumed.conversation.as_ref(),
        Some(&conversation),
        "停后接着做还是原会话"
    );
    assert_eq!(resumed.trigger, tasks::AttemptTrigger::UserMessage);
    assert!(resumed.ended_at.is_none());

    wait_until("停后的请求带上新句子", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("停后再说一句"))
    })
    .await;
    let last = fixture
        .router
        .bodies(true)
        .last()
        .cloned()
        .expect("停后的请求");
    assert!(last.contains("找答案"), "停后的请求带着原来的历史");
}

/// 在跑的时候说一句：Steer 进下一步，Attempt 数不变。
#[tokio::test]
async fn saying_while_running_steers_the_next_step_without_a_new_attempt() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;
    fixture.router.set_thin_delay(Duration::from_millis(1_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("第一步收尾"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;
    let attempts_before = attempt_count(&store.task(&task.id).unwrap().unwrap());

    let said = say_in_task_dialog(&fixture.host, &task, "插一句").await;
    assert_eq!(said.new_attempt, None, "在跑时说话不预览新 Attempt");

    wait_until("下一步请求带上这句话", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("插一句"))
    })
    .await;
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        attempt_count(&after),
        attempts_before,
        "在跑时说话不开新 Attempt"
    );
    assert!(
        after
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none()),
        "还是同一个 Attempt 在跑"
    );
}

/// 全局暂停时说的这句压着不送（不算失败），开闸后按原输入送进原会话。
#[tokio::test]
async fn a_paused_task_dialog_holds_the_input_until_the_gate_opens() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "第一轮的答案", None));
    fixture
        .router
        .push_thin(crate::common::text_sse("第一轮收尾"));
    for _ in 0..2 {
        fixture.router.push_kind(crate::common::text_sse("收到"));
    }
    let store = tasks_store(fixture.data());
    wait_until("第一轮报 ready", || reported_ready(&store, &task.id)).await;
    let attempts_before = attempt_count(&store.task(&task.id).unwrap().unwrap());

    fixture
        .host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            paused: Some(true),
            ..Default::default()
        }))
        .await
        .expect("暂停");
    let said = say_in_task_dialog(&fixture.host, &task, "暂停时说的").await;
    assert_eq!(said.new_attempt, Some(attempts_before as u64 + 1));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let held = store.task(&task.id).unwrap().unwrap();
    assert_eq!(attempt_count(&held), attempts_before, "暂停时不接着开");
    assert!(
        store
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap(),
        "delivery 留在库里等开闸"
    );
    assert!(
        !fixture
            .router
            .bodies(true)
            .iter()
            .any(|body| body.contains("暂停时说的")),
        "暂停时这句话没送进任务会话"
    );

    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("开闸后接着做"));
    fixture
        .host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            paused: Some(false),
            ..Default::default()
        }))
        .await
        .expect("开闸");
    wait_until("开闸后开新 Attempt", || {
        attempt_count(&store.task(&task.id).unwrap().unwrap()) > attempts_before
    })
    .await;
    wait_until("开闸后请求带上这句话", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("暂停时说的"))
    })
    .await;
}

/// 抄送正文照 spec：{kind: task_cc, task:{id,title}, text<=600} + chars；
/// 用 Inject 进 Kind 日志但不叫醒它；全局暂停也照送。
#[tokio::test]
async fn a_task_cc_carries_the_task_and_truncated_text_without_waking_kind() {
    let fixture = Fixture::new().await;
    let task = first_report_fixture(&fixture, 2).await;
    fixture
        .host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            paused: Some(true),
            ..Default::default()
        }))
        .await
        .expect("暂停");
    let kind_before = fixture.router.bodies(false).len();

    // 650 字：正文截到 600，chars 报原始字数；头尾不同的字便于确认截的是头。
    let long = format!("前{}后", "中".repeat(648));
    assert_eq!(long.chars().count(), 650);
    let said = say_in_task_dialog(&fixture.host, &task, &long).await;
    let request_id = format!("task-cc:{}", said.input_id);
    wait_until("Kind 日志里有 task-cc", || {
        cc_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(id, _)| id == &request_id)
    })
    .await;

    let events = crate::common::read_root_events(fixture.data());
    let body = cc_messages(&events)
        .into_iter()
        .find(|(id, _)| id == &request_id)
        .map(|(_, body)| body)
        .expect("task-cc 正文");
    assert_eq!(body["kind"], "task_cc");
    assert_eq!(body["task"]["id"], task.id.as_str());
    assert_eq!(body["task"]["title"], "查一下");
    assert_eq!(
        body["text"].as_str().unwrap(),
        format!("前{}", "中".repeat(599)),
        "正文最多 600 字"
    );
    assert_eq!(body["chars"], 650, "超出的字数单独报");
    assert!(
        tasks_store(fixture.data())
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap(),
        "全局暂停时 delivery 压着"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fixture.router.bodies(false).len(),
        kind_before,
        "Inject 不唤醒 Kind：没有新请求"
    );
    assert!(
        !crate::common::read_root_events(fixture.data())
            .iter()
            .any(|event| matches!(
                &event.kind,
                session::EventKind::UserMessage { message, .. }
                    if matches!(
                        &message.source,
                        model::MessageSource::User { rpc_id: Some(id) } if id == &request_id
                    )
            )),
        "没被叫醒，消息还压在 Kind 的下一步队列里"
    );

    // 另一条普通消息叫醒 Kind：请求里要带着这条抄送。
    fixture
        .router
        .push_kind(crate::common::text_sse("看到抄送了"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-2", "叫醒一下")))
        .await
        .expect("叫醒 Kind");
    wait_until("叫醒后的请求带着抄送", || {
        fixture
            .router
            .bodies(false)
            .last()
            .is_some_and(|body| body.contains("task_cc"))
    })
    .await;
    wait_until(
        "抄送被领取后变成 Kind 日志里的用户消息",
        || {
            crate::common::read_root_events(fixture.data())
                .iter()
                .any(|event| {
                    matches!(
                        &event.kind,
                        session::EventKind::UserMessage { message, .. }
                            if matches!(
                                &message.source,
                                model::MessageSource::User { rpc_id: Some(id) } if id == &request_id
                            )
                    )
                })
        },
    )
    .await;
}

/// 发起的 Kind 会话不是当前根会话：notice 按 Later 留在库里，不投送、不算失败。
#[tokio::test]
async fn a_task_cc_stays_later_when_its_kind_conversation_is_not_the_root() {
    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let task = store
        .create_task(
            tasks::NewTask {
                title: "另一个对话派的任务".to_owned(),
                requirement: "找答案".to_owned(),
                dod: "有答案".to_owned(),
                cwd: fixture.data().to_path_buf(),
                conversation: tasks::ConversationId::new("another-kind-session"),
                model: tasks::ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: service::THIN_TOOLS.map(str::to_owned).to_vec(),
            },
            1_700_000_000_000,
        )
        .unwrap();

    let said = say_in_task_dialog(&fixture.host, &task, "给另一个 Kind 的抄送").await;
    let key = format!("cc:{}", said.input_id);
    wait_until("notice 留在库里", || {
        store.effect_pending(EffectKind::Notice, &key).unwrap()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        cc_messages(&crate::common::read_root_events(fixture.data())).is_empty(),
        "发起的 Kind 不是当前根会话：不投送"
    );
    let health = store.effect_health(EffectKind::Notice).unwrap();
    assert_eq!(health.state, EffectState::Ready);
    assert_eq!(health.attempts, 0, "Later 不算失败");
}

/// 任务对话的附件：校验同 Kind（坏 id 拒收），投递到 ThinAgent 时文字块和文件块都在。
#[tokio::test]
async fn a_task_say_attachment_is_validated_and_lands_in_the_thin_request() {
    let fixture = Fixture::new().await;
    let task = first_report_fixture(&fixture, 3).await;
    let store = tasks_store(fixture.data());

    let missing = SendAttachment {
        id: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned(),
        name: "missing.txt".to_owned(),
        bytes: 1,
    };
    let error = fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "看附件".to_owned(),
            pastes: Vec::new(),
            attachments: vec![missing],
        }))
        .await
        .expect_err("不存在的附件要拒收");
    assert_eq!(error.code().as_str(), "attachment/invalid");
    assert_eq!(
        store.task(&task.id).unwrap().unwrap().inputs.len(),
        0,
        "拒收不改任务行"
    );

    let put = fixture
        .host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "notes.txt".to_owned(),
            data: "aGVsbG8=".to_owned(),
        }))
        .await
        .expect("存附件");
    let put = serde_json::to_value(&put).unwrap();
    let attachment_id = put["id"].as_str().unwrap().to_owned();
    let bytes = put["bytes"].as_u64().unwrap();

    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收到附件"));
    fixture.router.push_kind(crate::common::text_sse("收到"));
    let response = fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "看附件".to_owned(),
            pastes: Vec::new(),
            attachments: vec![SendAttachment {
                id: attachment_id.clone(),
                name: "notes.txt".to_owned(),
                bytes,
            }],
        }))
        .await
        .expect("带附件说一句");
    let Response::TaskSay(said) = response else {
        panic!("TaskSay 的响应形状不对");
    };

    wait_until("ThinAgent 请求带着附件", || {
        fixture.router.bodies(true).iter().any(|body| {
            body.contains("看附件")
                && body.contains("notes.txt")
                && body.contains("verbatim read-only copy saved at")
        })
    })
    .await;

    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("接着做的 Attempt");
    let request_id = format!("task-say:{}", said.input_id);
    let message = thin_events(fixture.data(), &attempt)
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => match &message.source {
                model::MessageSource::User {
                    rpc_id: Some(rpc_id),
                } if rpc_id == &request_id => Some(message.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("任务会话里有这条输入");
    assert!(
        message
            .content
            .iter()
            .any(|block| block.as_text() == Some("看附件"))
    );
    let (id, name, file_bytes, path) = message
        .content
        .iter()
        .find_map(|block| match block {
            model::ContentBlock::File {
                attachment_id,
                name,
                bytes,
                path,
            } => Some((attachment_id.clone(), name.clone(), *bytes, path.clone())),
            _ => None,
        })
        .expect("输入里带着附件块");
    assert_eq!(id, attachment_id);
    assert_eq!(name, "notes.txt");
    assert_eq!(file_bytes, bytes);
    assert!(
        path.as_deref()
            .is_some_and(|path| path.contains("attachments/v1/files")),
        "附件走只读副本路径：{path:?}"
    );
}

/// 只有粘贴、没有文字也算一句：ThinAgent 的请求带着粘贴原文，任务会话里是 paste 块。
#[tokio::test]
async fn a_paste_only_task_say_lands_as_a_paste_block() {
    let fixture = Fixture::new().await;
    let task = first_report_fixture(&fixture, 2).await;
    let store = tasks_store(fixture.data());
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收到粘贴"));
    fixture.router.push_kind(crate::common::text_sse("收到"));

    let response = fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: String::new(),
            pastes: vec!["只有粘贴的一段".to_owned()],
            attachments: Vec::new(),
        }))
        .await
        .expect("只有粘贴也算一句");
    let Response::TaskSay(said) = response else {
        panic!("TaskSay 的响应形状不对");
    };

    wait_until("ThinAgent 请求带着粘贴", || {
        fixture
            .router
            .bodies(true)
            .iter()
            .any(|body| body.contains("只有粘贴的一段"))
    })
    .await;

    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("接着做的 Attempt");
    let request_id = format!("task-say:{}", said.input_id);
    let message = thin_events(fixture.data(), &attempt)
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => match &message.source {
                model::MessageSource::User {
                    rpc_id: Some(rpc_id),
                } if rpc_id == &request_id => Some(message.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("任务会话里有这条输入");
    let paste = message
        .content
        .iter()
        .find_map(|block| match block {
            model::ContentBlock::Text {
                text, paste: true, ..
            } => Some(text.clone()),
            _ => None,
        })
        .expect("输入里是粘贴块，不是普通文字");
    assert_eq!(paste, "只有粘贴的一段");
    assert!(
        message
            .content
            .iter()
            .all(|block| !matches!(block, model::ContentBlock::Text { paste: false, .. })),
        "没有文字块：这句话只有粘贴"
    );
}

/// 同一条输入的 delivery 再跑一次（重试/重领）：request id 判重，会话里只留一条。
#[tokio::test]
async fn a_delivery_retry_does_not_repeat_the_same_input() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;
    fixture.router.set_thin_delay(Duration::from_millis(2_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("第一步收尾"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;

    let said = say_in_task_dialog(&fixture.host, &task, "只该出现一次的话").await;
    wait_until("下一步请求带上这句话", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("只该出现一次的话"))
    })
    .await;

    let runtime = fixture.host.task_runtime().expect("runtime");
    runtime
        .store()
        .enqueue_effect(
            tasks::EffectPayload::Delivery {
                task_id: task.id.clone(),
            },
            0,
        )
        .unwrap();
    runtime.set_pump_paused(false);
    wait_until("重跑的 delivery 收掉", || {
        !store
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap()
    })
    .await;

    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("Attempt");
    let request_id = format!("task-say:{}", said.input_id);
    let events = thin_events(fixture.data(), &attempt);
    // 一条消息在日志里会出现两次（inbox/spliced 插入 + 领取后的 user/message）；
    // 按稳定 message id 去重，重复投递才是两条。
    let mut repeated = HashSet::new();
    for event in &events {
        let messages: Vec<&model::Message> = match &event.kind {
            session::EventKind::UserMessage { message, .. } => vec![message],
            session::EventKind::InboxSpliced { inserted, .. } => inserted.iter().collect(),
            _ => continue,
        };
        for message in messages {
            if message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect::<String>()
                == "只该出现一次的话"
            {
                repeated.insert(message.id.clone());
            }
        }
    }
    assert_eq!(repeated.len(), 1, "同一条输入只进会话一次");
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            session::EventKind::UserMessage { message, .. }
                if matches!(&message.source, model::MessageSource::User { rpc_id: Some(id) } if id == &request_id)
        )),
        "输入用 task-say:<inputId> 做判重键"
    );
}

// ===== T029.2a：重启汇报与接续卡 =====

/// spec v1 的恢复说明原文：确认接续后第一条输入就是它。
const RECOVERY_NOTE: &str = "AIOS 在这个任务进行中重启了。先看当前的文件和状态再继续；已经做完的外部动作不要再做一遍。把原来的任务做完，然后用 task_report 上报。";

/// 起一个假模型服务和两个临时目录：重启测试的起点。
async fn restart_setup() -> (MockServer, Router, tempfile::TempDir, tempfile::TempDir) {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    (
        server,
        router,
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    )
}

/// 重开一次根宿主：模拟进程退出后根会话写锁还要一点时间才放开。
///
/// 写锁没放开时 `RootHost::open` 报 Busy，那次启动按设计不扫描；重开直到接管成功，
/// 才算一次真正的启动。
async fn reopen_actions_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
    for _ in 0..250 {
        let host = open_actions_host(server, data, cwd).await;
        if matches!(host.info().root, service::RootState::Ok) {
            return host;
        }
        drop(host);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("根会话写锁一直没放开");
}

/// 放开效果泵，等每个任务都开好会话、还没交结果，然后「杀掉服务」（drop 宿主）。
///
/// ThinAgent 的回复压后 10 秒：进程停掉前不会合成 waiting，Attempt 以「在跑没报」留在库里。
async fn crash_while_running(router: &Router, data: &Path, host: RootHost, tasks: &[Task]) {
    for task in tasks {
        router.push_thin_for_title(&task.title, crate::common::text_sse("还在做"));
    }
    router.set_thin_delay(Duration::from_secs(10));
    host.task_runtime().expect("runtime").set_pump_paused(false);
    let store = tasks_store(data);
    wait_until("任务都在跑", || {
        tasks.iter().all(|task| {
            store
                .task(&task.id)
                .ok()
                .flatten()
                .and_then(|task| task.latest_attempt().cloned())
                .is_some_and(|attempt| attempt.conversation.is_some() && attempt.ended_at.is_none())
        })
    })
    .await;
    // 模拟进程退出：会话写锁要真的放开，重启后的接续才开得出原会话。
    host.task_runtime().expect("runtime").crash();
    drop(host);
}

/// 重新读当前的任务聚合（启动扫描收尾之后）。
fn reload_tasks(store: &Store, tasks: &[Task]) -> Vec<Task> {
    tasks
        .iter()
        .map(|task| store.task(&task.id).expect("读任务").expect("任务存在"))
        .collect()
}

/// Kind 收到的重启汇报：rpc id 是 restart:<bootId>，正文是 tasks_interrupted JSON。
fn restart_messages(events: &[session::Event]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return None;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return None;
            };
            let rpc_id = rpc_id.as_deref()?;
            if !rpc_id.starts_with("restart:") {
                return None;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            Some((rpc_id.to_owned(), serde_json::from_str(&text).ok()?))
        })
        .collect()
}

/// 接续的第一条输入：rpc id 是 resume:<taskId>:<attemptId>。
///
/// 消息在日志里可能出现两次（inbox/spliced 插入 + 领取后的 user/message），按 rpc id 去重。
fn recovery_notes(events: &[session::Event]) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for event in events {
        let messages: Vec<&model::Message> = match &event.kind {
            session::EventKind::UserMessage { message, .. } => vec![message],
            session::EventKind::InboxSpliced { inserted, .. } => inserted.iter().collect(),
            _ => continue,
        };
        for message in messages {
            let model::MessageSource::User { rpc_id } = &message.source else {
                continue;
            };
            let Some(rpc_id) = rpc_id.as_deref() else {
                continue;
            };
            if !rpc_id.starts_with("resume:") {
                continue;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            if !found.iter().any(|(id, _)| id == rpc_id) {
                found.push((rpc_id.to_owned(), text));
            }
        }
    }
    found
}

/// 一张卡的全部选项（id/label/facts）。
fn card_options(events: &[session::Event], call_id: &str) -> Vec<Value> {
    events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult {
                message,
                meta: Some(meta),
                ..
            } if message.tool_call_id.as_deref() == Some(call_id) => Some(
                meta["card"]["options"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// 点确认卡上的一个选项；返回写下的决定。
async fn confirm_card_option(host: &RootHost, call_id: &str, option: &str) -> Value {
    let response = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: call_id.to_owned(),
            verb: CardVerb::Confirm,
            option: Some(option.to_owned()),

            note: None,
            report_id: None,
        }))
        .await
        .expect("确认接续卡");
    serde_json::to_value(response).expect("确认卡 json")["decision"].clone()
}

/// 点卡片上的「暂不」。
async fn dismiss_card(host: &RootHost, call_id: &str) -> Value {
    let response = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: call_id.to_owned(),
            verb: CardVerb::Dismiss,
            option: None,

            note: None,
            report_id: None,
        }))
        .await
        .expect("暂不接续");
    serde_json::to_value(response).expect("暂不 json")["decision"].clone()
}

/// Kind 重启汇报这一轮的回复：加载 aios-tasks、出一张每任务一项的接续卡、说一句。
fn push_restart_turn(router: &Router, call_id: &str, tasks: &[&Task], reply: &str) {
    let options: Vec<Value> = tasks
        .iter()
        .map(|task| {
            serde_json::json!({
                "id": task.id.as_str(),
                "action": "resume_task",
                "label": task.title,
                "details": { "task_id": task.id.as_str() },
            })
        })
        .collect();
    let args = serde_json::json!({
        "title": "接着做被打断的任务",
        "brief": "每个任务一项：勾一个就接着做，不勾就继续停着。",
        "options": options,
    })
    .to_string();
    router.push_kind(skill_sse("call-load-tasks", "aios-tasks"));
    router.push_kind(crate::common::tool_use_sse(
        1,
        call_id,
        "propose_action",
        &args,
    ));
    router.push_kind(crate::common::text_sse(reply));
}

/// 重启后 Kind 收到恰好一条汇报、出一张每任务一项的接续卡；确认一项只接这一项。
#[tokio::test]
async fn a_restart_reports_interrupted_tasks_and_resumes_the_confirmed_one() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        2,
        "乙",
    )
    .await;
    crash_while_running(&router, data.path(), host, &[first.clone(), second.clone()]).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[first, second]);
    let conversations: Vec<_> = crashed
        .iter()
        .map(|task| task.latest_attempt().unwrap().conversation.clone())
        .collect();
    let task_refs: Vec<&Task> = crashed.iter().collect();

    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲乙都被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;

    // 启动扫描在效果循环之前收尾：两个 Attempt 都记成 interrupted。
    for task in &crashed {
        let stored = store.task(&task.id).unwrap().unwrap();
        assert_eq!(
            stored.latest_attempt().unwrap().end,
            Some(AttemptEnd::Interrupted),
            "{}",
            task.title
        );
        assert_eq!(stored.stage(false), Stage::NeedsConditions);
    }

    wait_until("Kind 收到重启汇报", || {
        !restart_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;
    wait_until("接续卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    let events = crate::common::read_root_events(data.path());
    let reports = restart_messages(&events);
    assert_eq!(reports.len(), 1, "同一次启动只送一条 restart：{reports:?}");
    assert_eq!(reports[0].1["kind"], "tasks_interrupted");
    let listed = reports[0].1["tasks"].as_array().expect("tasks 数组");
    assert_eq!(listed.len(), 2, "{}", reports[0].1);
    for task in &crashed {
        let entry = listed
            .iter()
            .find(|entry| entry["id"] == task.id.as_str())
            .expect("任务在汇报里");
        assert_eq!(entry["title"], task.title);
        assert_eq!(entry["attempts"], 1);
    }
    let options = card_options(&events, "call-restart-card");
    assert_eq!(options.len(), 2, "{options:?}");
    let fixed = serde_json::json!([{ "name": "状态", "value": "被打断了 → 新的 Attempt，接着做" }]);
    for task in &crashed {
        let option = options
            .iter()
            .find(|option| option["id"] == task.id.as_str())
            .expect("每个任务一项");
        assert_eq!(option["label"], task.title);
        assert_eq!(option["facts"], fixed);
    }

    // 确认甲：新 Attempt、trigger resume、会话不变；没勾的仍是「被打断了」。
    let first_id = crashed[0].id.as_str().to_owned();
    router.push_thin_for(&first_id, crate::common::text_sse("接着做"));
    router.push_kind(crate::common::text_sse("收到"));
    let decision = confirm_card_option(&host, "call-restart-card", &first_id).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    wait_until("甲开了第二次 Attempt", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&crashed[0].id).unwrap().unwrap();
    let second_attempt = resumed.latest_attempt().unwrap().clone();
    assert_eq!(second_attempt.trigger, tasks::AttemptTrigger::Resume);
    assert_eq!(
        second_attempt.conversation, conversations[0],
        "会话 id 不变"
    );
    assert!(second_attempt.ended_at.is_none());
    let untouched = store.task(&crashed[1].id).unwrap().unwrap();
    assert_eq!(
        untouched.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted)
    );
    assert_eq!(attempt_count(&untouched), 1);

    // 第一条输入是恢复说明，原文照 spec；请求真的把它送到 ThinAgent。
    wait_until("恢复说明进了任务会话", || {
        recovery_notes(&thin_events(data.path(), &second_attempt)).len() == 1
    })
    .await;
    let notes = recovery_notes(&thin_events(data.path(), &second_attempt));
    assert_eq!(
        notes[0].0,
        format!("resume:{first_id}:{}", second_attempt.id),
        "恢复说明的判重键"
    );
    assert_eq!(notes[0].1, RECOVERY_NOTE);
    wait_until("接续请求带上恢复说明", || {
        router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains(RECOVERY_NOTE))
    })
    .await;

    // 同一张卡再点一次不重复执行。
    let again = confirm_card_option(&host, "call-restart-card", &first_id).await;
    assert_eq!(again, decision);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        attempt_count(&store.task(&crashed[0].id).unwrap().unwrap()),
        2
    );
    drop(host);
}

/// 点「暂不」：任务继续停着，之后不再有新的 restart 汇报。
#[tokio::test]
async fn dismissing_the_restart_card_leaves_the_task_interrupted_without_another_restart() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let task_refs: Vec<&Task> = crashed.iter().collect();
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    assert_eq!(
        store
            .task(&crashed[0].id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .end,
        Some(AttemptEnd::Interrupted),
        "启动扫描先把上次的 Attempt 收尾"
    );
    wait_until("接续卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    let decision = dismiss_card(&host, "call-restart-card").await;
    assert_eq!(decision["phase"], "dismissed", "{decision}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        restart_messages(&crate::common::read_root_events(data.path())).len(),
        1,
        "暂不之后不再有新的 restart"
    );
    let stored = store.task(&crashed[0].id).unwrap().unwrap();
    assert_eq!(
        stored.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted)
    );
    assert_eq!(attempt_count(&stored), 1, "暂不不开新 Attempt");
    drop(host);
}

/// 先在任务对话里说话，再点卡上那一项：reconcile 回「已经接着做了」，只跑一个 Attempt。
#[tokio::test]
async fn talking_in_the_task_dialog_before_confirming_says_already_resumed() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let task_refs: Vec<&Task> = crashed.iter().collect();
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    assert_eq!(
        store
            .task(&crashed[0].id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .end,
        Some(AttemptEnd::Interrupted),
        "启动扫描先把上次的 Attempt 收尾"
    );
    wait_until("接续卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    // 用户在任务对话里先说一句：打断后的说话按接续开新 Attempt，第一条带着恢复说明。
    router.push_thin_for(crashed[0].id.as_str(), crate::common::text_sse("收到"));
    router.push_kind(crate::common::text_sse("收到"));
    say_in_task_dialog(&host, &crashed[0], "把没跑完的补上").await;
    wait_until("任务对话先接上了", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&crashed[0].id).unwrap().unwrap();
    let attempt = resumed.latest_attempt().cloned().expect("新 Attempt");
    assert_eq!(attempt.trigger, tasks::AttemptTrigger::Resume);
    wait_until("恢复说明进了任务会话", || {
        recovery_notes(&thin_events(data.path(), &attempt)).len() == 1
    })
    .await;
    let notes = recovery_notes(&thin_events(data.path(), &attempt));
    assert_eq!(
        notes[0].0,
        format!("resume:{}:{}", crashed[0].id, attempt.id),
        "恢复说明的判重键"
    );
    let note_at = notes[0]
        .1
        .find(RECOVERY_NOTE)
        .expect("恢复说明在同一条消息里");
    let words_at = notes[0]
        .1
        .find("把没跑完的补上")
        .expect("用户的话在同一条消息里");
    assert!(note_at < words_at, "恢复说明在用户的话前面：{}", notes[0].1);

    let decision = confirm_card_option(&host, "call-restart-card", crashed[0].id.as_str()).await;
    assert_eq!(decision["phase"], "refused", "{decision}");
    let summary = decision["data"]["summary"].as_str().expect("拒绝原因");
    assert!(summary.contains("已经接着做了"), "{summary}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        attempt_count(&store.task(&crashed[0].id).unwrap().unwrap()),
        2,
        "只跑一个 Attempt"
    );
    drop(host);
}

/// 被打断后先在任务对话里说一句：新 Attempt 的 trigger 是 resume，第一条消息
/// 恢复说明在前、用户的话在后，判重键是 resume:<taskId>:<attemptId>，页头不再带停止标记。
#[tokio::test]
async fn saying_after_an_interruption_resumes_with_the_recovery_note_before_the_user_words() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let first = crashed[0]
        .latest_attempt()
        .cloned()
        .expect("第一次 Attempt");
    let conversation = first.conversation.clone().expect("会话 id");
    let task_refs: Vec<&Task> = crashed.iter().collect();
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    let interrupted = store.task(&crashed[0].id).unwrap().unwrap();
    assert_eq!(
        interrupted.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted),
        "启动扫描先把上次的 Attempt 收尾"
    );
    wait_until("Kind 收到重启汇报", || {
        !restart_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;

    router.push_thin_for(crashed[0].id.as_str(), crate::common::text_sse("接着做"));
    let said = say_in_task_dialog(&host, &crashed[0], "把剩下两个线索查完").await;
    assert_eq!(said.new_attempt, Some(first.id.0 + 1));
    wait_until("说话开了新 Attempt", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&crashed[0].id).unwrap().unwrap();
    let second = resumed.latest_attempt().cloned().expect("第二次 Attempt");
    assert_eq!(second.trigger, tasks::AttemptTrigger::Resume);
    assert_eq!(
        second.conversation.as_ref(),
        Some(&conversation),
        "会话 id 不变"
    );
    assert!(second.ended_at.is_none());

    wait_until("恢复说明进了任务会话", || {
        recovery_notes(&thin_events(data.path(), &second)).len() == 1
    })
    .await;
    let notes = recovery_notes(&thin_events(data.path(), &second));
    assert_eq!(
        notes[0].0,
        format!("resume:{}:{}", crashed[0].id, second.id),
        "恢复说明的判重键"
    );
    let note_at = notes[0]
        .1
        .find(RECOVERY_NOTE)
        .expect("恢复说明在同一条消息里");
    let words_at = notes[0]
        .1
        .find("把剩下两个线索查完")
        .expect("用户的话在同一条消息里");
    assert!(note_at < words_at, "恢复说明在用户的话前面：{}", notes[0].1);

    wait_until("下一次请求带着恢复说明和用户的话", || {
        router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains(RECOVERY_NOTE) && body.contains("把剩下两个线索查完"))
    })
    .await;
    let body = router.bodies(true).last().cloned().expect("接续请求");
    let note_at = body.find(RECOVERY_NOTE).expect("恢复说明在请求里");
    let words_at = body.find("把剩下两个线索查完").expect("用户的话在请求里");
    assert!(note_at < words_at, "请求体里恢复说明在用户的话前面");

    // 新 Attempt 已经开跑：页头不再带上一 Attempt 的停止标记，阶段也不是被打断了。
    let read = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: crashed[0].id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(read) = read else {
        panic!("TaskRead 的响应形状不对");
    };
    assert!(!read.task.stopped, "不残留停止标记：{:?}", read.task);
    assert_eq!(read.task.attempts, 2);
    assert_ne!(
        resumed.stage(false),
        Stage::NeedsConditions,
        "新 Attempt 开跑后不再是被打断了"
    );
    drop(host);
}

/// 打断后的说话接续占 wake 席位，不额外多占：席位满时那句留在库里，腾出来再接。
#[tokio::test]
async fn saying_after_an_interruption_uses_a_wake_seat_and_waits_when_all_are_taken() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&first)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[first]);
    // 重启前把席位改成 1：唤醒接续占满以后，说话只能等。
    let mut saved = service::settings::load(data.path());
    saved.exec.seats = 1;
    service::settings::save(data.path(), &saved).expect("保存席位");
    router.clear_thin_delay();
    let task_refs: Vec<&Task> = crashed.iter().collect();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    assert_eq!(
        store
            .task(&crashed[0].id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .end,
        Some(AttemptEnd::Interrupted)
    );
    wait_until("Kind 收到重启汇报", || {
        !restart_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;
    wait_until("接续卡出来，这一轮收尾", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    // 新派一个任务占掉唯一的席位，这一轮挂住不结束。
    router.set_thin_delay(Duration::from_secs(30));
    router.push_thin_for_title("乙", crate::common::text_sse("还在做"));
    let filler = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        3,
        "乙",
    )
    .await;
    wait_until("乙占住席位", || attempt_in_flight(&store, &filler.id)).await;

    // 甲说话：wake 不超席位，Attempt 不增加，delivery 留在库里等。
    let said = say_in_task_dialog(&host, &crashed[0], "甲先把没跑完的补上").await;
    assert_eq!(said.new_attempt, Some(2), "回执只是预览，不代表真开");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let waiting = store.task(&crashed[0].id).unwrap().unwrap();
    assert_eq!(attempt_count(&waiting), 1, "席位满时打断接续不开 Attempt");
    assert!(
        store
            .effect_pending(EffectKind::Delivery, crashed[0].id.as_str())
            .unwrap(),
        "甲的 delivery 留在库里等席位"
    );
    assert_eq!(
        store.effect_health(EffectKind::Delivery).unwrap().attempts,
        0,
        "Later 不算失败"
    );

    // 乙停下放出席位：甲的那句被重新领取，按 resume 接上。
    router.push_kind(crate::common::text_sse("收到"));
    router.push_thin_for(crashed[0].id.as_str(), crate::common::text_sse("甲接着做"));
    let response = host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: filler.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停止");
    let Response::TaskStop(stop) = response else {
        panic!("TaskStop 的响应形状不对");
    };
    assert!(stop.stopped, "{stop:?}");
    wait_until("甲接上", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&crashed[0].id).unwrap().unwrap();
    let attempt = resumed.latest_attempt().cloned().expect("甲的新 Attempt");
    assert_eq!(attempt.trigger, tasks::AttemptTrigger::Resume);
    assert!(attempt.ended_at.is_none());
    drop(host);
}

/// 全局暂停开着时重启：汇报不送；开闸后送一条，而且只有一条。
#[tokio::test]
async fn a_paused_restart_waits_for_the_gate_then_reports_once() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let task_refs: Vec<&Task> = crashed.iter().collect();
    let requests_before = router.bodies(false).len();
    let mut saved = service::settings::load(data.path());
    saved.exec.paused = true;
    service::settings::save(data.path(), &saved).expect("保存暂停");
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;

    // 扫描照做；效果不送。
    assert_eq!(
        store
            .task(&crashed[0].id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .end,
        Some(AttemptEnd::Interrupted)
    );
    wait_until("restart 留在库里", || {
        store
            .effect_health(EffectKind::Restart)
            .expect("效果健康")
            .waiting
            == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        restart_messages(&crate::common::read_root_events(data.path())).is_empty(),
        "暂停时不送 restart"
    );
    assert_eq!(
        router.bodies(false).len(),
        requests_before,
        "暂停时不叫模型"
    );

    host.dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
        currency: None,
        paused: Some(false),
        seats: None,
        bash: None,
    }))
    .await
    .expect("开闸");
    wait_until("开闸后汇报", || {
        restart_messages(&crate::common::read_root_events(data.path())).len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        restart_messages(&crate::common::read_root_events(data.path())).len(),
        1
    );
    drop(host);
}

/// 模型不可用时汇报按 Later 留在库里：不算失败，不发请求，等模型恢复。
#[tokio::test]
async fn a_restart_report_stays_later_while_the_model_is_unavailable() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let requests_before = router.bodies(false).len();
    let factory: service::ModelFactory = Arc::new(|_selection| Err("还没有接模型".to_owned()));
    let host = open_host_with(&server, data.path(), cwd.path(), |options| {
        options.actions = Arc::new(service::ActionRegistry::settings(
            data.path().to_path_buf(),
            options.identity.clone(),
        ));
        options.model_factory = Some(factory.clone());
    })
    .await;
    assert!(
        !matches!(host.info().root, service::RootState::Ok),
        "模型不可用：根会话起不来"
    );
    assert_eq!(
        store
            .task(&crashed[0].id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .end,
        Some(AttemptEnd::Interrupted),
        "扫描照做"
    );
    wait_until("restart 留在库里", || {
        store
            .effect_health(EffectKind::Restart)
            .expect("效果健康")
            .waiting
            == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let health = store.effect_health(EffectKind::Restart).expect("效果健康");
    assert_eq!(health.waiting, 1, "Later 不丢效果");
    assert_eq!(health.attempts, 0, "Later 不算失败");
    assert_eq!(
        router.bodies(false).len(),
        requests_before,
        "模型不可用不发请求"
    );
    assert!(restart_messages(&crate::common::read_root_events(data.path())).is_empty());
    drop(host);
}

/// TaskRead 的头部事实（只读；测试里读页头用）。
async fn task_read_header(host: &RootHost, task_id: &tasks::TaskId) -> service::TaskReadHeader {
    let request = TaskReadRequest {
        task_id: task_id.as_str().to_owned(),
        before_seq: None,
    };
    match host
        .dispatch(Call::TaskRead(request))
        .await
        .expect("task-read")
    {
        Response::TaskRead(read) => read.task,
        _ => panic!("task-read 响应"),
    }
}

/// 页头事实：启动扫描打断后一直「被打断了」；给 Kind 的 restart 还在队里时
/// restartPending 为真，送到后转假（横幅消失，任务不自动跑）。
#[tokio::test]
async fn task_read_reports_interrupted_and_restart_pending_until_kind_is_told() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let task_refs: Vec<&Task> = crashed.iter().collect();
    let mut saved = service::settings::load(data.path());
    saved.exec.paused = true;
    service::settings::save(data.path(), &saved).expect("保存暂停");
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;

    // 扫描照做，restart 不送：页头同时是「被打断了」和「待 Kind 核实」。
    wait_until("restart 留在库里", || {
        store
            .effect_health(EffectKind::Restart)
            .expect("效果健康")
            .waiting
            == 1
    })
    .await;
    let before = task_read_header(&host, &crashed[0].id).await;
    assert!(before.interrupted, "扫描后页头要固定「被打断了」");
    assert!(
        before.restart_pending,
        "Kind 还没收到 restart：页头要「待 Kind 核实，尚未接续」"
    );
    assert_eq!(before.attempts, 1, "没自动重跑");

    host.dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
        currency: None,
        paused: Some(false),
        seats: None,
        bash: None,
    }))
    .await
    .expect("开闸");
    wait_until("Kind 收到重启汇报", || {
        restart_messages(&crate::common::read_root_events(data.path())).len() == 1
    })
    .await;

    let after = task_read_header(&host, &crashed[0].id).await;
    assert!(after.interrupted, "restart 送到后任务仍是「被打断了」");
    assert!(!after.restart_pending, "restart 送到后横幅消失");
    assert_eq!(after.attempts, 1, "送汇报不自动开新的 Attempt");
    drop(host);
}

/// 汇报里的 last_report 是最近一份报告，超过 300 字截断；attempts 是总次数。
#[tokio::test]
async fn a_restart_report_carries_the_last_report_capped_at_300_characters() {
    let (server, router, data, cwd) = restart_setup().await;
    let store = tasks_store(data.path());
    let long = "报".repeat(400);
    let draft = tasks::NewTask {
        title: "有旧报告的".to_owned(),
        requirement: "找答案".to_owned(),
        dod: "有答案".to_owned(),
        cwd: data.path().to_path_buf(),
        conversation: tasks::ConversationId::new("kind-root"),
        model: tasks::ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: service::THIN_TOOLS.map(str::to_owned).to_vec(),
    };
    let task = store.create_task(draft, 1_700_000_000_000).unwrap();
    let attempt = store.begin_attempt(&task.id, 1_700_000_000_001).unwrap();
    store
        .attach_session(
            &task.id,
            attempt,
            tasks::ConversationId::new("task-session"),
        )
        .unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            tasks::NewReport {
                state: ReportState::Waiting,
                origin: tasks::ReportOrigin::Executor,
                summary: long.clone(),
                question: None,
            },
            1_700_000_000_002,
        )
        .unwrap();
    // 报告之后又记了输入：启动扫描按「报告后又输入」记 interrupted。
    store
        .say_input(
            &task.id,
            tasks::NewInput {
                text: "继续".to_owned(),
                ..tasks::NewInput::default()
            },
            1_700_000_000_003,
        )
        .unwrap();

    router.push_kind(crate::common::text_sse("收到"));
    let _host = open_host(&server, data.path(), cwd.path()).await;
    wait_until("Kind 收到重启汇报", || {
        !restart_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;
    let reports = restart_messages(&crate::common::read_root_events(data.path()));
    assert_eq!(reports.len(), 1);
    let entry = &reports[0].1["tasks"][0];
    assert_eq!(entry["id"], task.id.as_str());
    assert_eq!(entry["attempts"], 1);
    let summary = entry["last_report"].as_str().expect("最近报告");
    assert_eq!(summary.chars().count(), 300, "最多 300 字");
    assert!(summary.chars().all(|ch| ch == '报'));
}

/// 第二次启动只报这次扫描打断的任务：上次已暂不的甲不再上卡，仍停在「被打断了」。
#[tokio::test]
async fn a_second_restart_reports_only_the_tasks_its_scan_interrupted() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&first)).await;

    let store = tasks_store(data.path());
    let interrupted = reload_tasks(&store, &[first]);
    let first_id = interrupted[0].id.clone();
    let task_refs: Vec<&Task> = interrupted.iter().collect();
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-a", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    wait_until("第一次重启的卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-a",
        )
        .is_empty()
    })
    .await;
    // 用户点「暂不」：甲继续停着，Kind 不再追问。
    dismiss_card(&host, "call-restart-a").await;

    // 第二次启动的服务里乙跑着，进程停掉时它还没交结果。
    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        3,
        "乙",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&second)).await;

    let interrupted = reload_tasks(&store, &[second]);
    let second_id = interrupted[0].id.clone();
    let task_refs: Vec<&Task> = interrupted.iter().collect();
    push_restart_turn(&router, "call-restart-b", &task_refs, "乙被打断了。");
    let host = reopen_actions_host(&server, data.path(), cwd.path()).await;

    wait_until("第二次重启汇报", || {
        restart_messages(&crate::common::read_root_events(data.path())).len() == 2
    })
    .await;
    wait_until("第二次重启的卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-b",
        )
        .is_empty()
    })
    .await;
    let events = crate::common::read_root_events(data.path());
    let reports = restart_messages(&events);
    let latest = &reports[1].1;
    let listed: Vec<&str> = latest["tasks"]
        .as_array()
        .expect("tasks 数组")
        .iter()
        .map(|entry| entry["id"].as_str().expect("任务 id"))
        .collect();
    assert_eq!(
        listed,
        vec![second_id.as_str()],
        "只报这次扫描打断的乙：{latest}"
    );

    // 甲没有被再问一次，也没有被悄悄再接续：仍是一次 interrupted 的 Attempt。
    let stored_first = store.task(&first_id).unwrap().unwrap();
    assert_eq!(
        stored_first.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted)
    );
    assert_eq!(attempt_count(&stored_first), 1);
    assert_eq!(stored_first.stage(false), Stage::NeedsConditions);

    let options = card_options(&events, "call-restart-b");
    assert_eq!(options.len(), 1, "{options:?}");
    assert_eq!(options[0]["id"], second_id.as_str());
    drop(host);
}

/// 写任务：写入范围落库、工具名单正好 11 件（+write/edit）；task_accept 的 schema
/// 有 write_scope/worktree，没有 bash 字段（bash 开关归另一张票）。
#[tokio::test]
async fn a_write_task_freezes_the_scope_and_eleven_tools() {
    let fixture = Fixture::new().await;
    let target = fixture.cwd.path().join("target.py");
    std::fs::write(&target, "old\n").unwrap();
    let task = fixture.accept_write(true, "target.py").await;

    let scope = std::fs::canonicalize(&target).unwrap();
    assert_eq!(task.write_scope.as_deref(), Some(scope.as_path()));
    assert_eq!(task.requirement_version(), 1);
    assert_eq!(
        task.versions[0].write_scope.as_deref(),
        Some(scope.as_path()),
        "第一条要求版本带着写入范围"
    );

    let mut tools = task.tools.clone();
    tools.sort();
    let mut expected = vec![
        "edit",
        "glob",
        "grep",
        "read",
        "skill",
        "task_handback",
        "task_ledger_history",
        "task_ledger_note",
        "task_report",
        "todo_write",
        "web_fetch",
        "web_search",
        "write",
    ];
    expected.sort();
    assert_eq!(tools, expected, "写任务冻结 13 件工具");

    let kind_body = fixture.router.bodies(false).remove(0);
    let request: Value = serde_json::from_str(&kind_body).unwrap();
    let accept = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "task_accept")
        .expect("task_accept 的工具定义");
    let properties = &accept["input_schema"]["properties"];
    assert!(properties.get("write_scope").is_some(), "{properties}");
    assert!(properties.get("worktree").is_some(), "{properties}");
    assert!(
        properties.get("bash").is_none(),
        "bash 不能成为 task_accept 字段：{properties}"
    );
}

/// Kind 的工具面：能读、能派/问/停/改任务，没有写、改、跑命令和三件后台任务。
#[tokio::test]
async fn kind_has_no_write_shell_or_background_tools() {
    let fixture = Fixture::new().await;
    fixture.router.push_kind(crate::common::text_sse("好"));
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-1",
            "看一下你能做什么",
        )))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;

    let body = fixture.router.bodies(false).remove(0);
    let names = tool_names(&body);
    for banned in [
        "write",
        "edit",
        "apply_patch",
        "bash",
        "job_output",
        "job_list",
        "job_kill",
    ] {
        assert!(
            !names.iter().any(|name| name == banned),
            "Kind 不该有 {banned}：{names:?}"
        );
    }
    for required in [
        "read",
        "glob",
        "grep",
        "skill",
        "task_accept",
        "task_status",
        "task_stop",
        "task_update",
        "ask_user_question",
        "canvas",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "Kind 要有 {required}：{names:?}"
        );
    }
}

/// 写入范围外的写被拒：错误码和原文固定、文件内容与 mtime 都没变，之后能交回 Kind。
#[tokio::test]
async fn an_out_of_scope_write_is_refused_unchanged_and_hands_back() {
    let fixture = Fixture::new().await;
    let target = fixture.cwd.path().join("target.py");
    let outside = fixture.cwd.path().join("outside.py");
    std::fs::write(&target, "inside\n").unwrap();
    std::fs::write(&outside, "outside\n").unwrap();
    let outside_mtime = std::fs::metadata(&outside).unwrap().modified().unwrap();

    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-write",
        "write",
        &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::tool_use_sse(
        2,
        "call-handback",
        "task_handback",
        &serde_json::json!({"reason": "用户要改写入范围外的文件"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::text_sse("交回"));
    let task = fixture.accept_write(false, "target.py").await;
    wait_until("交回 relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;

    let store = tasks_store(fixture.data());
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    let (text, error) = tool_result(&events, "call-write");
    let scope = std::fs::canonicalize(&target).unwrap();
    assert_eq!(error, Some(true), "{text}");
    assert_eq!(
        text,
        format!(
            "Error: 这个路径不在写入范围里（写入范围：{}）。要改范围外的，调 task_handback。",
            scope.display()
        ),
        "拒绝原文固定"
    );
    let code = events.iter().find_map(|event| match &event.kind {
        session::EventKind::ToolResult { message, error, .. }
            if message.tool_call_id.as_deref() == Some("call-write") =>
        {
            error.as_ref().map(|error| error.code.clone())
        }
        _ => None,
    });
    assert_eq!(code.as_deref(), Some("WRITE_OUT_OF_SCOPE"), "拒绝码固定");
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "outside\n",
        "越界写不能改文件内容"
    );
    assert_eq!(
        std::fs::metadata(&outside).unwrap().modified().unwrap(),
        outside_mtime,
        "越界写连 mtime 都不能动"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "inside\n");

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "blocked");
    assert_eq!(relays[0].1["origin"], "agent");
    assert_eq!(relays[0].1["summary"], "用户要改写入范围外的文件");
    assert_eq!(relays[0].1["question"], "这超出了任务范围，要不要改任务？");
    assert_eq!(
        attempt.end,
        Some(AttemptEnd::Reported),
        "交回结束这个 Attempt"
    );
}

/// 越界交回后，用户在 Kind 对话里同意改范围：task_update 落第 2 版要求，
/// 原会话开新 Attempt（trigger requirement_changed），第一条输入写明新范围，新范围能写。
#[tokio::test]
async fn task_update_records_a_new_version_and_resumes_with_the_new_scope() {
    let fixture = Fixture::new().await;
    let target = fixture.cwd.path().join("target.py");
    let outside = fixture.cwd.path().join("outside.py");
    std::fs::write(&target, "inside\n").unwrap();

    // 第一个 Attempt：越界写被拒，交回。
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-write-1",
        "write",
        &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::tool_use_sse(
        2,
        "call-handback",
        "task_handback",
        &serde_json::json!({"reason": "outside.py 不在写入范围里"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::text_sse("交回"));
    // 第二个 Attempt：范围扩大后写成功、报 ready。
    fixture.router.push_thin(crate::common::tool_use_sse(
        3,
        "call-write-2",
        "write",
        &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
    ));
    fixture
        .router
        .push_thin(report_sse(4, "call-report", "ready", "改完了", None));
    fixture.router.push_thin(crate::common::text_sse("done"));

    let task = fixture.accept_write(true, "target.py").await;
    // 交回 relay 会叫醒 Kind 一轮；先给它一条备用回复。
    fixture.router.push_kind(crate::common::text_sse("收到卡"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("交回 relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;

    let scope = std::fs::canonicalize(fixture.cwd.path())
        .unwrap()
        .join("outside.py");
    let update_args = serde_json::json!({
        "task_id": task.id.as_str(),
        "write_scope": "outside.py"
    })
    .to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        5,
        "call-update",
        "task_update",
        &update_args,
    ));
    fixture
        .router
        .push_kind(crate::common::text_sse("好，范围扩大了"));
    // 新 Attempt 报 ready 之后还会叫醒 Kind 一轮。
    fixture
        .router
        .push_kind(crate::common::text_sse("收到结果"));
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-2",
            "我同意把写入范围扩大到 outside.py",
        )))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 3).await;

    let store = tasks_store(fixture.data());
    wait_until("第二个 Attempt 报 ready", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            task.requirement_version() == 2
                && attempt_count(&task) == 2
                && task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;

    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.versions.len(), 2, "{:?}", updated.versions);
    assert_eq!(
        updated.versions[1].write_scope.as_deref(),
        Some(scope.as_path())
    );
    assert_eq!(
        updated.versions[1].source,
        tasks::VersionSource::UserAgreed,
        "第 2 版来自用户在 Kind 对话里的同意"
    );
    let first = updated.runs[0].attempts[0].clone();
    let second = updated.latest_attempt().expect("新 Attempt").clone();
    assert_eq!(second.id.0, first.id.0 + 1);
    assert_eq!(second.trigger, tasks::AttemptTrigger::RequirementChanged);
    assert_eq!(
        second.conversation, first.conversation,
        "还在原任务会话里接着做"
    );

    let events = thin_events(fixture.data(), &second);
    let request_id = format!("task-update:{}:{}", updated.id, second.id);
    let note = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => match &message.source {
                model::MessageSource::User { rpc_id: Some(id) } if id == &request_id => Some(
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                ),
                _ => None,
            },
            _ => None,
        })
        .expect("新 Attempt 的第一条输入");
    assert!(
        note.contains(&scope.display().to_string()),
        "第一条输入写明新范围：{note}"
    );
    assert!(note.contains("写入范围"), "{note}");

    let (write_text, write_error) = tool_result(&events, "call-write-2");
    assert_eq!(write_error, None, "{write_text}");
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "changed\n",
        "新范围里的文件写成了"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "inside\n");

    let root_events = crate::common::read_root_events(fixture.data());
    let (update_text, update_error) = tool_result(&root_events, "call-update");
    assert_eq!(update_error, None, "{update_text}");
    assert!(update_text.contains("第 2 版"), "{update_text}");
    assert!(
        update_text.contains(&scope.display().to_string()),
        "{update_text}"
    );
}

/// 写任务工具表里和 bash 有关的四件（D85：角色开关打开才给）。
const BASH_JOB_TOOLS: [&str; 4] = ["bash", "job_output", "job_list", "job_kill"];

/// 只读 9 件：T030.2b 里开关打开也不能给只读任务加。
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

/// bash 关着：写任务还是 11 件、没有 bash 和后台任务，结果卡数据里也没有 bash 说明。
#[tokio::test]
async fn a_write_task_without_bash_gets_eleven_tools_and_no_bash_note() {
    let fixture = Fixture::new().await;
    save_exa_key(fixture.data());
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-skill",
            "skill",
            &serde_json::json!({"name": "web"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(2, "call-report", "ready", "改好了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));

    let task = fixture.accept_args(true, write_task_args(), 1).await;
    assert!(!task.bash, "默认关：写任务不冻结 bash");
    assert_eq!(
        sorted(task.tools.clone()),
        sorted(WRITE_TOOLS),
        "写任务 13 件"
    );

    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let body = &relays[0].1;
    assert_eq!(body["state"], "ready", "{body}");
    assert!(
        body.get("bash_note").is_none(),
        "关着 bash 不写说明：{body}"
    );

    let names = sorted(tool_names(&loaded_request(&fixture.router)));
    assert_eq!(names, sorted(WRITE_TOOLS), "ThinAgent 的工具表正好 13 件");

    let bodies = fixture.router.bodies(true);
    assert!(
        bodies[0].contains("You cannot run shell commands"),
        "关着 bash 的角色规则说明没有跑命令的手段"
    );
    assert!(
        !bodies[0].contains("You also have bash and the background job tools"),
        "关着 bash 不给跑命令的说法"
    );
}

/// 开关打开：受理时冻结 15 件；之后关掉开关不影响已派的任务（起跑仍用 15 件）。
#[tokio::test]
async fn the_bash_switch_on_freezes_fifteen_tools_into_the_write_task() {
    let fixture = Fixture::new().await;
    save_exa_key(fixture.data());
    set_bash(&fixture.host, true).await;
    assert!(
        service::settings::load(fixture.data()).exec.bash,
        "开关写进设置文件"
    );

    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-skill",
            "skill",
            &serde_json::json!({"name": "web"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(2, "call-report", "ready", "改好了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));

    let task = fixture.accept_args(true, write_task_args(), 1).await;
    assert!(task.bash, "创建时冻结打开");
    assert_eq!(
        sorted(task.tools.clone()),
        sorted(WRITE_BASH_TOOLS),
        "开了 bash 的写任务冻结 17 件"
    );
    for name in BASH_JOB_TOOLS {
        assert!(
            task.tools.iter().any(|tool| tool == name),
            "冻结名单要有 {name}：{:?}",
            task.tools
        );
    }

    // 创建之后关掉开关：任务行不跟着变，起跑也用冻结的 15 件。
    set_bash(&fixture.host, false).await;
    assert!(!service::settings::load(fixture.data()).exec.bash);
    let frozen = tasks_store(fixture.data()).task(&task.id).unwrap().unwrap();
    assert!(frozen.bash, "已派任务的 bash 不跟着开关变");
    assert_eq!(sorted(frozen.tools.clone()), sorted(WRITE_BASH_TOOLS));

    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;

    let names = sorted(tool_names(&loaded_request(&fixture.router)));
    assert_eq!(
        names,
        sorted(WRITE_BASH_TOOLS),
        "ThinAgent 的工具表正好 15 件"
    );

    let bodies = fixture.router.bodies(true);
    assert!(
        bodies[0].contains("You also have bash and the background job tools"),
        "开着 bash 的角色规则说清楚能跑命令"
    );
    assert!(
        !bodies[0].contains("You cannot run shell commands"),
        "开着 bash 不能再说不许跑命令"
    );
}

/// 开了 bash 的写任务，结果卡数据带固定说明（原文来自 spec 故事 7）。
#[tokio::test]
async fn a_bash_task_card_carries_the_fixed_note() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(1, "call-report", "ready", "改好了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));

    let task = fixture.accept_args(false, write_task_args(), 1).await;
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let body = &relays[0].1;
    assert_eq!(body["task"]["id"], task.id.as_str(), "{body}");
    assert_eq!(
        body["bash_note"], "这个任务开了 bash，bash 的改动不在清单里",
        "说明原文进卡数据"
    );
}

/// 只读任务：开关打开也不加 bash 和后台任务，还是 9 件。
#[tokio::test]
async fn a_read_only_task_never_gets_bash_even_when_the_switch_is_on() {
    let fixture = Fixture::new().await;
    save_exa_key(fixture.data());
    set_bash(&fixture.host, true).await;
    assert!(service::settings::load(fixture.data()).exec.bash);

    fixture.router.push_thin_for_title(
        "查一下",
        crate::common::tool_use_sse(
            1,
            "call-skill",
            "skill",
            &serde_json::json!({"name": "web"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "查一下",
        report_sse(2, "call-report", "ready", "查好了", None),
    );
    fixture
        .router
        .push_thin_for_title("查一下", crate::common::text_sse("done"));

    let task = fixture
        .accept_args(
            true,
            serde_json::json!({
                "title": "查一下",
                "requirement": "找答案",
                "dod": "有答案"
            }),
            1,
        )
        .await;
    assert!(!task.bash, "只读任务不冻结 bash");
    assert_eq!(
        sorted(task.tools.clone()),
        sorted(READ_ONLY_TOOLS),
        "只读任务还是 11 件"
    );

    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;
    let names = sorted(tool_names(&loaded_request(&fixture.router)));
    assert_eq!(
        names,
        sorted(READ_ONLY_TOOLS),
        "只读任务给 ThinAgent 的工具表正好 11 件"
    );
    for name in BASH_JOB_TOOLS {
        assert!(
            !names.iter().any(|tool| tool == name),
            "只读任务不给 {name}：{names:?}"
        );
    }
}

/// 立即停：连这个任务的后台命令一起停（进程收到 SIGTERM 并退出）。
///
/// 后台命令把收到 TERM 写成 cwd 里的文件；没被杀掉时它会自己睡完 60 秒，
/// 所以红的时候这里等不到文件，也不会留下一个永远不退的进程。
#[tokio::test]
async fn immediately_stopping_a_task_kills_its_background_jobs() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    // 第一轮回复慢一点：任务要在后台命令起来、第二轮还在途的时候被停。
    fixture.router.set_thin_delay(Duration::from_millis(2_000));
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-bash",
            "bash",
            &serde_json::json!({
                "description": "等停止信号",
                "command": "trap 'echo stopped > killed.txt' TERM; sleep 60",
                "run_in_background": true
            })
            .to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("还在等"));

    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let store = tasks_store(fixture.data());
    wait_until("后台任务登记", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| {
                let Some(conversation) = attempt.conversation else {
                    return false;
                };
                let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
                    return false;
                };
                session::SessionStore::open_read_only(fixture.data(), &id).is_ok_and(|session| {
                    session.events().iter().any(|event| {
                        matches!(&event.kind, session::EventKind::JobRegistered { .. })
                    })
                })
            })
    })
    .await;

    let stop = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stop.stopped, "{stop:?}");
    wait_until("后台命令收到 SIGTERM", || {
        fixture.cwd.path().join("killed.txt").exists()
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("killed.txt")).unwrap(),
        "stopped\n",
        "后台命令收到 TERM 后落了文件"
    );
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

/// 请求体系统提示里的工作目录。
fn request_working_directory(body: &str) -> String {
    let value: Value = serde_json::from_str(body).expect("请求体 JSON");
    let system = value["system"].as_str().expect("system");
    let prefix = "You are AIOS, a coding agent. Your working directory is ";
    let line = system
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .expect("system 里的工作目录");
    line.strip_suffix('.').expect("工作目录句号").to_owned()
}

/// 受理前就能按标题排队 ThinAgent 回复的写任务参数：cwd 用仓库里的 sub，
/// 写入范围是 sub/app.py，开工作树。
fn worktree_task_args() -> Value {
    serde_json::json!({
        "title": "改一下判重",
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

/// worktree:true：第一次 Attempt 起跑前在 cwd 所在的仓库开工作树，ThinAgent 的 cwd
/// 换到工作树里对应的 sub；写落到工作树，原目录不动；卡数据带工作树路径。
#[tokio::test]
async fn a_worktree_task_runs_in_the_worktree_and_writes_only_there() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-read",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-write",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "changed\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(2, "call-report", "ready", "改好了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));
    let task = fixture.accept_args(false, worktree_task_args(), 1).await;
    let store = tasks_store(fixture.data());
    wait_until("worktree ready", || reported_ready(&store, &task.id)).await;
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;

    let worktree_root = worktree_root(&fixture, &task);
    let worktree_cwd = worktree_cwd(&fixture, &task);
    assert_eq!(
        worktree_cwd.strip_prefix(&worktree_root).unwrap(),
        Path::new("sub"),
        "工作树里的子目录对应原 cwd 的相对位置"
    );

    let stored = store.task(&task.id).unwrap().unwrap();
    let attempt = stored.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    let assignment = assignment(&events);
    assert_eq!(
        assignment["cwd"],
        serde_json::json!(worktree_cwd.display().to_string()),
        "assignment 的 cwd 在工作树里"
    );
    assert_eq!(
        assignment["write_scope"],
        serde_json::json!(worktree_cwd.join("app.py").display().to_string()),
        "写入范围按同样的相对位置换算"
    );
    let bodies = fixture.router.bodies(true);
    assert!(
        bodies
            .iter()
            .any(|body| request_working_directory(body) == worktree_cwd.display().to_string()),
        "ThinAgent 请求的工作目录在工作树里：{bodies:?}"
    );

    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("sub").join("app.py")).unwrap(),
        "old\n",
        "原目录文件不改"
    );
    assert_eq!(
        std::fs::read_to_string(worktree_cwd.join("app.py")).unwrap(),
        "changed\n",
        "写落在工作树里"
    );

    let repo = fixture.cwd.path();
    let listed = listed_worktrees(repo);
    assert_eq!(listed.len(), 2, "原仓库加一个工作树：{listed:?}");
    assert!(
        listed.contains(&worktree_root),
        "工作树登记在这个仓库里：{listed:?}"
    );
    let branches = git(repo, &["branch", "--list", &format!("aios/{}", task.id)]);
    assert_eq!(
        branches.matches(&format!("aios/{}", task.id)).count(),
        1,
        "工作树分支只有一条：{branches}"
    );

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let (_, ready) = relays
        .iter()
        .find(|(_, body)| body["state"] == "ready")
        .expect("ready relay");
    assert_eq!(ready["task"]["stage"], "ready_for_review");
    assert_eq!(
        ready["worktree_path"],
        serde_json::json!(worktree_root.display().to_string()),
        "卡数据带工作树路径"
    );
    assert_eq!(ready["card_state"], "review", "{ready}");
    assert_eq!(
        ready["changed"],
        serde_json::json!(["app.py"]),
        "清单相对工作树里的 cwd：{ready}"
    );
    assert_eq!(
        ready["changed_line"], "改了 1 个文件（写入范围 app.py）",
        "{ready}"
    );
}

/// 工作树任务报 ready 之后在任务对话里说话：新 Attempt 还在同一个工作树，
/// 不再 git worktree add，也不回原目录写。
#[tokio::test]
async fn saying_again_keeps_the_worktree_task_in_the_same_worktree() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-read-1",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-write-1",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "first\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(2, "call-report-1", "ready", "第一轮", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));
    let task = fixture.accept_args(false, worktree_task_args(), 2).await;
    let store = tasks_store(fixture.data());
    wait_until("worktree 第一轮 ready", || {
        reported_ready(&store, &task.id)
    })
    .await;
    let before = store.task(&task.id).unwrap().unwrap();
    let first_attempt = before.latest_attempt().cloned().expect("第一个 Attempt");
    let (first_write, first_error) =
        tool_result(&thin_events(fixture.data(), &first_attempt), "call-write-1");
    assert_eq!(first_error, None, "第一个 Attempt 写失败：{first_write}");
    let first = before.latest_attempt().cloned().expect("第一个 Attempt");
    let worktree_cwd = worktree_cwd(&fixture, &task);
    let listed_before = listed_worktrees(fixture.cwd.path());
    let branches_before = git(
        fixture.cwd.path(),
        &["branch", "--list", &format!("aios/{}", task.id)],
    );
    assert_eq!(listed_before.len(), 2, "{listed_before:?}");
    // 第一轮的收尾请求可能还没跑完；等它结束再排第二轮的脚本，免得被收尾请求领走。
    wait_until("第一轮收尾", || {
        session_turn_ended(fixture.data(), &first)
    })
    .await;

    // 停了再说话：第二个 Attempt 继续改同一个文件。
    fixture.router.push_thin_for(
        task.id.as_str(),
        crate::common::tool_use_sse(
            3,
            "call-read-2",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for(
        task.id.as_str(),
        crate::common::tool_use_sse(
            4,
            "call-write-2",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "second\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for(
        task.id.as_str(),
        report_sse(4, "call-report-2", "ready", "第二轮", None),
    );
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("done"));
    let said = say_in_task_dialog(&fixture.host, &task, "再改一下").await;
    assert_eq!(said.new_attempt, Some(first.id.0 + 1));
    wait_until("第二个 Attempt 报 ready", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            attempt_count(&task) == 2
                && task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;

    let after = store.task(&task.id).unwrap().unwrap();
    let second = after.latest_attempt().expect("第二个 Attempt");
    assert_eq!(second.trigger, tasks::AttemptTrigger::UserMessage);
    assert_eq!(
        second.conversation.as_ref(),
        first.conversation.as_ref(),
        "还在原任务会话里"
    );
    wait_until("接着做的请求", || {
        fixture
            .router
            .bodies(true)
            .iter()
            .any(|body| body.contains("再改一下"))
    })
    .await;
    let resumed = fixture
        .router
        .bodies(true)
        .into_iter()
        .find(|body| body.contains("再改一下"))
        .expect("接着做的请求");
    assert_eq!(
        request_working_directory(&resumed),
        worktree_cwd.display().to_string(),
        "新 Attempt 的 cwd 仍在工作树里"
    );
    let second_events = thin_events(fixture.data(), second);
    let (second_write, second_error) = tool_result(&second_events, "call-write-2");
    assert_eq!(second_error, None, "第二个 Attempt 写失败：{second_write}");
    assert_eq!(
        std::fs::read_to_string(worktree_cwd.join("app.py")).unwrap(),
        "second\n",
        "第二个 Attempt 写进同一个工作树"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("sub").join("app.py")).unwrap(),
        "old\n",
        "原目录文件不改"
    );
    assert_eq!(
        listed_worktrees(fixture.cwd.path()),
        listed_before,
        "不再 git worktree add"
    );
    assert_eq!(
        git(
            fixture.cwd.path(),
            &["branch", "--list", &format!("aios/{}", task.id)]
        ),
        branches_before,
        "工作树分支没有多出来"
    );
}

/// 工作树任务跨一次服务重启：任务行里的工作树还在，停后说话仍开在同一棵树，不再 add。
#[tokio::test]
async fn a_worktree_task_keeps_its_tree_across_a_service_restart() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-read-1",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-write-1",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "first\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(3, "call-report-1", "ready", "第一轮", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));
    let task = fixture.accept_args(false, worktree_task_args(), 1).await;
    let store = tasks_store(fixture.data.path());
    wait_until("worktree 第一轮 ready", || {
        reported_ready(&store, &task.id)
    })
    .await;
    let before = store.task(&task.id).unwrap().unwrap();
    let first = before.latest_attempt().cloned().expect("第一个 Attempt");
    let worktree_cwd = worktree_cwd(&fixture, &task);
    let listed_before = listed_worktrees(fixture.cwd.path());
    wait_until("第一轮收尾", || {
        session_turn_ended(fixture.data.path(), &first)
    })
    .await;
    wait_until("ready relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data.path())).is_empty()
    })
    .await;

    // 模拟进程退出：换一个新根宿主，任务行里的工作树要还在。
    fixture.host.task_runtime().expect("runtime").crash();
    drop(fixture.host);
    let host = reopen_actions_host(&fixture._server, fixture.data.path(), fixture.cwd.path()).await;

    fixture.router.push_thin_for(
        task.id.as_str(),
        crate::common::tool_use_sse(
            4,
            "call-read-2",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for(
        task.id.as_str(),
        crate::common::tool_use_sse(
            5,
            "call-write-2",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "second\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for(
        task.id.as_str(),
        report_sse(6, "call-report-2", "ready", "第二轮", None),
    );
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("done"));
    fixture.router.push_kind(crate::common::text_sse("收到"));
    let said = say_in_task_dialog(&host, &task, "再改一下").await;
    assert_eq!(said.new_attempt, Some(first.id.0 + 1));
    wait_until("重启后的第二个 Attempt 报 ready", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            attempt_count(&task) == 2
                && task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;

    let after = store.task(&task.id).unwrap().unwrap();
    let second = after.latest_attempt().expect("第二个 Attempt");
    assert_eq!(second.trigger, tasks::AttemptTrigger::UserMessage);
    wait_until("重启后接着做的请求", || {
        fixture
            .router
            .bodies(true)
            .iter()
            .any(|body| body.contains("再改一下"))
    })
    .await;
    let resumed = fixture
        .router
        .bodies(true)
        .into_iter()
        .find(|body| body.contains("再改一下"))
        .expect("接着做的请求");
    assert_eq!(
        request_working_directory(&resumed),
        worktree_cwd.display().to_string(),
        "重启后的新 Attempt 的 cwd 仍在工作树里"
    );
    let second_events = thin_events(fixture.data.path(), second);
    let (write_text, write_error) = tool_result(&second_events, "call-write-2");
    assert_eq!(write_error, None, "重启后写失败：{write_text}");
    assert_eq!(
        std::fs::read_to_string(worktree_cwd.join("app.py")).unwrap(),
        "second\n",
        "重启后写进同一个工作树"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("sub").join("app.py")).unwrap(),
        "old\n",
        "原目录文件不改"
    );
    assert_eq!(
        listed_worktrees(fixture.cwd.path()),
        listed_before,
        "重启后不再 git worktree add"
    );
}

/// 确认卡接续被打断的工作树任务：新 Attempt 还在同一棵树，不再 add。
#[tokio::test]
async fn confirming_an_interrupted_worktree_task_resumes_in_the_same_worktree() {
    let (server, router, data, cwd) = restart_setup().await;
    init_git_repo(cwd.path());
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    kind_turn(
        &host,
        &router,
        data.path(),
        "kind-a",
        "派一个任务",
        1,
        vec![
            crate::common::tool_use_sse(
                0,
                "call-accept-a",
                "task_accept",
                &worktree_task_args().to_string(),
            ),
            crate::common::text_sse("已派出"),
        ],
    )
    .await;
    let store = tasks_store(data.path());
    let task = one_task(&store);

    router.push_thin_for_title("改一下判重", crate::common::text_sse("还在做"));
    router.set_thin_delay(Duration::from_secs(10));
    host.task_runtime().expect("runtime").set_pump_paused(false);
    wait_until("工作树任务在跑", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.conversation.is_some() && attempt.ended_at.is_none())
    })
    .await;
    let worktree_root = std::fs::canonicalize(data.path().join("worktrees").join(task.id.as_str()))
        .expect("工作树目录");
    let worktree_cwd = std::fs::canonicalize(worktree_root.join("sub")).expect("工作树 cwd");
    let listed_before = listed_worktrees(cwd.path());
    assert_eq!(listed_before.len(), 2, "{listed_before:?}");

    host.task_runtime().expect("runtime").crash();
    drop(host);
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &[&task], "被打断了。");
    let host = reopen_actions_host(&server, data.path(), cwd.path()).await;
    wait_until("接续卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    router.push_thin_for(task.id.as_str(), crate::common::text_sse("接着做"));
    router.push_kind(crate::common::text_sse("收到"));
    let decision = confirm_card_option(&host, "call-restart-card", task.id.as_str()).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    wait_until("接续开了新的 Attempt", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&task.id).unwrap().unwrap();
    let second = resumed.latest_attempt().unwrap().clone();
    assert_eq!(second.trigger, tasks::AttemptTrigger::Resume);
    wait_until("接续请求带上恢复说明", || {
        router
            .bodies(true)
            .iter()
            .any(|body| body.contains(RECOVERY_NOTE))
    })
    .await;
    let body = router
        .bodies(true)
        .into_iter()
        .find(|body| body.contains(RECOVERY_NOTE))
        .expect("接续请求");
    assert_eq!(
        request_working_directory(&body),
        worktree_cwd.display().to_string(),
        "接续的新 Attempt 的 cwd 仍在工作树里"
    );
    assert_eq!(
        listed_worktrees(cwd.path()),
        listed_before,
        "不再 git worktree add"
    );
    drop(host);
}

/// cwd 不是 git 仓库：不开会话，进待补足条件，卡上写固定的说明。
#[tokio::test]
async fn a_worktree_task_outside_a_repository_waits_for_conditions() {
    let fixture = Fixture::new().await;
    let mut args = worktree_task_args();
    args["cwd"] = serde_json::json!(".");
    let task = fixture.accept_args(false, args, 1).await;
    let store = tasks_store(fixture.data());
    wait_until("非仓库卡", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "blocked")
    })
    .await;

    let stored = store.task(&task.id).unwrap().unwrap();
    assert!(
        stored
            .missing
            .iter()
            .any(|item| item.as_str() == "这个目录不是 git 仓库，开不了工作树"),
        "缺项原文固定：{:?}",
        stored.missing
    );
    let attempt = stored.latest_attempt().expect("缺项 Attempt");
    assert_eq!(attempt.end, Some(AttemptEnd::NotOpened));
    assert_eq!(attempt.conversation, None, "非仓库不开会话");
    assert!(
        !fixture
            .data()
            .join("worktrees")
            .join(task.id.as_str())
            .exists(),
        "没开出工作树"
    );
    assert!(
        fixture.router.bodies(true).is_empty(),
        "非仓库不该给 ThinAgent 发请求"
    );
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::NeedsConditions,
        "进待补足条件"
    );

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let (_, card) = relays
        .iter()
        .find(|(_, body)| body["state"] == "blocked")
        .expect("blocked 卡");
    assert_eq!(card["origin"], "package");
    assert_eq!(card["task"]["stage"], "needs_conditions");
    assert_eq!(
        card["summary"], "任务包缺：这个目录不是 git 仓库，开不了工作树",
        "卡上写固定说明"
    );
}

/// 工作树任务交回、用户在 Kind 对话里同意扩大范围：新 Attempt 还在同一个工作树，
/// 新范围按同样的相对位置换算，第一条输入写明换算后的范围。
#[tokio::test]
async fn task_update_in_a_worktree_resumes_with_the_converted_scope() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    // 第一个 Attempt：越界写被拒、交回。
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-write-1",
            "write",
            &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-handback",
            "task_handback",
            &serde_json::json!({"reason": "outside.py 不在写入范围里"}).to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("交回"));
    // 第二个 Attempt：范围扩大后写成功、报 ready。
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            3,
            "call-write-2",
            "write",
            &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(4, "call-report", "ready", "改完了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));

    let task = fixture.accept_args(true, worktree_task_args(), 0).await;
    // 交回 relay 会叫醒 Kind 一轮。
    fixture.router.push_kind(crate::common::text_sse("收到卡"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("交回 relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;
    let worktree_cwd = worktree_cwd(&fixture, &task);
    let scope = worktree_cwd.join("outside.py");

    // 用户在 Kind 对话里同意扩大范围。
    let update_args = serde_json::json!({
        "task_id": task.id.as_str(),
        "write_scope": "outside.py"
    })
    .to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        5,
        "call-update",
        "task_update",
        &update_args,
    ));
    fixture
        .router
        .push_kind(crate::common::text_sse("好，范围扩大了"));
    fixture
        .router
        .push_kind(crate::common::text_sse("收到结果"));
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-2",
            "我同意把写入范围扩大到 outside.py",
        )))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 3).await;

    let store = tasks_store(fixture.data());
    wait_until("第二个 Attempt 报 ready", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            task.requirement_version() == 2
                && attempt_count(&task) == 2
                && task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;

    let updated = store.task(&task.id).unwrap().unwrap();
    let second = updated.latest_attempt().expect("第二个 Attempt").clone();
    assert_eq!(second.trigger, tasks::AttemptTrigger::RequirementChanged);
    let events = thin_events(fixture.data(), &second);
    let request_id = format!("task-update:{}:{}", updated.id, second.id);
    let note = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => match &message.source {
                model::MessageSource::User { rpc_id: Some(id) } if id == &request_id => Some(
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                ),
                _ => None,
            },
            _ => None,
        })
        .expect("新 Attempt 的第一条输入");
    assert!(
        note.contains(&scope.display().to_string()),
        "第一条输入写明换算后的范围：{note}"
    );
    assert!(note.contains("写入范围"), "{note}");

    let (write_text, write_error) = tool_result(&events, "call-write-2");
    assert_eq!(write_error, None, "{write_text}");
    assert_eq!(
        std::fs::read_to_string(&scope).unwrap(),
        "changed\n",
        "新范围里的文件在工作树里写成"
    );
    assert!(
        !fixture.cwd.path().join("sub").join("outside.py").exists(),
        "原目录没多出文件"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("sub").join("app.py")).unwrap(),
        "old\n",
        "原目录文件不改"
    );
    assert_eq!(listed_worktrees(fixture.cwd.path()).len(), 2);
}

/// 工作树任务的写入范围在原仓库外面：换算不进工作树，按缺项停机，不悄悄写回原目录。
#[tokio::test]
async fn a_worktree_task_with_an_outside_scope_waits_for_conditions() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    let outside = fixture.data().join("outside.txt");
    let mut args = worktree_task_args();
    args["write_scope"] = serde_json::json!(outside.to_string_lossy());
    let task = fixture.accept_args(false, args, 1).await;
    let store = tasks_store(fixture.data());
    wait_until("范围换算不了卡", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "blocked")
    })
    .await;

    let stored = store.task(&task.id).unwrap().unwrap();
    assert!(
        stored
            .missing
            .iter()
            .any(|item| item.as_str().contains("工作树换算会落到工作树外面")),
        "缺项说明范围换算不了：{:?}",
        stored.missing
    );
    assert_eq!(
        stored
            .latest_attempt()
            .and_then(|attempt| attempt.conversation.clone()),
        None,
        "换算不了就不开会话"
    );
    assert!(
        !fixture
            .data()
            .join("worktrees")
            .join(task.id.as_str())
            .exists(),
        "没开出工作树"
    );
    assert!(
        fixture.router.bodies(true).is_empty(),
        "不该给 ThinAgent 发请求"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
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

/// 等第一条 ready relay，返回它的 request id 和正文。
async fn ready_relay(fixture: &Fixture) -> (String, Value) {
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;
    relay_messages(&crate::common::read_root_events(fixture.data()))
        .into_iter()
        .find(|(_, body)| body["state"] == "ready")
        .expect("ready relay")
}

fn write_file_sse(call_id: &str, file_path: &str, content: &str) -> String {
    crate::common::tool_use_sse(
        1,
        call_id,
        "write",
        &serde_json::json!({"file_path": file_path, "content": content}).to_string(),
    )
}

fn read_file_sse(call_id: &str, file_path: &str) -> String {
    crate::common::tool_use_sse(
        1,
        call_id,
        "read",
        &serde_json::json!({"file_path": file_path}).to_string(),
    )
}

fn edit_file_sse(call_id: &str, file_path: &str, old_string: &str, new_string: &str) -> String {
    crate::common::tool_use_sse(
        1,
        call_id,
        "edit",
        &serde_json::json!({
            "file_path": file_path,
            "old_string": old_string,
            "new_string": new_string
        })
        .to_string(),
    )
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

/// 写任务 ready：卡状态固定「待你核」的键（review），阶段仍是待检查；
/// 清单来自所有 Attempt 的写类工具成功 diffs，去重、相对 cwd；固定一行 N/范围。
#[tokio::test]
async fn a_write_task_ready_card_is_review_with_deduped_relative_changes() {
    let fixture = Fixture::new().await;
    let outside = fixture.data().join("outside.py");
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            write_file_sse("call-w1", "a.py", "one\n"),
            write_file_sse("call-w2", "b.py", "b\n"),
            read_file_sse("call-read", "a.py"),
            edit_file_sse("call-edit", "a.py", "one", "two"),
            write_file_sse("call-outside", &outside.to_string_lossy(), "no\n"),
            report_sse(2, "call-report", "ready", "改好了", None),
            crate::common::text_sse("done"),
        ],
    )
    .await;

    let (_, body) = ready_relay(&fixture).await;
    assert_eq!(body["state"], "ready", "{body}");
    assert_eq!(body["task"]["stage"], "ready_for_review", "阶段仍是待检查");
    assert_eq!(body["card_state"], "review", "写任务 ready 的卡状态");
    assert_eq!(body["write_scope"], ".", "{body}");
    assert_eq!(
        body["changed"],
        serde_json::json!(["a.py", "b.py"]),
        "两次写同一文件与一次编辑只算一条；范围外的失败写不进清单：{body}"
    );
    assert_eq!(body["changed_line"], "改了 2 个文件（写入范围 .）");
    assert!(!body.to_string().contains("完成"), "卡上不写完成：{body}");
    assert!(body.get("bash_note").is_none(), "{body}");
    assert!(body.get("worktree_path").is_none(), "{body}");

    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);
    let stored = store.task(&task.id).unwrap().unwrap();
    assert!(stored.acceptances.is_empty(), "还没接受");
    let attempt = stored.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    assert_eq!(
        tool_result(&events, "call-outside").1,
        Some(true),
        "范围外的写确实失败"
    );
}

/// 调查任务不变：ready 的卡上没有写任务的卡状态和清单字段。
#[tokio::test]
async fn an_investigation_ready_card_has_no_write_card_data() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查好了", None));
    fixture.router.push_thin(crate::common::text_sse("done"));
    let _task = fixture.accept(true).await;
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);

    let (_, body) = ready_relay(&fixture).await;
    assert_eq!(body["state"], "ready", "{body}");
    assert_eq!(body["task"]["stage"], "ready_for_review");
    for key in [
        "write_scope",
        "changed",
        "changed_line",
        "card_state",
        "bash_note",
        "worktree_path",
    ] {
        assert!(body.get(key).is_none(), "调查任务卡上没有 {key}：{body}");
    }
}

/// 接受写任务的结果：服务端记下版本和报告，卡变「你已接受」，阶段不变；重复点击幂等。
#[tokio::test]
async fn accepting_a_ready_write_result_records_once_and_keeps_the_stage() {
    let fixture = Fixture::new().await;
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            write_file_sse("call-w1", "a.py", "one\n"),
            report_sse(2, "call-report", "ready", "改好了", None),
            crate::common::text_sse("done"),
        ],
    )
    .await;

    let (relay_id, body) = ready_relay(&fixture).await;
    assert_eq!(body["card_state"], "review", "{body}");
    assert!(!body.to_string().contains("完成"), "卡上不写完成：{body}");
    let report_id = relay_id
        .strip_prefix("task-report:")
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, report)| report.to_owned())
        .expect("结果卡的 report id");

    let decision = accept_result_card(&fixture.host, &relay_id).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    assert_eq!(decision["action"], "accept_task_result", "{decision}");
    assert_eq!(decision["data"]["summary"], "你接受了", "{decision}");
    assert_eq!(decision["data"]["version"], 1, "{decision}");
    assert_eq!(decision["data"]["report_id"], report_id, "{decision}");
    assert!(decision["data"]["at"].is_number(), "回执带时间：{decision}");
    assert!(!decision.to_string().contains("完成"));

    let store = tasks_store(fixture.data());
    let stored = store.task(&task.id).unwrap().unwrap();
    let acceptance = stored.acceptances.last().cloned().expect("接受记录");
    assert_eq!(acceptance.version, 1);
    assert_eq!(acceptance.report_id.as_str(), report_id);
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::Closed,
        "接受后任务关闭"
    );
    assert_eq!(
        card_decisions(&crate::common::read_root_events(fixture.data()), &relay_id).len(),
        1,
        "决定记进 Kind 的日志"
    );

    let second = accept_result_card(&fixture.host, &relay_id).await;
    assert_eq!(second, decision, "重复点击返回同一条决定");
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        after.acceptances, stored.acceptances,
        "重复点击不改接受记录"
    );
    assert_eq!(
        card_decisions(&crate::common::read_root_events(fixture.data()), &relay_id).len(),
        1,
        "重复点击不再记决定"
    );
}

/// 要求版本变过以后，旧版本的报告不能接受：卡上的「接受」回拒绝，接受记录不落，阶段不变。
#[tokio::test]
async fn a_report_from_before_a_requirement_change_is_refused() {
    let fixture = Fixture::new().await;
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            write_file_sse("call-w1", "a.py", "one\n"),
            report_sse(2, "call-report", "ready", "改好了", None),
            crate::common::text_sse("done"),
        ],
    )
    .await;

    let (relay_id, _) = ready_relay(&fixture).await;
    let store = tasks_store(fixture.data());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    store
        .update_write_scope(&task.id, "other.py", now + 1)
        .expect("改写入范围");

    let decision = accept_result_card(&fixture.host, &relay_id).await;
    assert_eq!(decision["phase"], "refused", "{decision}");
    let summary = decision["data"]["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("要求改过了"),
        "拒绝原因写明要求改过了：{summary}"
    );
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.requirement_version(), 2);
    assert!(stored.acceptances.is_empty(), "旧版本的报告不写接受记录");
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::ReadyForReview,
        "拒绝也不改阶段"
    );

    let again = accept_result_card(&fixture.host, &relay_id).await;
    assert_eq!(again, decision, "拒绝也幂等");
    assert_eq!(
        card_decisions(&crate::common::read_root_events(fixture.data()), &relay_id).len(),
        1
    );
}

/// 开了 bash 的写任务：卡上有固定说明，也有改过的文件清单和 N/范围一行。
#[tokio::test]
async fn a_bash_write_task_card_notes_bash_and_lists_the_changes() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            write_file_sse("call-w1", "a.py", "one\n"),
            report_sse(2, "call-report", "ready", "改好了", None),
            crate::common::text_sse("done"),
        ],
    )
    .await;
    assert!(task.bash, "受理时冻结了 bash");

    let (_, body) = ready_relay(&fixture).await;
    assert_eq!(
        body["bash_note"], "这个任务开了 bash，bash 的改动不在清单里",
        "{body}"
    );
    assert_eq!(body["changed"], serde_json::json!(["a.py"]), "{body}");
    assert_eq!(
        body["changed_line"], "改了 1 个文件（写入范围 .）",
        "{body}"
    );
    assert_eq!(body["card_state"], "review", "{body}");
}

// ===== T052.1：任务图：提议 → 确认卡 → 确认 → 第一项开跑 =====

/// T052：取一条消息里的纯文本（工具结果正文）。
fn message_text(message: &model::Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            model::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
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

/// 提议两项 A→B → 出确认卡（两行事实）→ 确认 → 只放 A；再确认不重复执行。
#[tokio::test]
async fn graph_propose_card_confirm_launches_first_subtask_only() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    host.dispatch(Call::Send(send(
        &host,
        "kind-1",
        "先查 notes.md 有几行，再把行数写进 other.txt",
    )))
    .await
    .expect("发送");
    wait_until("建图并出卡", || {
        let store = tasks_store(data.path());
        let ids = store.task_ids().unwrap_or_default();
        ids.len() == 3
            && crate::common::read_root_events(data.path())
                .iter()
                .any(|event| matches!(&event.kind, session::EventKind::ToolResult { message, meta: Some(_), .. } if message.tool_call_id.as_deref() == Some("call-graph-card")))
    })
    .await;

    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    assert_eq!(children.len(), 2);
    assert_eq!(children[1].needs.len(), 1);
    assert_eq!(children[1].needs[0].from, children[0].id);

    // Kind 工具表里有 task_graph_propose。
    let kind_body = router.bodies(false).into_iter().next().expect("Kind 请求");
    assert!(
        tool_names(&kind_body)
            .iter()
            .any(|name| name == "task_graph_propose"),
        "Kind 工具表：{:?}",
        tool_names(&kind_body)
    );

    // 确认卡：一个选项、每个子任务一行事实（标题/等谁/写入范围）。
    let card = graph_card(data.path());
    let options = card["options"].as_array().expect("options");
    assert_eq!(options.len(), 1);
    let facts = options[0]["facts"].as_array().expect("facts");
    assert_eq!(facts.len(), 2, "每项一行事实：{facts:?}");
    assert!(
        facts[1]["name"]
            .as_str()
            .unwrap_or_default()
            .contains("写结果"),
        "第二行是下游子任务：{facts:?}"
    );
    assert!(
        facts[0]["value"]
            .as_str()
            .unwrap_or_default()
            .contains("等：无"),
        "第一项不等人：{facts:?}"
    );
    assert!(
        facts[1]["value"]
            .as_str()
            .unwrap_or_default()
            .contains("等：先查行数"),
        "第二项写明等谁：{facts:?}"
    );
    assert!(
        facts[1]["value"]
            .as_str()
            .unwrap_or_default()
            .contains("写入范围：other.txt"),
        "第二项写明写入范围：{facts:?}"
    );

    let confirm = Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),

        note: None,
        report_id: None,
    });
    let Response::CardAct(response) = host.dispatch(confirm.clone()).await.expect("确认") else {
        panic!("card-act 响应");
    };
    assert!(
        response
            .decision
            .as_ref()
            .is_some_and(|decision| decision.phase == session::CardDecisionPhase::Confirmed),
        "确认成功：{response:?}"
    );

    // 只放没有 needs 的第一项。
    let claim = store
        .claim_effect(EffectKind::Launch, i64::MAX / 2)
        .expect("取效果")
        .expect("A 的 launch");
    let tasks::EffectPayload::Launch { task_id } = claim.payload.clone() else {
        panic!("launch 载荷");
    };
    assert_eq!(task_id, children[0].id);
    store.complete_effect(&claim).expect("完成 launch");
    assert!(
        store
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .expect("取效果")
            .is_none(),
        "B 不能启动"
    );
    assert_eq!(
        store.stage(&children[1].id).expect("阶段"),
        Stage::NeedsConditions
    );
    assert_eq!(
        store.stage(&root.id).expect("阶段"),
        Stage::NeedsConditions,
        "总任务汇总取最靠前的子任务阶段"
    );

    // 重复确认：原样返回已记的决定，不再执行。
    let Response::CardAct(again) = host.dispatch(confirm).await.expect("再确认") else {
        panic!("card-act 响应");
    };
    assert!(again.decision.is_some());
    assert!(
        store
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .expect("取效果")
            .is_none(),
        "重复确认不重复启动"
    );

    // 卡片出了以后 Kind 这一轮就结束了；再发一句，下一轮动态脚本读 task_status 的总任务汇总。
    host.dispatch(Call::Send(send(&host, "kind-2", "看一下任务图的状态")))
        .await
        .expect("发送第二句");
    crate::common::wait_turns(&host, data.path(), 2).await;
    // Kind 那一轮跑了 task_status 读总任务：汇总阶段 + 子任务行。
    let status_result = crate::common::read_root_events(data.path())
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult {
                message,
                error: None,
                ..
            } if message.tool_call_id.as_deref() == Some("call-graph-status") => {
                Some(message_text(message))
            }
            _ => None,
        })
        .expect("task_status 的结果");
    assert!(status_result.contains("任务图"), "汇总：{status_result}");
    assert!(
        status_result.contains("子任务数：2"),
        "汇总：{status_result}"
    );
    assert!(status_result.contains("先查行数"), "汇总：{status_result}");

    // task-read 读总任务：attempts 0、事件空（T051 的空态）。
    let Response::TaskRead(read) = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: root.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read 总任务")
    else {
        panic!("task-read 响应");
    };
    assert_eq!(read.task.attempts, 0);
    assert!(read.events.is_empty());
    assert_eq!(read.last_seq, 0);
}

/// 提议不合法（有环）：工具结果回可读原因，任务库不留半张图。
#[tokio::test]
async fn graph_propose_rejects_invalid_graph_with_reason() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);
    let bad = serde_json::json!({
        "title": "有环的图",
        "requirement": "r",
        "dod": "d",
        "subtasks": [
            {"title": "A", "requirement": "r", "dod": "d", "after": [1]},
            {"title": "B", "requirement": "r", "dod": "d", "after": [0]}
        ]
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        1,
        "call-graph",
        "task_graph_propose",
        &bad,
    ));
    router.push_kind(crate::common::text_sse("好"));
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再改")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    let result = crate::common::read_root_events(data.path())
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, error, .. }
                if message.tool_call_id.as_deref() == Some("call-graph") =>
            {
                Some((message.is_error, message_text(message), error.is_some()))
            }
            _ => None,
        })
        .expect("工具结果");
    assert!(result.0 == Some(true) || result.2, "要报错：{result:?}");
    assert!(result.1.contains("有环"), "原因可读：{}", result.1);
    assert!(
        tasks_store(data.path())
            .task_ids()
            .expect("任务 id")
            .is_empty(),
        "不合格的图不落库"
    );
}

/// 暂不：图留着、不跑；没有 launch 效果，子任务都是待补足条件。
#[tokio::test]
async fn graph_dismiss_card_keeps_it_unconfirmed_and_launches_nothing() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再改")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图并出卡", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
            && crate::common::read_root_events(data.path()).iter().any(|event| {
                matches!(&event.kind, session::EventKind::ToolResult { message, meta: Some(_), .. } if message.tool_call_id.as_deref() == Some("call-graph-card"))
            })
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    let Response::CardAct(response) = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Dismiss,
            option: None,

            note: None,
            report_id: None,
        }))
        .await
        .expect("暂不")
    else {
        panic!("card-act 响应");
    };
    assert!(
        response
            .decision
            .as_ref()
            .is_some_and(|decision| decision.phase == session::CardDecisionPhase::Dismissed)
    );
    let reloaded = graph_root(&store);
    assert!(
        reloaded
            .arrangement
            .as_ref()
            .expect("安排记录")
            .confirmed
            .is_none(),
        "暂不不算确认"
    );
    assert!(
        store
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .expect("取效果")
            .is_none(),
        "什么都不跑"
    );
    for child in children {
        assert_eq!(
            store.stage(&child.id).expect("阶段"),
            Stage::NeedsConditions
        );
    }
}

/// launch 入口统一挡：总任务不跑；没确认的图里的子任务不跑。
#[tokio::test]
async fn launch_gate_blocks_graph_root_and_unconfirmed_children() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再改")))
        .await
        .expect("发送");
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    // 手动替总任务和下游子任务各排一条 launch（模拟重启扫描/异常重放），launch 入口必须挡掉。
    store
        .enqueue_effect(
            tasks::EffectPayload::Launch {
                task_id: root.id.clone(),
            },
            1,
        )
        .expect("排总任务 launch");
    store
        .enqueue_effect(
            tasks::EffectPayload::Launch {
                task_id: children[1].id.clone(),
            },
            1,
        )
        .expect("排下游 launch");
    runtime.set_pump_paused(false);
    wait_until("launch 效果被收掉", || {
        tasks_store(data.path())
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .expect("取效果")
            .is_none()
    })
    .await;
    let store = tasks_store(data.path());
    assert!(
        store
            .task(&root.id)
            .expect("读任务")
            .expect("总任务")
            .runs
            .is_empty(),
        "总任务不跑"
    );
    assert!(
        store
            .task(&children[1].id)
            .expect("读任务")
            .expect("子任务")
            .runs
            .is_empty(),
        "没确认/上游没结果的下游不跑"
    );
}

/// T052.2：A 的 ready 报告落库的同一事务放出 B；B 开跑时任务包里有「上游结果」，needs 上记交接。
#[tokio::test]
async fn graph_ready_releases_downstream_with_the_upstream_result() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);

    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图并出卡", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    // 确认：A 进 launch，B 等 A 的 ready。
    let Response::CardAct(response) = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),

            note: None,
            report_id: None,
        }))
        .await
        .expect("确认")
    else {
        panic!("card-act 响应");
    };
    assert!(response.decision.is_some());

    // ThinAgent 脚本按任务排：A 报 ready，B 报 ready；Kind 的后续轮多备几段文本。
    let report_a = crate::common::tool_use_sse(
        2,
        "call-a-report",
        "task_report",
        r#"{"state":"ready","summary":"查完了：notes.md 有 3 行"}"#,
    );
    let report_b = crate::common::tool_use_sse(
        2,
        "call-b-report",
        "task_report",
        r#"{"state":"ready","summary":"写好了：other.txt 里是 3 行"}"#,
    );
    let text = crate::common::text_sse("好");
    router.push_thin_for(children[0].id.as_str(), report_a.clone());
    router.push_thin_for(children[0].id.as_str(), text.clone());
    router.push_thin_for(children[1].id.as_str(), report_b);
    router.push_thin_for(children[1].id.as_str(), text);
    for _ in 0..6 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    runtime.set_pump_paused(false);

    wait_until("A 报 ready", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("B 自动开跑", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .and_then(|task| {
                task.latest_attempt()
                    .and_then(|attempt| attempt.conversation.clone())
            })
            .is_some()
    })
    .await;

    let downstream = tasks_store(data.path())
        .task(&children[1].id)
        .expect("读任务")
        .expect("B 存在");
    let handoff = downstream.needs[0]
        .handoff
        .clone()
        .expect("B 开跑前解析出交接");
    assert!(handoff.summary.contains("3 行"));
    let upstream = tasks_store(data.path())
        .task(&children[0].id)
        .expect("读任务")
        .expect("A 存在");
    assert_eq!(
        handoff.report_id,
        upstream.latest_report().expect("A ready").id
    );

    // B 的任务包（assignment）里有「上游结果」一段：标题、要求版本、摘要。
    let conversation = downstream
        .latest_attempt()
        .and_then(|attempt| attempt.conversation.clone())
        .expect("B 的会话");
    let session_id = session::SessionId::parse(conversation.as_str()).expect("B 会话 id");
    let session =
        session::SessionStore::open_read_only(data.path(), &session_id).expect("B 的会话日志");
    let assignment = session
        .events()
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => {
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                serde_json::from_str::<Value>(&text).ok()
            }
            _ => None,
        })
        .expect("B 的 assignment");
    let upstream_results = assignment["upstream_results"]
        .as_array()
        .expect("上游结果一段");
    assert_eq!(upstream_results.len(), 1);
    assert!(
        upstream_results[0]["title"]
            .as_str()
            .unwrap_or_default()
            .contains("先查行数")
    );
    assert_eq!(upstream_results[0]["requirement_version"], 1);
    assert!(
        upstream_results[0]["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("3 行")
    );

    wait_until("B 也报 ready、总任务待检查", || {
        tasks_store(data.path())
            .stage(&root.id)
            .map(|stage| stage == Stage::ReadyForReview)
            .unwrap_or(false)
    })
    .await;
}

/// T052.3：全局暂停时确认任务图——先都排着，开闸后按先后跑（A 先，B 等 A 的 ready）。
#[tokio::test]
async fn graph_confirm_while_paused_queues_then_runs_in_order_after_unpause() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        paused: Some(true),
        ..Default::default()
    }))
    .await
    .expect("暂停");

    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图并出卡", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    let Response::CardAct(response) = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),

            note: None,
            report_id: None,
        }))
        .await
        .expect("确认")
    else {
        panic!("card-act 响应");
    };
    assert!(response.decision.is_some());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let store = tasks_store(data.path());
    assert!(
        store
            .task(&children[0].id)
            .expect("读任务")
            .expect("A")
            .latest_attempt()
            .is_none(),
        "暂停时确认也不开跑"
    );
    assert!(router.bodies(true).is_empty(), "暂停时不开任务会话");
    assert!(
        store
            .effect_pending(EffectKind::Launch, children[0].id.as_str())
            .expect("效果")
            || store
                .task(&children[0].id)
                .expect("读任务")
                .expect("A")
                .stage(false)
                == Stage::Pending,
        "A 的 launch 还排着"
    );

    // 开闸前备好两个子任务的 ThinAgent 脚本与 Kind 后续文本。
    router.push_thin_for(
        children[0].id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-a-report",
            "task_report",
            r#"{"state":"ready","summary":"查完了：notes.md 有 3 行"}"#,
        ),
    );
    router.push_thin_for(
        children[1].id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b-report",
            "task_report",
            r#"{"state":"ready","summary":"写好了：other.txt 里是 3 行"}"#,
        ),
    );
    for _ in 0..6 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    runtime.set_pump_paused(false);
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        paused: Some(false),
        ..Default::default()
    }))
    .await
    .expect("开闸");

    wait_until("A 开跑", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().map(|attempt| attempt.started_at))
            .is_some()
    })
    .await;
    wait_until("B 自动开跑", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().map(|attempt| attempt.started_at))
            .is_some()
    })
    .await;
    let store = tasks_store(data.path());
    let a_started = store
        .task(&children[0].id)
        .expect("读任务")
        .expect("A")
        .latest_attempt()
        .expect("A Attempt")
        .started_at;
    let b_started = store
        .task(&children[1].id)
        .expect("读任务")
        .expect("B")
        .latest_attempt()
        .expect("B Attempt")
        .started_at;
    assert!(a_started <= b_started, "开闸后按先后跑：A 先于 B");
    let total = store.task(&root.id).expect("读任务").expect("总任务");
    assert!(total.latest_attempt().is_none(), "总任务始终不跑");
}

/// T052.3：A 跑着时重启——A 被打断；B 和总任务不出被打断，B 仍是待补足条件。
#[tokio::test]
async fn graph_restart_interrupts_running_child_but_not_root_or_waiting_sibling() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),

        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");
    // A 的 ThinAgent 压后 10 秒，进程退出前它会一直挂着。
    router.push_thin_for(children[0].id.as_str(), crate::common::text_sse("还在做"));
    router.set_thin_delay(Duration::from_secs(10));
    runtime.set_pump_paused(false);
    wait_until("A 在跑", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.conversation.is_some() && attempt.ended_at.is_none())
    })
    .await;
    runtime.crash();
    drop(host);
    for _ in 0..6 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    let mut reopened = false;
    for _ in 0..250 {
        let host = open_graph_host(&server, data.path(), cwd.path()).await;
        if matches!(host.info().root, service::RootState::Ok) {
            reopened = true;
            let store = tasks_store(data.path());
            wait_until("启动扫描收尾", || {
                store
                    .task(&children[0].id)
                    .ok()
                    .flatten()
                    .and_then(|task| task.latest_attempt().cloned())
                    .is_some_and(|attempt| attempt.end.is_some())
            })
            .await;
            let store = tasks_store(data.path());
            let a = store.task(&children[0].id).expect("读任务").expect("A");
            assert_eq!(
                a.latest_attempt().and_then(|attempt| attempt.end),
                Some(tasks::AttemptEnd::Interrupted),
                "A 被打断了"
            );
            let b = store.task(&children[1].id).expect("读任务").expect("B");
            assert!(b.latest_attempt().is_none(), "B 没开跑，不出被打断");
            assert!(!b.is_interrupted());
            let total = store.task(&root.id).expect("读任务").expect("总任务");
            assert!(total.latest_attempt().is_none(), "总任务没有 Attempt");
            assert!(!total.is_interrupted());
            assert_eq!(
                store.stage(&children[1].id).expect("阶段"),
                Stage::NeedsConditions
            );
            assert_eq!(store.stage(&root.id).expect("阶段"), Stage::NeedsConditions);
            drop(host);
            break;
        }
        drop(host);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reopened, "重开根宿主 250 次都没到 RootState::Ok");
}

/// T052.3：A 报回 ready 的那一刻崩溃——重启后 B 照样被放出，且只放一次（效果表重放）。
#[tokio::test]
async fn graph_crash_after_ready_releases_downstream_exactly_once_after_restart() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),

        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");
    router.push_thin_for(
        children[0].id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-a-report",
            "task_report",
            r#"{"state":"ready","summary":"查完了：notes.md 有 3 行"}"#,
        ),
    );
    router.push_thin_for(
        children[1].id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b-report",
            "task_report",
            r#"{"state":"ready","summary":"写好了：other.txt 里是 3 行"}"#,
        ),
    );
    // A 的报告和「放出 B」的 launch 已经落库；relay 还没投递就崩溃。
    runtime.crash_before_next_relay();
    runtime.set_pump_paused(false);
    wait_until("A 的 ready 落库", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("B 的 launch 留在队列里", || {
        tasks_store(data.path())
            .effect_pending(EffectKind::Launch, children[1].id.as_str())
            .unwrap_or(false)
    })
    .await;
    drop(host);
    for _ in 0..8 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    let mut reopened = false;
    for _ in 0..250 {
        let host = open_graph_host(&server, data.path(), cwd.path()).await;
        if matches!(host.info().root, service::RootState::Ok) {
            reopened = true;
            wait_until("重启后 B 开跑", || {
                tasks_store(data.path())
                    .task(&children[1].id)
                    .ok()
                    .flatten()
                    .and_then(|task| task.latest_attempt().cloned())
                    .is_some_and(|attempt| attempt.conversation.is_some())
            })
            .await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let store = tasks_store(data.path());
            let b = store.task(&children[1].id).expect("读任务").expect("B");
            let attempts = b.runs.iter().map(|run| run.attempts.len()).sum::<usize>();
            assert_eq!(attempts, 1, "崩溃重放只放一次");
            assert!(
                !store
                    .effect_pending(EffectKind::Launch, children[1].id.as_str())
                    .unwrap_or(false),
                "没有残留的 launch"
            );
            drop(host);
            break;
        }
        drop(host);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reopened, "重开根宿主 250 次都没到 RootState::Ok");
}

/// T052.2 F1：上游二次 ready 更新摘要后，下游第二次 Attempt 的会话里随接续带上「上游结果（最新）」。
#[tokio::test]
async fn resume_attempt_rehands_the_latest_upstream_result() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图并出卡", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),

        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");

    let report_a1 = crate::common::tool_use_sse(
        2,
        "call-a1-report",
        "task_report",
        r#"{"state":"ready","summary":"第一次结果：notes.md 有 3 行"}"#,
    );
    let report_b1 = crate::common::tool_use_sse(
        2,
        "call-b1-report",
        "task_report",
        r#"{"state":"ready","summary":"第一版写好了"}"#,
    );
    router.push_thin_for(children[0].id.as_str(), report_a1);
    router.push_thin_for(children[0].id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(children[1].id.as_str(), report_b1);
    router.push_thin_for(children[1].id.as_str(), crate::common::text_sse("好"));
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    wait_until("A1 ready", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("B1 开跑", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .and_then(|task| {
                task.latest_attempt()
                    .and_then(|attempt| attempt.conversation.clone())
            })
            .is_some()
    })
    .await;
    wait_until("A1 轮结束", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;

    // A 第二次 ready：更新交接摘要。
    let report_a2 = crate::common::tool_use_sse(
        2,
        "call-a2-report",
        "task_report",
        r#"{"state":"ready","summary":"第二次结果：notes.md 现在 5 行"}"#,
    );
    router.push_thin_for(children[0].id.as_str(), report_a2);
    router.push_thin_for(children[0].id.as_str(), crate::common::text_sse("好"));
    host.dispatch(Call::TaskSay(service::TaskSayRequest {
        task_id: children[0].id.as_str().to_owned(),
        text: "再查一遍".to_owned(),
        pastes: Vec::new(),
        attachments: Vec::new(),
    }))
    .await
    .expect("A2 输入");
    wait_until("A2 ready 且摘要更新", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_report()
                        .is_some_and(|report| report.summary.contains("5 行"))
            })
    })
    .await;

    // B 第二次 Attempt：接续时应该带上最新的上游结果。
    wait_until("B1 轮结束", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    let report_b2 = crate::common::tool_use_sse(
        2,
        "call-b2-report",
        "task_report",
        r#"{"state":"ready","summary":"照最新的 5 行写好了"}"#,
    );
    router.push_thin_for(children[1].id.as_str(), report_b2);
    router.push_thin_for(children[1].id.as_str(), crate::common::text_sse("好"));
    host.dispatch(Call::TaskSay(service::TaskSayRequest {
        task_id: children[1].id.as_str().to_owned(),
        text: "照最新的再写一遍".to_owned(),
        pastes: Vec::new(),
        attachments: Vec::new(),
    }))
    .await
    .expect("B2 输入");
    wait_until("B2 开跑", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
            })
    })
    .await;
    let b = tasks_store(data.path())
        .task(&children[1].id)
        .expect("读任务")
        .expect("B");
    assert_eq!(
        b.runs.iter().map(|run| run.attempts.len()).sum::<usize>(),
        2,
        "A2 不再重新放 B（只有两次 Attempt）"
    );
    let conversation = b
        .latest_attempt()
        .and_then(|attempt| attempt.conversation.clone())
        .expect("B2 会话");
    let session_id = session::SessionId::parse(conversation.as_str()).expect("会话 id");
    let session = session::SessionStore::open_read_only(data.path(), &session_id).expect("B2 日志");
    let texts: Vec<String> = session
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("上游结果（最新）") && text.contains("5 行")),
        "B2 会话里要有最新的上游结果：{texts:?}"
    );
}

/// T053.1：门任务和被把关任务按标题找（动态脚本建的名字固定）。
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

/// T053.1：建好门图、确认、B 报回、门报回，返回手头的事实；调用方再判或改动。
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

#[tokio::test]
async fn gate_pass_flow_releases_guarded_downstream_after_the_user_verdict() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (root, guarded, downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    // 判之前：B 是待检查，C 没开跑。
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::ReadyForReview);
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::ReadyForReview);
    assert!(downstream.latest_attempt().is_none(), "C 还没开跑");
    assert!(
        !store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );

    // 用户点通过：判定记下，C 被放出。
    let response = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("判定");
    let Response::GateJudge(judged) = response else {
        panic!("gate-judge 响应");
    };
    assert!(!judged.changed, "不该是已经变了");
    assert!(!judged.duplicate);
    assert_eq!(judged.released, vec!["再写结果".to_owned()]);
    assert_eq!(
        judged.verdict.as_ref().map(|verdict| verdict.conclusion),
        Some(GateJudgeConclusion::Pass)
    );

    wait_until("C 开跑", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.latest_attempt().is_some())
    })
    .await;
    let store = tasks_store(data.path());
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::RequirementMet);
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::RequirementMet);
    let b = store.task(&guarded.id).unwrap().unwrap();
    let c = store.task(&downstream.id).unwrap().unwrap();
    let b_report = b.latest_report().unwrap();
    assert_eq!(
        c.needs[0]
            .handoff
            .as_ref()
            .map(|handoff| handoff.report_id.clone()),
        Some(b_report.id.clone()),
        "C 交接的是判通过的那份"
    );

    // 同一份门报告再判：返回已有判定，不记第二次。
    let again = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::NeedsFix,
            note: None,
        }))
        .await
        .expect("再判");
    let Response::GateJudge(again) = again else {
        panic!("gate-judge 响应");
    };
    assert!(again.duplicate);
    assert_eq!(
        again.verdict.as_ref().map(|verdict| verdict.conclusion),
        Some(GateJudgeConclusion::Pass)
    );
    assert_eq!(
        tasks_store(data.path())
            .task(&gate.id)
            .unwrap()
            .unwrap()
            .verdicts
            .len(),
        1
    );
    let _ = root;
}

#[tokio::test]
async fn gate_judge_rejects_stale_report_and_candidate_changed() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, _downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    // 不是门的最新报告：已经变了，不记。
    let stale = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: "deadbeefdeadbeefdeadbeefdeadbeef".to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("旧报告判定");
    let Response::GateJudge(stale) = stale else {
        panic!("gate-judge 响应");
    };
    assert!(stale.changed);
    assert!(stale.verdict.is_none());
    assert_eq!(
        tasks_store(data.path())
            .task(&gate.id)
            .unwrap()
            .unwrap()
            .verdicts
            .len(),
        0
    );

    // B 又交一份新结果 → 候选变了；这时点旧门报告：已经变了，不记。
    router.push_thin_for(
        guarded.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b2-report",
            "task_report",
            r#"{"state":"ready","summary":"第二次结果：5 行"}"#,
        ),
    );
    router.push_thin_for(guarded.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        gate.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-gate2-report",
            "task_report",
            r#"{"state":"ready","summary":"第二次核查"}"#,
        ),
    );
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("好"));
    wait_until("B 第一轮结束", || {
        tasks_store(data.path())
            .task(&guarded.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    wait_until("门第一轮结束", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    host.dispatch(Call::TaskSay(TaskSayRequest {
        task_id: guarded.id.as_str().to_owned(),
        text: "再改一版".to_owned(),
        pastes: Vec::new(),
        attachments: Vec::new(),
    }))
    .await
    .expect("B 第二轮");
    wait_until("门重跑到第二份候选", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_report()
                        .is_some_and(|report| report.summary.contains("第二次核查"))
            })
    })
    .await;
    let stale_again = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("候选变后的旧报告判定");
    let Response::GateJudge(stale_again) = stale_again else {
        panic!("gate-judge 响应");
    };
    assert!(stale_again.changed, "候选已经变了");
    let gate_now = tasks_store(data.path()).task(&gate.id).unwrap().unwrap();
    assert!(gate_now.verdicts.is_empty(), "已经变了不记判定");
    assert_eq!(
        gate_now.needs[0].handoff.as_ref().unwrap().summary,
        "第二次结果：5 行"
    );
}

#[tokio::test]
async fn gate_blocked_keeps_guarded_task_and_downstream_untouched() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    // 门脚本换成 blocked：B 报回 ready 后门开跑，门报 blocked。
    router.enable_gate_kind();
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再核再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建门图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 4)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
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
    router.push_thin_for(
        gate.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-gate-blocked",
            "task_report",
            r#"{"state":"blocked","summary":"缺原始链接，核不了","question":"要查什么？"}"#,
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
    wait_until("门报 blocked", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Blocked)
    })
    .await;
    let store = tasks_store(data.path());
    let b = store.task(&guarded.id).unwrap().unwrap();
    assert_eq!(
        b.latest_report().map(|report| report.state),
        Some(ReportState::Ready),
        "B 不动"
    );
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::ReadyForReview);
    assert!(
        store
            .task(&downstream.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_none()
    );
    assert!(
        !store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );
}

/// T053.1：判定刚落库就崩溃 → 重启后下游恰好放一次、判定 relay 恰好送一次。
#[tokio::test]
async fn gate_verdict_crash_replays_downstream_launch_and_relay_once() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    let guarded_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();

    // 判定事务提交后、relay 投递前崩溃。
    host.task_runtime()
        .expect("runtime")
        .crash_before_next_relay();
    let response = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("判定");
    let Response::GateJudge(judged) = response else {
        panic!("gate-judge 响应");
    };
    assert!(!judged.changed, "判定要记下");
    // 下游的 launch 还在效果表里（或崩溃前已经被领走，但绝不该重复）。
    wait_until("C 的 launch 留在队列或已开跑", || {
        let store = tasks_store(data.path());
        store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap_or(false)
            || store
                .task(&downstream.id)
                .ok()
                .flatten()
                .is_some_and(|task| task.latest_attempt().is_some())
    })
    .await;
    drop(host);
    // 重开根宿主（模拟进程重启）。
    let mut reopened = false;
    for _ in 0..250 {
        let host = open_graph_host(&server, data.path(), cwd.path()).await;
        if matches!(host.info().root, service::RootState::Ok) {
            reopened = true;
            wait_until("重启后 C 开跑", || {
                tasks_store(data.path())
                    .task(&downstream.id)
                    .ok()
                    .flatten()
                    .and_then(|task| task.latest_attempt().cloned())
                    .is_some_and(|attempt| attempt.conversation.is_some())
            })
            .await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let store = tasks_store(data.path());
            let c = store.task(&downstream.id).unwrap().unwrap();
            let attempts = c.runs.iter().map(|run| run.attempts.len()).sum::<usize>();
            assert_eq!(attempts, 1, "崩溃重放只放一次");
            assert_eq!(
                c.needs[0]
                    .handoff
                    .as_ref()
                    .map(|handoff| handoff.report_id.clone()),
                Some(guarded_report.id.clone()),
                "交接的是判通过的那份"
            );
            assert_eq!(
                store.task(&gate.id).unwrap().unwrap().verdicts.len(),
                1,
                "判定只记一次"
            );
            // 判定 relay 恰好一次。
            let count_verdict_relay = || {
                crate::common::read_root_events(data.path())
                    .iter()
                    .filter(|event| {
                        let session::EventKind::UserMessage { message, .. } = &event.kind else {
                            return false;
                        };
                        matches!(
                            &message.source,
                            model::MessageSource::User { rpc_id: Some(rpc_id) }
                                if rpc_id.starts_with("gate-verdict:")
                        )
                    })
                    .count()
            };
            wait_until("判定 relay 送到", || count_verdict_relay() >= 1).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(count_verdict_relay(), 1, "判定 relay 只送一次");
            drop(host);
            break;
        }
        drop(host);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reopened, "重开根宿主 250 次都没到 RootState::Ok");
}

/// T053.2：判需修正 → relay 带原话 → task_continue → 新 Attempt 任务包含原话和核查发现 → 门自动重跑 → 通过。
#[tokio::test]
async fn gate_needs_fix_continue_and_auto_recheck_end_to_end() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    let note = "空标题的别直接放过，用域名加路径当键";
    // 用户判需修正（带原话）：relay 记下结论、原话和候选。
    let response = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::NeedsFix,
            note: Some(note.to_owned()),
        }))
        .await
        .expect("判定需修正");
    let Response::GateJudge(judged) = response else {
        panic!("gate-judge 响应");
    };
    assert_eq!(
        judged
            .verdict
            .as_ref()
            .and_then(|verdict| verdict.note.clone()),
        Some(note.to_owned()),
        "原话原样存档"
    );
    wait_until("Kind 收到需修正 relay", || {
        crate::common::read_root_events(data.path())
            .iter()
            .any(|event| {
                let session::EventKind::UserMessage { message, .. } = &event.kind else {
                    return false;
                };
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                text.contains("gate_verdict") && text.contains(note) && text.contains("needs_fix")
            })
    })
    .await;

    // Kind 用 task_continue 让被把关任务在原要求里接着做。
    router.push_thin_for(
        guarded.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b2-report",
            "task_report",
            r#"{"state":"ready","summary":"第二次结果：5 行"}"#,
        ),
    );
    router.push_thin_for(guarded.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        gate.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-gate2-report",
            "task_report",
            r#"{"state":"ready","summary":"第二次核查：这次按域名加路径"}"#,
        ),
    );
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        downstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-c-report",
            "task_report",
            r#"{"state":"ready","summary":"写好了"}"#,
        ),
    );
    router.push_thin_for(downstream.id.as_str(), crate::common::text_sse("好"));
    let continued = host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: guarded.id.as_str().to_owned(),
            note: Some(note.to_owned()),
        }))
        .await
        .expect("接着做");
    let Response::TaskContinue(continued) = continued else {
        panic!("task-continue 响应");
    };
    wait_until("B 第二次 Attempt 开跑", || {
        tasks_store(data.path())
            .task(&guarded.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_attempt()
                        .is_some_and(|attempt| attempt.conversation.is_some())
            })
    })
    .await;
    let store = tasks_store(data.path());
    let b = store.task(&guarded.id).unwrap().unwrap();
    let attempt = b.latest_attempt().unwrap();
    assert_eq!(attempt.id.0, continued.attempt_id);
    match &attempt.trigger {
        tasks::AttemptTrigger::KindContinue { cause, note } => {
            assert_eq!(
                note.as_deref(),
                Some("空标题的别直接放过，用域名加路径当键")
            );
            assert!(matches!(cause, tasks::ContinueCause::GateNeedsFix { .. }));
        }
        other => panic!("trigger 是 {other:?}"),
    }
    // 任务包里有「你要按这个改」：用户原话 + 核查发现。
    let user_texts = |data: &Path| -> Vec<String> {
        thin_events(data, b.latest_attempt().expect("B 最新 Attempt"))
            .iter()
            .filter_map(|event| match &event.kind {
                session::EventKind::UserMessage { message, .. } => Some(
                    message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect()
    };
    wait_until("B 的二次任务包写进会话", || {
        user_texts(data.path())
            .iter()
            .any(|text| text.contains("你要按这个改"))
    })
    .await;
    let texts = user_texts(data.path());
    assert!(
        texts.iter().any(|text| text.contains("你要按这个改")
            && text.contains(note)
            && text.contains("核查发现：")
            && text.contains("空标题会漏判")),
        "任务包要带原话和核查发现：{texts:?}"
    );

    // B 再报 ready → 门自动重跑第 2 份候选；判通过后 C 才开。
    wait_until("门重跑第 2 份", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_report()
                        .is_some_and(|report| report.summary.contains("第二次核查"))
            })
    })
    .await;
    let store = tasks_store(data.path());
    let gate_now = store.task(&gate.id).unwrap().unwrap();
    let second_gate_report = gate_now.latest_report().cloned().unwrap();
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::ReadyForReview);
    let pass = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: second_gate_report.id.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("判通过");
    let Response::GateJudge(pass) = pass else {
        panic!("gate-judge 响应");
    };
    assert_eq!(pass.released, vec!["再写结果".to_owned()]);
    wait_until("C 开跑", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.latest_attempt().is_some())
    })
    .await;
    assert_eq!(
        tasks_store(data.path()).stage(&guarded.id).unwrap(),
        Stage::RequirementMet
    );
}

/// T053.2：task_continue 的拒绝项——没有可认的 cause、任务在跑。
#[tokio::test]
async fn task_continue_rejects_without_cause_or_while_running() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, _downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    // 用户还没判：没有可认的 cause。
    let error = host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: guarded.id.as_str().to_owned(),
            note: None,
        }))
        .await
        .expect_err("没有 cause 应拒绝");
    assert!(
        error.to_string().contains("没找到让它接着做的事实"),
        "拒绝原因：{error}"
    );

    // 判需修正并接着做；它还在跑时再 continue 一次要拒绝。
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    host.dispatch(Call::GateJudge(GateJudgeRequest {
        gate_task: gate.id.as_str().to_owned(),
        gate_report: gate_report.as_str().to_owned(),
        conclusion: GateJudgeConclusion::NeedsFix,
        note: Some("再改一版".to_owned()),
    }))
    .await
    .expect("判需修正");
    // 判完等上一轮 Attempt 收尾落库：并发跑测试时 record_report 后的 end 写入可能稍晚。
    wait_until("B 上一轮 Attempt 结束", || {
        tasks_store(data.path())
            .task(&guarded.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    router.set_thin_delay(Duration::from_secs(2));
    router.push_thin_for(guarded.id.as_str(), crate::common::text_sse("还在改"));
    // 并发跑整套时执行席位/会话写者可能还没让出来，服务按 Later 回「现在接不上」；
    // 用户再点一次就行，这里也重试到接上（第一次 Attempt 没有副作用，重试安全）。
    let mut continued = false;
    for _ in 0..100 {
        match host
            .dispatch(Call::TaskContinue(service::TaskContinueRequest {
                task_id: guarded.id.as_str().to_owned(),
                note: Some("再改一版".to_owned()),
            }))
            .await
        {
            Ok(_) => {
                continued = true;
                break;
            }
            Err(error) if error.to_string().contains("没有空位或会话还在收尾") => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!("第一次接着做：{error}"),
        }
    }
    assert!(continued, "第一次接着做一直接不上");
    wait_until("B 在跑", || {
        tasks_store(data.path())
            .task(&guarded.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_none())
    })
    .await;
    let error = host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: guarded.id.as_str().to_owned(),
            note: Some("再改一版".to_owned()),
        }))
        .await
        .expect_err("在跑应拒绝");
    assert!(error.to_string().contains("还在跑"), "拒绝原因：{error}");
}

/// T053.2：门报 blocked 后，Kind 用 task_continue AfterReport 让门重跑。
#[tokio::test]
async fn task_continue_on_blocked_gate_reruns_with_after_report_cause() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    router.enable_gate_kind();
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再核再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建门图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 4)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let (guarded, gate, _downstream) = gate_children(&store, &root);
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
            "call-gate-blocked",
            "task_report",
            r#"{"state":"blocked","summary":"缺原始链接，核不了","question":"要查什么？"}"#,
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
    wait_until("门报 blocked", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Blocked)
    })
    .await;
    // 门这一轮先收尾（会话写锁放开）再接着做。
    wait_until("门第一轮结束", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    // Kind 用 task_continue 让门重跑（原因：最近报告 blocked）。
    router.set_thin_delay(Duration::from_millis(30));
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("重新核过了"));
    // 会话写锁可能还在收尾：像 Kind 一样稍后重试（工具回的是「稍后再试一次」）。
    let mut response = None;
    for _ in 0..100 {
        match host
            .dispatch(Call::TaskContinue(service::TaskContinueRequest {
                task_id: gate.id.as_str().to_owned(),
                note: None,
            }))
            .await
        {
            Ok(response_now) => {
                response = Some(response_now);
                break;
            }
            Err(error) if error.to_string().contains("稍后再试") => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("门重跑失败：{error}"),
        }
    }
    let response = response.expect("门重跑一直接不上");
    let Response::TaskContinue(response) = response else {
        panic!("task-continue 响应");
    };
    wait_until("门第二次 Attempt 开跑", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.latest_attempt()
                    .is_some_and(|attempt| attempt.id.0 == response.attempt_id)
            })
    })
    .await;
    let gate_now = tasks_store(data.path()).task(&gate.id).unwrap().unwrap();
    let attempt = gate_now.latest_attempt().unwrap();
    match &attempt.trigger {
        tasks::AttemptTrigger::KindContinue { cause, .. } => assert!(
            matches!(cause, tasks::ContinueCause::AfterReport { .. }),
            "cause 是 {cause:?}"
        ),
        other => panic!("trigger 是 {other:?}"),
    }
}

/// T053.3：上游 A 又交新结果 → B 需重评、Kind 收到 relay、阶段不回退 → task_continue 按新结果核对 → 清除。
#[tokio::test]
async fn upstream_new_result_marks_reevaluation_and_continue_checks_again() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    let (upstream, downstream) = (children[0].clone(), children[1].clone());
    router.push_thin_for(
        upstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-a1-report",
            "task_report",
            r#"{"state":"ready","summary":"A 第一次结果"}"#,
        ),
    );
    router.push_thin_for(upstream.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        downstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b1-report",
            "task_report",
            r#"{"state":"ready","summary":"B 第一次结果"}"#,
        ),
    );
    router.push_thin_for(downstream.id.as_str(), crate::common::text_sse("好"));
    for _ in 0..12 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),
        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");
    wait_until("B 第一次报回", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    // A 第二轮：说话接续，再报一份新结果。
    wait_until("A 第一轮结束", || {
        tasks_store(data.path())
            .task(&upstream.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    router.push_thin_for(
        upstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-a2-report",
            "task_report",
            r#"{"state":"ready","summary":"A 第二次结果：5 行"}"#,
        ),
    );
    router.push_thin_for(upstream.id.as_str(), crate::common::text_sse("好"));
    host.dispatch(Call::TaskSay(TaskSayRequest {
        task_id: upstream.id.as_str().to_owned(),
        text: "再查一版".to_owned(),
        pastes: Vec::new(),
        attachments: Vec::new(),
    }))
    .await
    .expect("A 第二轮输入");
    wait_until("A 第二次报回", || {
        tasks_store(data.path())
            .task(&upstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_report()
                        .is_some_and(|report| report.summary.contains("5 行"))
            })
    })
    .await;
    wait_until("B 需重评", || {
        tasks_store(data.path())
            .needs_reevaluation(&downstream.id)
            .unwrap_or(false)
    })
    .await;
    // 阶段不回退，Kind 收到 upstream_changed relay。
    let store = tasks_store(data.path());
    assert_eq!(store.stage(&downstream.id).unwrap(), Stage::ReadyForReview);
    wait_until("Kind 收到上游变了 relay", || {
        crate::common::read_root_events(data.path())
            .iter()
            .any(|event| {
                let session::EventKind::UserMessage { message, .. } = &event.kind else {
                    return false;
                };
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                text.contains("upstream_changed") && text.contains("5 行")
            })
    })
    .await;
    let upstream_task = tasks_store(data.path())
        .task(&upstream.id)
        .unwrap()
        .unwrap();
    let latest_upstream = upstream_task.latest_report().cloned().unwrap();
    // Kind 用 task_continue 让 B 按新结果核对。
    router.push_thin_for(
        downstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b2-report",
            "task_report",
            r#"{"state":"ready","summary":"B 按新结果核对完了"}"#,
        ),
    );
    router.push_thin_for(downstream.id.as_str(), crate::common::text_sse("好"));
    let continued = host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: downstream.id.as_str().to_owned(),
            note: None,
        }))
        .await
        .expect("按新结果核对");
    let Response::TaskContinue(continued) = continued else {
        panic!("task-continue 响应");
    };
    wait_until("B 第二次 Attempt 开跑", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.latest_attempt()
                    .is_some_and(|attempt| attempt.id.0 == continued.attempt_id)
            })
    })
    .await;
    let b = tasks_store(data.path())
        .task(&downstream.id)
        .unwrap()
        .unwrap();
    let attempt = b.latest_attempt().unwrap();
    match &attempt.trigger {
        tasks::AttemptTrigger::KindContinue { cause, .. } => {
            assert!(
                matches!(
                    cause,
                    tasks::ContinueCause::UpstreamChanged { upstream_report }
                        if *upstream_report == latest_upstream.id
                ),
                "cause 是 {cause:?}"
            );
        }
        other => panic!("trigger 是 {other:?}"),
    }
    assert_eq!(
        attempt.handoffs[0].report_id, latest_upstream.id,
        "这次 Attempt 交接的是上游新结果"
    );
    wait_until("B 第二次 ready", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_report()
                        .is_some_and(|report| report.summary.contains("核对完"))
            })
    })
    .await;
    assert!(
        !tasks_store(data.path())
            .needs_reevaluation(&downstream.id)
            .unwrap(),
        "新结果报回后需重评清除"
    );
    assert_eq!(
        tasks_store(data.path()).stage(&downstream.id).unwrap(),
        Stage::ReadyForReview
    );
}

/// T053.4：点暂不 → 图取消（Dismissed），一项不跑；再点确认已变。
#[tokio::test]
async fn graph_dismissed_on_card_cancels_the_graph() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).unwrap();
    let dismissed = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Dismiss,
            option: None,
            note: None,
            report_id: None,
        }))
        .await
        .expect("暂不");
    let Response::CardAct(dismissed) = dismissed else {
        panic!("card-act 响应");
    };
    assert!(dismissed.decision.is_some());
    let store = tasks_store(data.path());
    let cancelled = store
        .task(&root.id)
        .unwrap()
        .unwrap()
        .arrangement
        .unwrap()
        .cancelled
        .expect("图要有取消记录");
    assert_eq!(cancelled.reason, tasks::CancellationReason::Dismissed);
    for child in &children {
        assert!(
            store
                .task(&child.id)
                .unwrap()
                .unwrap()
                .latest_attempt()
                .is_none()
        );
        assert_eq!(store.stage(&child.id).unwrap(), Stage::NeedsConditions);
        assert!(
            !store
                .effect_pending(EffectKind::Launch, child.id.as_str())
                .unwrap()
        );
    }
    // 再点确认：返回已有的暂不决定，不执行、不启动。
    let again = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),
            note: None,
            report_id: None,
        }))
        .await
        .expect("再点");
    let Response::CardAct(again) = again else {
        panic!("card-act 响应");
    };
    assert_eq!(
        again.decision.as_ref().map(|decision| decision.phase),
        Some(session::CardDecisionPhase::Dismissed)
    );
    assert!(
        tasks_store(data.path())
            .children(&root.id)
            .unwrap()
            .iter()
            .all(|child| child.latest_attempt().is_none())
    );
}

/// T053.4：Kind 一轮提了图却没上卡 → 下一轮开始前按 NoCard 取消。
#[tokio::test]
async fn uncarded_graph_is_cancelled_as_no_card() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_nocard_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    assert!(
        store
            .task(&root.id)
            .unwrap()
            .unwrap()
            .arrangement
            .unwrap()
            .cancelled
            .is_none()
    );
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.dispatch(Call::Send(send(&host, "kind-2", "再问一句")))
        .await
        .expect("第二轮");
    wait_until("NoCard 取消", || {
        tasks_store(data.path())
            .task(&root.id)
            .ok()
            .flatten()
            .and_then(|task| task.arrangement)
            .and_then(|arrangement| arrangement.cancelled)
            .is_some_and(|cancelled| cancelled.reason == tasks::CancellationReason::NoCard)
    })
    .await;
}

/// T053.4：新卡接受后，之前没点过的旧图按 Replaced 取消。
#[tokio::test]
async fn replacing_card_cancels_the_earlier_graph() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_two_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("第一张图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    // 下一轮再提一张图一张卡（同一轮只能出一张卡）。
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    router.enable_two_graph_second();
    host.dispatch(Call::Send(send(&host, "kind-2", "换一版")))
        .await
        .expect("第二轮");
    crate::common::wait_turns(&host, data.path(), 2).await;
    wait_until("两张图都建好", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 6)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let roots = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .filter(Task::is_graph_root)
        .collect::<Vec<_>>();
    let first_root = roots
        .iter()
        .find(|task| task.title == "第一张图")
        .cloned()
        .expect("第一张图");
    let second_root = roots
        .iter()
        .find(|task| task.title == "第二张图")
        .cloned()
        .expect("第二张图");
    let confirmed = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-two-card-2".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),
            note: None,
            report_id: None,
        }))
        .await
        .expect("确认第二张卡");
    let Response::CardAct(confirmed) = confirmed else {
        panic!("card-act 响应");
    };
    assert!(confirmed.decision.is_some());
    let store = tasks_store(data.path());
    let second = store.task(&second_root.id).unwrap().unwrap();
    assert!(
        second
            .arrangement
            .as_ref()
            .and_then(|arrangement| arrangement.confirmed.as_ref())
            .is_some()
    );
    let first = store.task(&first_root.id).unwrap().unwrap();
    assert_eq!(
        first
            .arrangement
            .unwrap()
            .cancelled
            .expect("第一张图要被替换取消")
            .reason,
        tasks::CancellationReason::Replaced
    );
    assert!(
        store
            .children(&first_root.id)
            .unwrap()
            .iter()
            .all(|child| child.latest_attempt().is_none())
    );
}

/// T054.1：调查任务没有写集，卡上也有「接受」；接受后任务关闭。
#[tokio::test]
async fn an_investigation_card_can_be_accepted_and_closes_the_task() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查好了：3 行", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let (relay_id, body) = ready_relay(&fixture).await;
    assert_eq!(body["task"]["id"], task.id.as_str());
    let report_id = body["report_id"].as_str().expect("卡上报告 id").to_owned();

    // T054：card-act 带卡上的 report id。
    let response = fixture
        .host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: relay_id.clone(),
            verb: CardVerb::Confirm,
            option: None,
            note: None,
            report_id: Some(report_id.clone()),
        }))
        .await
        .expect("接受");
    let decision = serde_json::to_value(response).unwrap()["decision"].clone();
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    assert_eq!(decision["data"]["summary"], "你接受了", "{decision}");
    assert_eq!(decision["data"]["report_id"], report_id, "{decision}");
    assert!(decision["data"]["at"].is_number(), "{decision}");

    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.acceptances.len(), 1);
    assert_eq!(
        stored.acceptances[0].report_id.as_str(),
        report_id,
        "接受绑卡上的报告"
    );
}

/// T054.1：同一任务后面又交了新结果，旧卡上的接受回「已经变了」，不记。
#[tokio::test]
async fn accepting_an_older_report_after_a_new_one_is_refused() {
    let fixture = Fixture::new().await;
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            report_sse(2, "call-report-1", "ready", "第一次结果", None),
            crate::common::text_sse("好"),
            report_sse(
                2,
                "call-report-2",
                "ready",
                "第二次结果：又核对了一遍",
                None,
            ),
            crate::common::text_sse("好"),
        ],
    )
    .await;
    let (first_relay, _) = ready_relay(&fixture).await;
    fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "再核对一遍".to_owned(),
            pastes: Vec::new(),
            attachments: Vec::new(),
        }))
        .await
        .expect("再核对");
    wait_until("第二张结果卡 relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .filter(|(id, body)| {
                id.starts_with("task-report:") && body["task"]["id"] == task.id.as_str()
            })
            .count()
            >= 2
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let second_relay = relays
        .iter()
        .map(|(id, _)| id.clone())
        .find(|id| id != &first_relay && id.starts_with("task-report:"))
        .expect("第二张结果卡");
    assert_ne!(first_relay, second_relay);

    let decision = accept_result_card(&fixture.host, &first_relay).await;
    assert_eq!(decision["phase"], "refused", "{decision}");
    let summary = decision["data"]["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("已经变了"), "回原因：{summary}");
    let store = tasks_store(fixture.data());
    assert!(
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .acceptances
            .is_empty()
    );
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::ReadyForReview,
        "拒绝不改阶段"
    );

    // 最新那张能接受。
    let decision = accept_result_card(&fixture.host, &second_relay).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
}

/// T054.1：门没判通过前，接受被服务端拒（正常按钮也不会出现）。
#[tokio::test]
async fn accepting_before_the_gate_passes_is_refused() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, _downstream, _gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let relay = relay_messages(&crate::common::read_root_events(data.path()))
        .into_iter()
        .find(|(_, body)| body["task"]["id"] == guarded.id.as_str())
        .map(|(id, _)| id)
        .expect("B 结果卡");
    let decision = accept_result_card(&host, &relay).await;
    assert_eq!(decision["phase"], "refused", "{decision}");
    let summary = decision["data"]["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("核查还没判通过"), "回原因：{summary}");
    assert!(
        tasks_store(data.path())
            .task(&guarded.id)
            .unwrap()
            .unwrap()
            .acceptances
            .is_empty()
    );
}

/// T054.1：判通过后接受 → 被把关任务和门任务关闭；下游也接受后总任务关闭。
#[tokio::test]
async fn accepting_after_the_gate_passes_closes_gate_task_and_graph() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (root, guarded, downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    wait_until("B 结果卡 relay", || {
        relay_messages(&crate::common::read_root_events(data.path()))
            .iter()
            .any(|(_, body)| body["task"]["id"] == guarded.id.as_str())
    })
    .await;
    let relay_of = |task_id: &str| -> String {
        let relays = relay_messages(&crate::common::read_root_events(data.path()));
        relays
            .into_iter()
            .find(|(_, body)| body["task"]["id"].as_str() == Some(task_id))
            .map(|(id, _)| id)
            .expect("结果卡 relay")
    };
    let guarded_relay = relay_of(guarded.id.as_str());

    // 判通过后 C 自动开跑：先备好 C 的报告脚本再判。
    router.push_thin_for(
        downstream.id.as_str(),
        report_sse(2, "call-c-report", "ready", "估算：少了 3 条", None),
    );
    router.push_thin_for(downstream.id.as_str(), crate::common::text_sse("好"));
    let judged = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("判通过");
    assert!(matches!(judged, Response::GateJudge(_)));

    let decision = accept_result_card(&host, &guarded_relay).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    let store = tasks_store(data.path());
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::Closed);
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::Closed);

    wait_until("C 结果卡 relay", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
            && relay_messages(&crate::common::read_root_events(data.path()))
                .iter()
                .any(|(_, body)| body["task"]["id"] == downstream.id.as_str())
    })
    .await;
    let downstream_relay = relay_of(downstream.id.as_str());
    let decision = accept_result_card(&host, &downstream_relay).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    assert_eq!(store.stage(&downstream.id).unwrap(), Stage::Closed);
    assert_eq!(store.stage(&root.id).unwrap(), Stage::Closed);
}

/// T054.2：等 Kind 根会话真正空闲（没有在跑的轮）再发下一条；
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

/// T054.2：带结果卡发消息 → 模型看到一行文字 → Kind 出 update_task_requirement 确认卡 →
/// 确认后同一任务开第 2 次 Attempt（RequirementChanged）、版本 2、新结果卡带版本。
#[tokio::test]
async fn update_task_requirement_card_round_trip_opens_a_new_attempt() {
    let fixture = Fixture::new_with_actions().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report-v1", "ready", "查好了 v1", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let task_id = task.id.as_str().to_owned();
    let title = task.title.clone();
    let requirement = "找答案；空标题也别丢，用域名加路径当键";
    // 改要求那一轮按请求内容动态回：触发词 → skill；skill 结果 → propose_action。
    // 不排静态队列，relay 轮和这条消息谁先跑都不影响脚本顺序。
    fixture.router.enable_update_kind(
        &task_id,
        vec![(
            "用户要求修改这张结果卡".to_owned(),
            "call-update-skill".to_owned(),
            "call-update-card".to_owned(),
            requirement.to_owned(),
        )],
    );
    let (_relay_id, body) = ready_relay(&fixture).await;
    assert_eq!(body["requirement_version"], 1, "{body}");
    // 报告落库和 Attempt 收尾是两个写；Kind 出卡前要求「不在跑」，等收尾落稳再发。
    wait_until("v1 Attempt 结束", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    wait_kind_idle(&fixture.host).await;

    fixture
        .host
        .dispatch(Call::Send(SendRequest {
            request_id: "card-send-1".to_owned(),
            content: "空标题也别丢，用域名加路径当键".to_owned(),
            delivery: service::DeliveryMode::FollowUp,
            pastes: Vec::new(),
            attachments: Vec::new(),
            canvas: None,
            result_card: Some(service::SendResultCard {
                task_id: task_id.clone(),
                title: title.clone(),
                version: 1,
            }),
        }))
        .await
        .expect("带结果卡发消息");
    let mut card1 = false;
    for _ in 0..900 {
        if crate::common::read_root_events(fixture.data())
            .iter()
            .any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::ToolResult { message, meta, .. }
                        if message.tool_call_id.as_deref() == Some("call-update-card")
                            && meta.as_ref().is_some_and(|meta| meta.get("card").is_some())
                )
            })
        {
            card1 = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(card1, "Kind 提出改要求卡");

    // 模型收到的请求里有那行文字（排在用户文字前面）。
    let expected =
        format!("[用户要求修改这张结果卡：任务 {task_id}「{title}」，第 1 版要求的结果]");
    let bodies = fixture.router.bodies(false);
    let saw_line = bodies
        .iter()
        .any(|body| body.contains(&expected) && body.contains("空标题也别丢，用域名加路径当键"));
    assert!(saw_line, "模型请求里要有结果卡那行文字");

    // 新 Attempt 的脚本备好，再点确认。
    fixture.router.push_thin_for(
        task_id.as_str(),
        report_sse(1, "call-report-v2", "ready", "查好了 v2", None),
    );
    fixture
        .router
        .push_thin_for(task_id.as_str(), crate::common::text_sse("好"));
    let confirmed = fixture
        .host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-update-card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),
            note: None,
            report_id: None,
        }))
        .await
        .expect("确认改要求");
    let decision = serde_json::to_value(confirmed).unwrap()["decision"].clone();
    assert_eq!(decision["phase"], "confirmed", "{decision}");

    wait_until("新 Attempt 开跑", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().map(|attempt| attempt.id.0))
            == Some(2)
    })
    .await;
    let store = tasks_store(fixture.data());
    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.requirement_version(), 2);
    assert_eq!(updated.requirement, requirement);
    match &updated.latest_attempt().expect("新 Attempt").trigger {
        tasks::AttemptTrigger::RequirementChanged => {}
        other => panic!("trigger 是 {other:?}"),
    }
    assert_eq!(
        updated.requirement_of_version(1),
        Some("找答案"),
        "第 1 版文本保留"
    );

    wait_until("第 2 版结果 relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["requirement_version"] == 2)
    })
    .await;
    let (v2_relay, v2_body) = relay_messages(&crate::common::read_root_events(fixture.data()))
        .into_iter()
        .find(|(_, body)| body["requirement_version"] == 2)
        .expect("第 2 版 relay");
    assert_eq!(v2_body["task"]["id"], task.id.as_str());

    // 接受第 2 版 → 已关闭；旧接受历史里没有第 1 版（这一轮才第一次接受）。
    let accepted = accept_result_card(&fixture.host, &v2_relay).await;
    assert_eq!(accepted["phase"], "confirmed", "{accepted}");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    let closed = store.task(&task.id).unwrap().unwrap();
    assert_eq!(closed.acceptances.len(), 1);
    assert_eq!(closed.acceptances[0].version, 2);
}

/// T054.2：已关闭的任务还能再改要求：新版本、同任务新 Attempt、旧接受留在历史。
#[tokio::test]
async fn a_closed_task_can_change_requirement_again_and_keeps_old_acceptance() {
    let fixture = Fixture::new_with_actions().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report-v1", "ready", "查好了 v1", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let task_id = task.id.as_str().to_owned();
    // 已关闭后从 Kind 说一句再改：按请求内容动态回（触发词 → skill；skill 结果 → propose_action）。
    fixture.router.enable_update_kind(
        &task_id,
        vec![(
            "再改一点：按域名归并".to_owned(),
            "call-update-skill-2".to_owned(),
            "call-update-card-2".to_owned(),
            "再按域名归并一次".to_owned(),
        )],
    );
    let (_relay_id, _) = ready_relay(&fixture).await;
    wait_kind_idle(&fixture.host).await;
    wait_until("v1 Attempt 结束", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    // 直接用任务库的接受把任务关掉：这条用例要的是「已关闭还能改要求」，
    // 接受回执本身由 T054.1 的 card-act 用例覆盖；走卡片会多起一轮 Kind，抢回放脚本。
    let store = tasks_store(fixture.data());
    let v1_report = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .expect("第 1 版报告")
        .id
        .clone();
    let old_acceptance = store
        .accept(&task.id, &v1_report, 2_000)
        .expect("接受第 1 版");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-2",
            "再改一点：按域名归并",
        )))
        .await
        .expect("发送");
    let mut card2 = false;
    for _ in 0..900 {
        if crate::common::read_root_events(fixture.data())
            .iter()
            .any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::ToolResult { message, meta, .. }
                        if message.tool_call_id.as_deref() == Some("call-update-card-2")
                            && meta.as_ref().is_some_and(|meta| meta.get("card").is_some())
                )
            })
        {
            card2 = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(card2, "第二张改要求卡");

    fixture.router.push_thin_for(
        task_id.as_str(),
        report_sse(1, "call-report-v2", "ready", "按域名归并好了", None),
    );
    fixture
        .router
        .push_thin_for(task_id.as_str(), crate::common::text_sse("好"));
    fixture
        .host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-update-card-2".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),
            note: None,
            report_id: None,
        }))
        .await
        .expect("确认再改要求");
    wait_until("第 2 次运行开始", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().map(|attempt| attempt.id.0))
            == Some(2)
    })
    .await;
    let store = tasks_store(fixture.data());
    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.requirement_version(), 2);
    assert_eq!(updated.requirement, "再按域名归并一次");
    assert_eq!(updated.acceptances, vec![old_acceptance], "旧接受留历史");
    assert_ne!(store.stage(&task.id).unwrap(), Stage::Closed);
}

/// T054.2：动作的三种拒绝：在排队、在跑、要求和 DoD 都没变；reconcile 回「要求改过了」。
#[tokio::test]
async fn update_task_requirement_action_rejects_queued_running_and_unchanged() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await; // pause：launch 还排在队里
    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    let events = crate::common::read_root_events(fixture.data());
    let env = service::ActionEnv {
        data_dir: fixture.data(),
        identity: None,
        events: &events,
    };
    let queued = action.describe(
        &serde_json::json!({"task_id": task.id.as_str(), "requirement": "新要求"}),
        &env,
    );
    assert!(
        queued
            .as_ref()
            .err()
            .is_some_and(|reason| reason.contains("排队")),
        "{queued:?}"
    );

    // 开闸后任务在跑：拒绝。
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("任务开跑", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_none())
    })
    .await;
    let events = crate::common::read_root_events(fixture.data());
    let env = service::ActionEnv {
        data_dir: fixture.data(),
        identity: None,
        events: &events,
    };
    let running = action.describe(
        &serde_json::json!({"task_id": task.id.as_str(), "requirement": "新要求"}),
        &env,
    );
    assert!(
        running
            .as_ref()
            .err()
            .is_some_and(|reason| reason.contains("还在跑")),
        "{running:?}"
    );
}

/// T054.2：动作的 reconcile 在卡上「改前」已经不符时回固定原因；要求和 DoD 都没变也被拒。
#[tokio::test]
async fn update_task_requirement_reconcile_refuses_after_a_change() {
    let fixture = Fixture::new_with_actions().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report-v1", "ready", "查好了 v1", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let (relay_id, _) = ready_relay(&fixture).await;
    let _ = relay_id;
    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    let events = crate::common::read_root_events(fixture.data());
    let env = service::ActionEnv {
        data_dir: fixture.data(),
        identity: None,
        events: &events,
    };
    let task_id = task.id.as_str().to_owned();

    let details = serde_json::json!({
        "task_id": task_id,
        "requirement": "第一版之外的第二个要求",
    });
    let facts = action.describe(&details, &env).expect("能出卡");
    assert!(
        facts[1].value.contains("第 1 版 → 第 2 版"),
        "{}",
        facts[1].value
    );
    // 要求和 DoD 都没变：describe 拒绝。
    let unchanged = action.describe(
        &serde_json::json!({"task_id": task_id, "requirement": "找答案"}),
        &env,
    );
    assert!(
        unchanged
            .as_ref()
            .err()
            .is_some_and(|reason| reason.contains("都没变")),
        "{unchanged:?}"
    );

    // 有人先把要求改成了别的（例如另一张卡先确认）：这张卡再确认要回固定原因。
    let store = tasks_store(fixture.data());
    store
        .update_requirement(&task.id, "别人先改成的第三版要求", None, 9_000)
        .expect("先改要求");
    let refused = action.reconcile(&details, &facts, &env);
    assert_eq!(
        refused.err().as_deref(),
        Some("这个任务的要求已经改过了，这张卡是按改之前写的")
    );
}

/// T055.2：直接造一个 ready 任务（组接受用例的输入；调用面本身才是被测的接缝）。
fn seed_ready_task(
    store: &Store,
    cwd: &std::path::Path,
    title: &str,
    at: i64,
) -> (tasks::TaskId, tasks::ReportId) {
    let task = store
        .create_task(
            NewTask {
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

/// T055.1：从调用驱动 Work——任务库 + Kind 会话派生成一份快照，结果卡在「等你」里带 pick。
#[tokio::test]
async fn work_call_returns_task_facts_and_the_accept_wait_item() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-work-report", "ready", "查好了", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    wait_until("任务报回 ready", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
                    && task
                        .latest_attempt()
                        .is_some_and(|attempt| attempt.ended_at.is_some())
            })
    })
    .await;

    let response = fixture
        .host
        .dispatch(service::Call::Work(service::WorkRequest {}))
        .await
        .expect("work");
    let service::Response::Work(work) = response else {
        panic!("work 响应");
    };
    assert!(work.generated_at > 0);
    let fact = work
        .tasks
        .iter()
        .find(|fact| fact.id == task.id.as_str())
        .expect("任务事实");
    assert_eq!(fact.title, task.title);
    assert_eq!(fact.stage, "ready_for_review");
    assert_eq!(fact.version, 1);
    assert!(!fact.running);
    let latest = fact.latest.as_ref().expect("最近报告");
    assert_eq!(latest.state, "ready");
    assert_eq!(latest.summary, "查好了");

    let accept = work
        .wait
        .iter()
        .find(|item| item.kind == "accept" && item.task_id.as_deref() == Some(task.id.as_str()))
        .expect("结果等你接受");
    let pick = accept.pick.as_ref().expect("pick");
    assert_eq!(pick.report_id, latest.report_id);
    assert_eq!(pick.version, 1);
    assert!(!accept.title.is_empty());
    assert!(!accept.need.is_empty());
}

/// T055.2：从调用驱动组接受——一项旧报告逐项回原话，另一项照记；被接受了才关闭。
#[tokio::test]
async fn task_accept_many_call_accepts_the_fresh_one_and_refuses_the_stale_one() {
    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let (fresh_id, fresh_report) =
        seed_ready_task(&store, fixture.data(), "新结果", 1_800_000_000_000);
    let (stale_id, stale_report) =
        seed_ready_task(&store, fixture.data(), "旧结果", 1_800_000_010_000);
    // 旧任务再开一次 Attempt，交一份新结果：勾选时看到的那份报告已经不是最新。
    let claim = store
        .claim_effect(EffectKind::Launch, 1_800_000_020_000)
        .unwrap();
    if let Some(claim) = claim {
        store.complete_effect(&claim).unwrap();
    }
    let attempt = store.begin_attempt(&stale_id, 1_800_000_021_000).unwrap();
    store
        .record_report(
            &stale_id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "更新的结果".to_owned(),
                question: None,
            },
            1_800_000_022_000,
        )
        .unwrap();

    let request = service::TaskAcceptManyRequest {
        picks: vec![
            service::work::WorkPick {
                task_id: stale_id.as_str().to_owned(),
                report_id: stale_report.as_str().to_owned(),
                version: 1,
            },
            service::work::WorkPick {
                task_id: fresh_id.as_str().to_owned(),
                report_id: fresh_report.as_str().to_owned(),
                version: 1,
            },
        ],
    };
    let response = fixture
        .host
        .dispatch(Call::TaskAcceptMany(request))
        .await
        .expect("task-accept-many");
    let Response::TaskAcceptMany(response) = response else {
        panic!("task-accept-many 响应");
    };
    assert!(response.accepted_at > 0);
    assert_eq!(response.outcomes.len(), 2);
    assert_eq!(response.outcomes[0].task_id, stale_id.as_str());
    assert!(
        response.outcomes[0]
            .refused
            .as_deref()
            .is_some_and(|reason| reason.contains("已经变了")),
        "{:?}",
        response.outcomes[0].refused
    );
    assert_eq!(response.outcomes[1].task_id, fresh_id.as_str());
    assert!(response.outcomes[1].refused.is_none());
    assert_eq!(store.stage(&fresh_id).unwrap(), Stage::Closed);
    assert_eq!(store.stage(&stale_id).unwrap(), Stage::ReadyForReview);
}

/// T055.3 审计备注 2：从没开跑的任务改要求——先查后写，回同样的话、任务库不动。
#[tokio::test]
async fn update_task_requirement_act_rejects_a_never_started_task_without_writing() {
    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let task = store
        .create_task(
            NewTask {
                title: "没开跑".to_owned(),
                requirement: "原要求".to_owned(),
                dod: "原 DoD".to_owned(),
                cwd: fixture.data().to_path_buf(),
                conversation: ConversationId::new("kind-work"),
                model: ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: vec!["read".to_owned()],
            },
            1_800_000_000_000,
        )
        .unwrap();
    // 把排队中的 launch 效果收掉但不开始 Attempt：模拟「从没开跑」的任务。
    let claim = store
        .claim_effect(EffectKind::Launch, 1_800_000_001_000)
        .unwrap()
        .expect("launch 在排队");
    store.complete_effect(&claim).unwrap();
    let before = store.task(&task.id).unwrap().unwrap();
    assert!(before.latest_attempt().is_none());

    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    let refused = service::ActionKind::act(
        action.as_ref(),
        &serde_json::json!({"task_id": task.id.as_str(), "requirement": "新要求"}),
        &fixture.host,
    )
    .await
    .expect_err("从没开跑不能改");
    assert!(refused.contains("还没开跑"), "{refused}");

    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(after.requirement, before.requirement, "不写库");
    assert_eq!(after.dod, before.dod, "不写库");
    assert_eq!(
        after.requirement_version(),
        before.requirement_version(),
        "版本不动"
    );
    assert_eq!(after.versions.len(), before.versions.len(), "版本不动");
}

/// T056.1：组接受每记下一项，给那份报告的 relay 卡补一条 card/decision（Kind 卡上回执）。
#[tokio::test]
async fn task_accept_many_records_a_card_decision_for_each_accepted_report() {
    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let (task_id, report_id) =
        seed_ready_task(&store, fixture.data(), "组接受回执", 1_800_100_000_000);
    let response = fixture
        .host
        .dispatch(Call::TaskAcceptMany(service::TaskAcceptManyRequest {
            picks: vec![service::work::WorkPick {
                task_id: task_id.as_str().to_owned(),
                report_id: report_id.as_str().to_owned(),
                version: 1,
            }],
        }))
        .await
        .expect("task-accept-many");
    let Response::TaskAcceptMany(response) = response else {
        panic!("task-accept-many 响应");
    };
    assert!(response.outcomes[0].refused.is_none());

    let call_id = format!("task-report:{}:{}", task_id.as_str(), report_id.as_str());
    let events = crate::common::read_root_events(fixture.data());
    let decision = events.iter().find_map(|event| {
        let session::EventKind::CardDecision {
            call_id: event_call,
            action,
            phase,
            data,
            ..
        } = &event.kind
        else {
            return None;
        };
        (event_call == &call_id).then(|| (action.clone(), *phase, data.clone()))
    });
    let (action, phase, data) = decision.expect("接受后应补 card/decision");
    assert_eq!(action.as_deref(), Some("accept_task_result"));
    assert_eq!(phase, session::CardDecisionPhase::Confirmed);
    let data = data.expect("回执 data");
    assert_eq!(data["summary"], "你接受了");
    assert_eq!(data["report_id"], report_id.as_str());
}

#[tokio::test]
async fn task_accept_constraints_must_be_verbatim_user_words() {
    let fixture = Fixture::new().await;
    let task = fixture
        .accept_args(
            false,
            serde_json::json!({
                "title": "带约束的调查",
                "requirement": "找答案",
                "dod": "有答案",
                "constraints": ["改一下"],
            }),
            0,
        )
        .await;
    assert_eq!(task.current_constraints(), ["改一下"]);
    // 任务包第一条 assignment 的 ledger 字段带着这条约束（ThinAgent 能看到）。
    let mut assignment = String::new();
    for _ in 0..250 {
        let latest = tasks_store(fixture.data())
            .task(&task.id)
            .unwrap()
            .expect("任务还在");
        let Some(conversation) = latest
            .latest_attempt()
            .and_then(|attempt| attempt.conversation.clone())
        else {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        };
        let id = session::SessionId::parse(conversation.as_str()).expect("会话 id");
        let Ok(log) = session::SessionStore::open_read_only(fixture.data(), &id) else {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        };
        let text_of = |message: &model::Message| -> Option<String> {
            let text = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect::<Vec<_>>()
                .join(
                    "
",
                );
            (text.trim_start().starts_with('{') && text.contains("ledger")).then_some(text)
        };
        let found = log.events().iter().find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => text_of(message),
            session::EventKind::InboxSpliced { inserted, .. } => inserted.iter().find_map(&text_of),
            _ => None,
        });
        if let Some(text) = found {
            assignment = text;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!assignment.is_empty(), "没等到 assignment");
    let package: Value = serde_json::from_str(&assignment).expect("assignment JSON");
    let ledger = package["ledger"].as_str().expect("assignment 带 ledger");
    assert!(ledger.contains("「改一下」"), "{ledger}");
    assert!(ledger.contains("你说的"), "{ledger}");
}

#[tokio::test]
async fn task_accept_rejects_a_constraint_the_user_never_said() {
    let fixture = Fixture::new().await;
    let args = serde_json::json!({
        "title": "伪造约束",
        "requirement": "找答案",
        "dod": "有答案",
        "constraints": ["用户没说过这句"],
    });
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept-bad-constraint",
        "task_accept",
        &args.to_string(),
    ));
    fixture
        .router
        .push_kind(crate::common::text_sse("没派出去"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-1", "改一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    assert!(tasks_store(fixture.data()).task_ids().unwrap().is_empty());
    let events = crate::common::read_root_events(fixture.data());
    let found = events.iter().any(|event| {
        serde_json::to_string(&event.kind)
            .unwrap_or_default()
            .contains("约束要用用户原话")
    });
    assert!(found, "Kind 会话里要留下拒绝原因");
}

#[tokio::test]
async fn task_read_exposes_ledger_constraints_notes_and_history() {
    let fixture = Fixture::new().await;
    let task = fixture
        .accept_args(
            false,
            serde_json::json!({
                "title": "读账本",
                "requirement": "找答案",
                "dod": "有答案",
                "constraints": ["改一下"],
            }),
            0,
        )
        .await;
    let store = tasks_store(fixture.data());
    let attempt = task.latest_attempt().expect("Attempt").id;
    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[tasks::NewLedgerNote {
                kind: tasks::LedgerNoteKind::Rejected,
                topic: "cache".to_owned(),
                text: "缓存不可行".to_owned(),
            }],
            1_700_000_000_000_i64,
        )
        .unwrap();
    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[tasks::NewLedgerNote {
                kind: tasks::LedgerNoteKind::Rejected,
                topic: "cache".to_owned(),
                text: "缓存换成手工".to_owned(),
            }],
            1_700_000_000_001_i64,
        )
        .unwrap();
    let response = fixture
        .host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(response) = response else {
        panic!("task-read 响应");
    };
    assert_eq!(response.ledger.constraints.len(), 1);
    assert_eq!(response.ledger.constraints[0].text, "改一下");
    assert!(response.ledger.constraints[0].from.is_none());
    assert_eq!(response.ledger.notes.len(), 1);
    assert_eq!(response.ledger.notes[0].text, "缓存换成手工");
    assert_eq!(response.ledger.history.len(), 1);
    assert_eq!(response.ledger.history[0].text, "缓存不可行");
    assert_eq!(response.ledger.history[0].closed_why, "被新的一条替换");
}

/// T056.2.2：改要求动作的 constraints 必须逐字是用户原话；对不上整次拒、不写库。
#[tokio::test]
async fn update_requirement_constraints_must_be_user_words_and_land_on_the_new_version() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查好了", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("任务这一轮结束", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    // 报告落库后会话可能还在收尾；等写者让出来再改要求，避免 open_resume 撞 AlreadyOwned。
    wait_until("任务会话写者让出", || {
        let task = tasks_store(fixture.data()).task(&task.id).ok().flatten();
        let Some(conversation) = task.and_then(|task| {
            task.latest_attempt()
                .and_then(|attempt| attempt.conversation.clone())
        }) else {
            return false;
        };
        let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
            return false;
        };
        session::SessionStore::open(fixture.data(), &id).is_ok()
    })
    .await;
    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    let before = store.task(&task.id).unwrap().unwrap();

    let refused = service::ActionKind::act(
        action.as_ref(),
        &serde_json::json!({
            "task_id": task.id.as_str(),
            "requirement": "新要求",
            "constraints": ["用户没说过这句"],
        }),
        &fixture.host,
    )
    .await
    .expect_err("非原话要整次拒");
    assert!(refused.contains("约束要用用户原话"), "{refused}");
    let unchanged = store.task(&task.id).unwrap().unwrap();
    assert_eq!(unchanged.requirement, before.requirement);
    assert_eq!(unchanged.dod, before.dod);
    assert_eq!(
        unchanged.requirement_version(),
        before.requirement_version()
    );
    assert_eq!(unchanged.versions.len(), before.versions.len());
    assert!(unchanged.current_constraints().is_empty());
    assert_eq!(unchanged.versions, before.versions);

    let summary = service::ActionKind::act(
        action.as_ref(),
        &serde_json::json!({
            "task_id": task.id.as_str(),
            "requirement": "按用户原话改的新要求",
            "constraints": ["改一下"],
        }),
        &fixture.host,
    )
    .await
    .expect("用户原话约束应成功");
    assert!(summary.summary.contains("第 2 版"), "{}", summary.summary);
    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.requirement, "按用户原话改的新要求");
    assert_eq!(
        updated.requirement_version(),
        before.requirement_version() + 1
    );
    assert_eq!(updated.current_constraints(), ["改一下"]);
}

/// T056.2.2：改要求不传 constraints 时沿用上一版。
#[tokio::test]
async fn update_requirement_without_constraints_keeps_the_previous_version() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查好了", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture
        .accept_args(
            false,
            serde_json::json!({
                "title": "沿约束",
                "requirement": "找答案",
                "dod": "有答案",
                "constraints": ["改一下"],
            }),
            0,
        )
        .await;
    let store = tasks_store(fixture.data());
    wait_until("任务这一轮结束", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    wait_until("任务会话写者让出", || {
        let task = tasks_store(fixture.data()).task(&task.id).ok().flatten();
        let Some(conversation) = task.and_then(|task| {
            task.latest_attempt()
                .and_then(|attempt| attempt.conversation.clone())
        }) else {
            return false;
        };
        let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
            return false;
        };
        session::SessionStore::open(fixture.data(), &id).is_ok()
    })
    .await;
    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    service::ActionKind::act(
        action.as_ref(),
        &serde_json::json!({
            "task_id": task.id.as_str(),
            "requirement": "只改要求文字，不传约束",
        }),
        &fixture.host,
    )
    .await
    .expect("改要求成功");
    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.current_constraints(), ["改一下"]);
}
