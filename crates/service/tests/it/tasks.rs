//! T024.3：Kind 派发、ThinAgent 执行、上报 relay 的后端闭环（假模型、两个真会话）。

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::AgentConfig;
use model::DEFAULT_CONTEXT_WINDOW;
use serde_json::Value;
use service::{
    Call, CardActRequest, CardVerb, DeliveryMode, DisabledModelsSetRequest, Response, RootHost,
    SendRequest, SessionModelSelectRequest, StopMode, TaskStopRequest,
};
use tasks::{AttemptEnd, EffectKind, ReportState, Stage, Store, Task};
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
    /// 按任务的 ThinAgent 脚本：多任务同时跑时，各自按 assignment 里的 task_id 取。
    thin_by_task: Mutex<HashMap<String, VecDeque<String>>>,
    /// 任务 id 要受理后才知道；测试先按标题排脚本（assignment 里也有标题）。
    thin_by_title: Mutex<HashMap<String, VecDeque<String>>>,

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
                thin_by_task: Mutex::new(HashMap::new()),
                thin_by_title: Mutex::new(HashMap::new()),
                bodies: Mutex::new(Vec::new()),
                unexpected: Mutex::new(Vec::new()),
                thin_delay: Mutex::new(None),
            }),
        }
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
        if pause {
            self.host
                .task_runtime()
                .expect("runtime")
                .set_pump_paused(true);
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
        for _ in 0..spares {
            self.router.push_kind(crate::common::text_sse("收到"));
        }
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

/// 暂停测试的宿主：动作登记表走产品装配（set_paused 属于 aios-tasks）。
async fn open_pause_host(server: &MockServer, data: &Path, cwd: &Path) -> RootHost {
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
    let host = open_pause_host(&server, data.path(), cwd.path()).await;
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
    let host = open_pause_host(&server, data.path(), cwd.path()).await;

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
    let host = open_pause_host(&server, data.path(), cwd.path()).await;
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
