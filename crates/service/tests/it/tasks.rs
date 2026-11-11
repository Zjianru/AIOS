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
    Response, RootHost, SendAttachment, SendRequest, SessionModelSelectRequest,
    SettingsUpdateRequest, StopMode, TaskInterruptRequest, TaskReadRequest, TaskSayRequest,
    TaskStopRequest,
};
use tasks::{AttemptEnd, EffectKind, EffectState, ReportState, Stage, Store, Task};
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
    wait_until("ThinAgent 的下一步请求", || {
        fixture.router.bodies(true).len() >= 2
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
    assert_eq!(service::THIN_TOOLS.len(), 9, "ThinAgent 的工具表 9 件");
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
    tools.retain(|name| name != "task_handback");
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
    crash_while_running(&router, data.path(), host, &[task.clone()]).await;

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
    crash_while_running(&router, data.path(), host, &[task.clone()]).await;

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

    // 用户在任务对话里先说一句：T028 的路开新 Attempt。
    router.push_thin_for(crashed[0].id.as_str(), crate::common::text_sse("收到"));
    router.push_kind(crate::common::text_sse("收到"));
    say_in_task_dialog(&host, &crashed[0], "继续").await;
    wait_until("任务对话先接上了", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;

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
    crash_while_running(&router, data.path(), host, &[task.clone()]).await;

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
    crash_while_running(&router, data.path(), host, &[task.clone()]).await;

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
