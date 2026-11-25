//! 假模型路由、两个宿主（Kind/带任务运行时）的 fixture 与启动辅助。

use super::*;

/// 假模型按请求体路由：工具表里有 task_accept 的是 Kind，有 task_report 的是 ThinAgent。
#[derive(Clone)]
pub(super) struct Router {
    pub(super) state: Arc<RouterState>,
}

/// 改要求 Kind 的动态脚本：按请求内容（触发词/skill 结果）出 skill→propose_action，
/// 不依赖静态队列顺序（relay 轮和用户消息的先后会抢脚本）。
pub(super) struct UpdateProposal {
    pub(super) trigger: String,
    pub(super) skill_call: String,
    pub(super) card_call: String,
    pub(super) requirement: String,
    pub(super) quote: String,
    pub(super) skill_emitted: bool,
    pub(super) card_emitted: bool,
}

pub(super) struct UpdateKind {
    pub(super) task_id: String,
    pub(super) proposals: Vec<UpdateProposal>,
}

pub(super) struct RouterState {
    pub(super) kind: Mutex<VecDeque<String>>,
    pub(super) thin: Mutex<VecDeque<String>>,
    /// 按任务的 ThinAgent 脚本：多任务同时跑时，各自按 assignment 里的 task_id 取。
    pub(super) thin_by_task: Mutex<HashMap<String, VecDeque<String>>>,
    /// 任务 id 要受理后才知道；测试先按标题排脚本（assignment 里也有标题）。
    pub(super) thin_by_title: Mutex<HashMap<String, VecDeque<String>>>,
    /// 先让 ThinAgent 吃一次 HTTP 错误（用量等待测试）；字段是状态码 + Retry-After。
    pub(super) thin_errors: Mutex<VecDeque<(u16, Option<String>)>>,

    pub(super) bodies: Mutex<Vec<(bool, String)>>,
    pub(super) unexpected: Mutex<Vec<(bool, String)>>,
    /// ThinAgent 的响应延迟：live 测试要在任务还在跑时接上事件流。
    pub(super) thin_delay: Mutex<Option<Duration>>,
    /// Kind 的动态脚本阶段（0=出确认卡、1=task_status、2=收尾）；None = 用静态队列。
    pub(super) graph_kind: Mutex<Option<u8>>,
    /// 带里程碑门的任务图动态脚本阶段；None = 用静态队列。
    pub(super) gate_kind: Mutex<Option<u8>>,
    /// 只提图不上的动态脚本（NoCard 收尾）。
    pub(super) nocard_kind: Mutex<Option<u8>>,
    /// 连续提两张图两张卡（替换取消）。
    pub(super) two_graph_kind: Mutex<Option<u8>>,
    /// 按请求内容驱动的改要求 Kind 脚本。
    pub(super) update_kind: Mutex<Option<UpdateKind>>,
}

impl Router {
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(RouterState {
                kind: Mutex::new(VecDeque::new()),
                thin: Mutex::new(VecDeque::new()),
                thin_by_task: Mutex::new(HashMap::new()),
                thin_by_title: Mutex::new(HashMap::new()),
                thin_errors: Mutex::new(VecDeque::new()),
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

    /// 让 Kind 的第二轮起用动态脚本（先 propose_action 出卡，再 task_status，最后收尾）。
    pub(super) fn enable_graph_kind(&self) {
        *self.state.graph_kind.lock().unwrap() = Some(0);
    }

    /// Kind 用带里程碑门的动态脚本（skill → 提议三子任务图 → 出确认卡 → 收尾）。
    pub(super) fn enable_gate_kind(&self) {
        *self.state.gate_kind.lock().unwrap() = Some(0);
    }

    /// 只提图不出的动态脚本（下一轮开始时按 NoCard 取消）。
    pub(super) fn enable_nocard_kind(&self) {
        *self.state.nocard_kind.lock().unwrap() = Some(0);
    }

    /// 两轮各提一张图一张卡（第二张卡接受时第一张按 Replaced 取消）。
    pub(super) fn enable_two_graph_kind(&self) {
        *self.state.two_graph_kind.lock().unwrap() = Some(0);
    }

    /// 第二轮脚本（下一轮再提一张图一张卡；同一轮仍只让出一张卡）。
    pub(super) fn enable_two_graph_second(&self) {
        *self.state.two_graph_kind.lock().unwrap() = Some(10);
    }

    /// 改要求 Kind 的动态脚本（trigger, skill_call, card_call, requirement, quote）。
    pub(super) fn enable_update_kind(
        &self,
        task_id: &str,
        proposals: Vec<(String, String, String, String, String)>,
    ) {
        let proposals = proposals
            .into_iter()
            .map(
                |(trigger, skill_call, card_call, requirement, quote)| UpdateProposal {
                    trigger,
                    skill_call,
                    card_call,
                    requirement,
                    quote,
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
    pub(super) fn take_update_kind_script(&self, body: &str) -> Option<String> {
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
                            "quote": proposal.quote,
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

    pub(super) fn set_thin_delay(&self, delay: Duration) {
        *self.state.thin_delay.lock().unwrap() = Some(delay);
    }

    pub(super) fn clear_thin_delay(&self) {
        *self.state.thin_delay.lock().unwrap() = None;
    }

    pub(super) fn push_kind(&self, body: impl Into<String>) {
        self.state.kind.lock().unwrap().push_back(body.into());
    }

    pub(super) fn push_thin_error(&self, status: u16, retry_after: Option<&str>) {
        self.state
            .thin_errors
            .lock()
            .unwrap()
            .push_back((status, retry_after.map(str::to_owned)));
    }

    pub(super) fn push_thin(&self, body: impl Into<String>) {
        self.state.thin.lock().unwrap().push_back(body.into());
    }

    /// 只给这个任务的 ThinAgent 排回复（按 assignment 里的 task_id 分）。
    pub(super) fn push_thin_for(&self, task_id: &str, body: impl Into<String>) {
        self.state
            .thin_by_task
            .lock()
            .unwrap()
            .entry(task_id.to_owned())
            .or_default()
            .push_back(body.into());
    }

    /// 按标题排 ThinAgent 回复；受理前就能排，第一个请求不会落空。
    pub(super) fn push_thin_for_title(&self, title: &str, body: impl Into<String>) {
        self.state
            .thin_by_title
            .lock()
            .unwrap()
            .entry(title.to_owned())
            .or_default()
            .push_back(body.into());
    }

    pub(super) fn take_thin_script(&self, body: &str) -> Option<String> {
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

    pub(super) fn bodies(&self, thin: bool) -> Vec<String> {
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
        if thin
            && let Some((status, retry_after)) = self.state.thin_errors.lock().unwrap().pop_front()
        {
            let mut template = ResponseTemplate::new(status).set_body_json(serde_json::json!({
                "error": {"type": "rate_limit_error", "message": "用量上限"}
            }));
            if let Some(retry_after) = retry_after {
                template = template.insert_header("retry-after", retry_after);
            }
            return template;
        }
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
        // 任务图的动态 Kind 轮：图 id 在 task_graph_propose 的工具结果里，静态回放拿不到。
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
                        "quote": "先查",
                        "requirement": "先查 notes.md 有几行，再把行数写进 other.txt",
                        "dod": "other.txt 里写着行数",
                        "subtasks": [
                            {"title": "先查行数", "requirement": "查 notes.md 有几行", "dod": "给出行数"},
                            {"title": "再写结果", "requirement": "把行数写进 other.txt", "dod": "other.txt 里是行数", "after": [0]}
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
        // 里程碑门的动态 Kind 轮。
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
                        "quote": "先查",
                        "requirement": "先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果",
                        "dod": "核查通过了再写结果",
                        "subtasks": [
                            {"title": "先查行数", "requirement": "查 notes.md 有几行", "dod": "给出行数"},
                            {"title": "独立核查", "requirement": "独立核查行数结果", "dod": "给出核查发现", "milestone": true, "after": [0]},
                            {"title": "再写结果", "requirement": "把行数写进 other.txt", "dod": "other.txt 里是行数", "after": [0]}
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
        // 只提图不上卡（NoCard）。
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
                        "quote": "先查",
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
        // 两轮各提一张图一张卡（第一张被第二张替换取消）。
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
                        "quote": "先查",
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
                        "quote": "先查",
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

pub(super) async fn mount(server: &MockServer, router: &Router) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(router.clone())
        .mount(server)
        .await;
}

pub(super) async fn open_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
    open_host_with(server, data, cwd, |_| {}).await
}

pub(super) async fn open_host_with(
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

pub(super) fn send(_host: &RootHost, request_id: &str, text: &str) -> SendRequest {
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

/// 卡片测试的宿主：动作登记表走产品装配（set_paused、resume_task 属于 aios-tasks）。
pub(super) async fn open_actions_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
    open_host_with(server, data, cwd, |options| {
        options.actions = Arc::new(service::ActionRegistry::settings(
            data.to_path_buf(),
            options.identity.clone(),
        ));
    })
    .await
}

/// 起一个假模型服务和两个临时目录：重启测试的起点。
pub(super) async fn restart_setup() -> (MockServer, Router, tempfile::TempDir, tempfile::TempDir) {
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
pub(super) async fn reopen_actions_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
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
pub(super) async fn crash_while_running(
    router: &Router,
    data: &Path,
    host: RootHost,
    tasks: &[Task],
) {
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
