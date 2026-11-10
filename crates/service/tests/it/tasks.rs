//! T024.3：Kind 派发、ThinAgent 执行、上报 relay 的后端闭环（假模型、两个真会话）。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::AgentConfig;
use model::DEFAULT_CONTEXT_WINDOW;
use serde_json::Value;
use service::{
    Call, DeliveryMode, DisabledModelsSetRequest, RootHost, SendRequest, SessionModelSelectRequest,
};
use tasks::{ReportState, Stage, Store, Task};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// 假模型按请求体路由：工具表里有 task_accept 的是 Kind，有 task_report 的是 ThinAgent。
#[derive(Clone)]
struct Router {
    state: Arc<RouterState>,
}

struct RouterState {
    kind: Mutex<VecDeque<String>>,
    thin: Mutex<VecDeque<String>>,
    bodies: Mutex<Vec<(bool, String)>>,
    unexpected: Mutex<Vec<(bool, String)>>,
    /// ThinAgent 的响应延迟：live 测试要在任务还在跑时接上事件流。
    thin_delay: Mutex<Option<Duration>>,
}

impl Router {
    fn new() -> Self {
        Self {
            state: Arc::new(RouterState {
                kind: Mutex::new(VecDeque::new()),
                thin: Mutex::new(VecDeque::new()),
                bodies: Mutex::new(Vec::new()),
                unexpected: Mutex::new(Vec::new()),
                thin_delay: Mutex::new(None),
            }),
        }
    }

    fn set_thin_delay(&self, delay: Duration) {
        *self.state.thin_delay.lock().unwrap() = Some(delay);
    }

    fn push_kind(&self, body: impl Into<String>) {
        self.state.kind.lock().unwrap().push_back(body.into());
    }

    fn push_thin(&self, body: impl Into<String>) {
        self.state.thin.lock().unwrap().push_back(body.into());
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
        let template = ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse);
        match (thin, *self.state.thin_delay.lock().unwrap()) {
            (true, Some(delay)) => template.set_delay(delay),
            _ => template,
        }
    }
}

async fn mount(server: &MockServer, router: &Router) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(router.clone())
        .mount(server)
        .await;
}

async fn open_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
    let mut options = crate::common::options(data);
    options.cwd = cwd.to_path_buf();
    options.model.base_url = server.uri();
    options.config = AgentConfig {
        instruction_roots: vec![data.join("agents-home")],
        skill_roots: vec![data.join("skills-home")],
        ..AgentConfig::default()
    };
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
    for _ in 0..300 {
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
    host.task_runtime().expect("任务执行者").set_paused(true);

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
    host.task_runtime().expect("任务执行者").set_paused(false);
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
    // 第一份请求只开出无条件的 6 件；加载 web skill（Exa key 已存）后开出 8 件。
    let loaded = thin_bodies
        .iter()
        .find(|body| body.contains("\"name\":\"web_search\""))
        .expect("加载 web 后的请求");
    let mut names = tool_names(loaded);
    names.sort();
    let mut expected: Vec<String> = service::THIN_TOOLS.map(str::to_owned).to_vec();
    expected.sort();
    assert_eq!(names, expected, "ThinAgent 的工具表正好 8 件");
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
    host.task_runtime().expect("任务执行者").set_paused(true);

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
    host.task_runtime().expect("任务执行者").set_paused(false);

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

    fn data(&self) -> &Path {
        self.data.path()
    }

    /// 让 Kind 受理一个任务，返回任务；pause 时 launch 先不跑。
    async fn accept(&self, pause: bool) -> Task {
        if pause {
            self.host.task_runtime().expect("runtime").set_paused(true);
        }
        let args = serde_json::json!({
            "title": "查一下",
            "requirement": "找答案",
            "dod": "有答案"
        })
        .to_string();
        self.router.push_kind(crate::common::tool_use_sse(
            0,
            "call-accept",
            "task_accept",
            &args,
        ));
        self.router.push_kind(crate::common::text_sse("已派出"));
        self.host
            .dispatch(Call::Send(send(&self.host, "kind-1", "查一下")))
            .await
            .expect("发送");
        crate::common::wait_turns(&self.host, self.data(), 1).await;
        let store = tasks_store(self.data());
        one_task(&store)
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
        .set_paused(false);
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
async fn kind_refuses_the_four_waits_and_runs_a_short_command() {
    let fixture = Fixture::new().await;
    let calls = [
        serde_json::json!({"description": "wait", "command": "sleep 5", "timeoutMs": 20000})
            .to_string(),
        serde_json::json!({"description": "echo", "command": "echo sleep 5", "timeoutMs": 20000})
            .to_string(),
        serde_json::json!({"description": "long", "command": "echo hi", "timeoutMs": 30001})
            .to_string(),
        serde_json::json!({"description": "default", "command": "echo hi"}).to_string(),
        serde_json::json!({"job_id": "job-1", "wait": true}).to_string(),
    ];
    for (index, args) in calls.iter().enumerate() {
        fixture.router.push_kind(crate::common::tool_use_sse(
            index,
            &format!("call-{index}"),
            if index == 4 { "job_output" } else { "bash" },
            args,
        ));
    }
    fixture.router.push_kind(crate::common::text_sse("跑完了"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-1", "跑几个命令")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;

    let events = crate::common::read_root_events(fixture.data());
    let mut order = Vec::new();
    let mut results = std::collections::HashMap::new();
    for (index, event) in events.iter().enumerate() {
        match &event.kind {
            session::EventKind::ToolCall { call_id, .. } => {
                order.push(("call", call_id.clone(), index));
            }
            session::EventKind::ToolResult { message, .. } => {
                order.push((
                    "result",
                    message.tool_call_id.clone().unwrap_or_default(),
                    index,
                ));
                let text: String = message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect();
                results.insert(
                    message.tool_call_id.clone().unwrap(),
                    (text, message.is_error),
                );
            }
            _ => {}
        }
    }
    for call_id in ["call-0", "call-1", "call-2", "call-3", "call-4"] {
        let call = order
            .iter()
            .find(|(kind, id, _)| *kind == "call" && id == call_id)
            .expect("有 tool/call");
        let result = order
            .iter()
            .find(|(kind, id, _)| *kind == "result" && id == call_id)
            .expect("有 tool/result");
        assert!(call.2 < result.2, "{call_id} 先记调用再记结果");
    }
    for call_id in ["call-0", "call-2", "call-4"] {
        let (text, is_error) = &results[call_id];
        assert!(is_error.unwrap_or(false), "{call_id}");
        assert_eq!(
            text.strip_prefix("Error: ").unwrap_or(text),
            agent::NO_WAIT,
            "{call_id} 的拒绝语逐字一致"
        );
    }
    let (text, is_error) = &results["call-3"];
    assert!(is_error.unwrap_or(false));
    assert_eq!(
        text.strip_prefix("Error: ").unwrap_or(text),
        agent::DEFAULT_WAIT,
        "没给超时也没转后台的提示"
    );
    let (text, is_error) = &results["call-1"];
    assert!(!is_error.unwrap_or(false), "echo sleep 不误判：{text}");
    assert!(text.contains("sleep 5"), "{text}");
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
