//! 屏蔽的模型：整表替换、出站拦截、失败原因、解除（service 公开边界）。

use std::path::Path;
use std::time::Duration;

use agent::AgentConfig;
use credentials::CheckState;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, CancelRequest, CompactRequest, ConnectionSaveRequest, ConnectionVerifyRequest,
    DeliveryMode, DisabledModelUser, DisabledModelsCheckRequest, DisabledModelsSetRequest,
    FollowRequest, ModelSettingsRequest, Response, RootHost, RootSessionOptions, SendRequest,
    SessionModelSelectRequest, StatusRequest,
};

use crate::scripted_sse::{ScriptedSseServer, SseScript};
use tempfile::tempdir;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 一轮成功的 SSE（会和会话日志、统计一起被投影）。
const TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_mask","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":9,"output_tokens":2}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

/// 产品的凭证来源会先查屏蔽表；测试来源照做（数据目录 + 连接 id）。
struct TestCredential {
    data_dir: std::path::PathBuf,
    connection: String,
}

impl model::CredentialSource for TestCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok("test-key".to_owned())
    }

    fn admit(&self, model: &str) -> Result<(), model::ModelError> {
        service::disabled::admit(&self.data_dir, &self.connection, model)
    }
}

fn options(data_dir: &Path, endpoint: &str) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: data_dir.to_path_buf(),
        model: DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: endpoint.to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::new("high"),
        },
        config: AgentConfig::default(),
        credentials: Some(std::sync::Arc::new(TestCredential {
            data_dir: data_dir.to_path_buf(),
            connection: "deepseek".to_owned(),
        })),
        identity: Some(data_dir.join("identity.txt")),
        model_factory: None,
        append_hook_factory: None,
        keyring_changed: None,
    }
}

async fn host(dir: &Path, endpoint: &str) -> RootHost {
    RootHost::open(options(dir, endpoint), DEFAULT_CONTEXT_WINDOW).await
}

async fn mock_turn(server: &MockServer) {
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TEXT_SSE),
        )
        .mount(server)
        .await;
}

async fn request_count(server: &MockServer) -> usize {
    server.received_requests().await.expect("请求记录").len()
}

async fn wait_host_idle(host: &RootHost) {
    for _ in 0..600 {
        let response = host.dispatch(Call::Status(StatusRequest {})).await;
        let Ok(Response::Status(status)) = response else {
            panic!("status 应可用：{response:?}");
        };
        if !status.running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等待根会话空闲超时");
}

async fn wait_host_running(host: &RootHost) {
    for _ in 0..600 {
        let response = host.dispatch(Call::Status(StatusRequest {})).await;
        let Ok(Response::Status(status)) = response else {
            panic!("status 应可用：{response:?}");
        };
        if status.running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等待根会话开跑超时");
}

fn root_session_id(dir: &Path) -> session::SessionId {
    let text = std::fs::read_to_string(dir.join(service::ROOT_STATE_FILE)).expect("root state");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    session::SessionId::parse(value["sessionId"].as_str().expect("sessionId")).expect("id")
}

async fn send(host: &RootHost, request_id: &str, text: &str) {
    host.dispatch(Call::Send(SendRequest {
        request_id: request_id.to_owned(),
        content: text.to_owned(),
        delivery: DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("受理发送");
}

async fn set_disabled(host: &RootHost, models: Vec<&str>) {
    let expected: Vec<String> = models
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    let response = host
        .dispatch(Call::DisabledModelsSet(DisabledModelsSetRequest {
            models: expected.clone(),
        }))
        .await
        .expect("disabled-models-set");
    let Response::DisabledModelsSet(response) = response else {
        panic!("expected DisabledModelsSet");
    };
    assert_eq!(response.models, expected, "响应是规范化后的整表");
}

async fn select_model(host: &RootHost, model: &str) {
    let response = host
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            connection: None,
            model: Some(model.to_owned()),
            effort_wanted: None,
        }))
        .await
        .expect("session-model-select");
    let Response::SessionModelSelect(selected) = response else {
        panic!("expected SessionModelSelect");
    };
    assert_eq!(selected.model, model, "选择写成了目标模型");
}

async fn check_rule(host: &RootHost, pattern: &str) -> service::DisabledModelsCheckResponse {
    let response = host
        .dispatch(Call::DisabledModelsCheck(DisabledModelsCheckRequest {
            pattern: pattern.to_owned(),
        }))
        .await
        .expect("disabled-models-check");
    let Response::DisabledModelsCheck(response) = response else {
        panic!("expected DisabledModelsCheck");
    };
    response
}

#[tokio::test]
async fn 屏蔽当前模型后发一句被拒且不出站() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    set_disabled(&host, vec!["deepseek/deepseek-flash"]).await;

    send(&host, "r1", "你好").await;
    wait_host_idle(&host).await;

    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    let error = snapshot
        .events()
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            session::EventKind::TurnEnd {
                reason: session::TurnEndReason::Error { error },
                ..
            } => Some(error.clone()),
            _ => None,
        })
        .expect("这一轮应以错误收尾");
    assert_eq!(error.code, "MODEL_DISABLED", "被屏蔽的错误码：{error:?}");
    assert!(
        error.message.contains("deepseek-flash"),
        "说明里有模型名：{}",
        error.message
    );
    assert!(
        error.message.contains("规则「deepseek/deepseek-flash」"),
        "说明里有命中的规则：{}",
        error.message
    );
    assert!(
        !snapshot
            .events()
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::LlmRetry { .. })),
        "被屏蔽不重试"
    );
    assert_eq!(request_count(&server).await, 0, "被屏蔽的请求不出站");

    let follow = host
        .follow(FollowRequest { last_seq: 0 })
        .await
        .expect("follow");
    let turn_end = follow
        .events
        .iter()
        .find(|event| event.event_type == "turn/end")
        .expect("turn/end 事件");
    assert_eq!(
        turn_end.cause,
        Some(service::FailureCause::Masked),
        "失败块按 masked 分类"
    );
}

#[tokio::test]
async fn 解除屏蔽后下一轮正常发出() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;

    set_disabled(&host, vec!["deepseek/deepseek-flash"]).await;
    send(&host, "r1", "被挡住的一句").await;
    wait_host_idle(&host).await;
    assert_eq!(request_count(&server).await, 0, "屏蔽期间不出站");

    set_disabled(&host, vec![]).await;
    send(&host, "r2", "解除了再发").await;
    wait_host_idle(&host).await;
    assert_eq!(request_count(&server).await, 1, "解除后照常发一次");

    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    let completed = snapshot.events().iter().rev().any(|event| {
        matches!(
            &event.kind,
            session::EventKind::TurnEnd {
                reason: session::TurnEndReason::Completed,
                ..
            }
        )
    });
    assert!(completed, "解除后这一轮正常完成");
}

#[tokio::test]
async fn 压缩摘要遇到屏蔽不发出请求() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    // 先聊两轮：给压缩留出可选的历史（否则没有可压的前缀，测试就测不到摘要请求）。
    send(&host, "r1", "第一句").await;
    wait_host_idle(&host).await;
    send(&host, "r2", "第二句").await;
    wait_host_idle(&host).await;
    assert_eq!(request_count(&server).await, 2, "两轮对话各发一次");

    set_disabled(&host, vec!["deepseek/deepseek-flash"]).await;
    let compact = host
        .dispatch(Call::Compact(CompactRequest::default()))
        .await;
    assert!(
        compact.is_err(),
        "屏蔽时压缩不该成功（摘要请求会被拦）：{compact:?}"
    );
    assert_eq!(
        request_count(&server).await,
        2,
        "摘要请求被拦，假端点没有收到新请求"
    );
    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    assert!(
        !snapshot
            .events()
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::CompactionSummary { .. })),
        "没有写出摘要"
    );
}

#[tokio::test]
async fn 验证连接遇到屏蔽不发出请求() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "deepseek".to_owned(),
        name: "DeepSeek".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: server.uri(),
        api_key: "sk-test-not-a-real-key".to_owned(),
        preset: Some("deepseek".to_owned()),
        ..ConnectionSaveRequest::default()
    }))
    .await
    .expect("save");

    set_disabled(&host, vec!["deepseek/*"]).await;
    let response = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "deepseek".to_owned(),
        }))
        .await
        .expect("验证调用本身应返回（结果写进注册器）");
    let Response::ConnectionVerify(response) = response else {
        panic!("expected ConnectionVerify");
    };
    match response.connection.check {
        CheckState::Fail { reason, .. } => assert_eq!(reason, "MODEL_DISABLED", "验证结果是被屏蔽"),
        other => panic!("应记成失败：{other:?}"),
    }
    assert_eq!(request_count(&server).await, 0, "验证的请求也不出站");
}

#[tokio::test]
async fn 屏蔽表整表替换规范化保存并发设置变化事件() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    let mut events = host.subscribe_events();

    let response = host
        .dispatch(Call::DisabledModelsSet(DisabledModelsSetRequest {
            models: vec![
                " *FABLE* ".to_owned(),
                String::new(),
                "m9".to_owned(),
                "*fable*".to_owned(),
                "   ".to_owned(),
            ],
        }))
        .await
        .expect("disabled-models-set");
    let Response::DisabledModelsSet(response) = response else {
        panic!("expected DisabledModelsSet");
    };
    assert_eq!(
        response.models,
        vec!["*FABLE*".to_owned(), "m9".to_owned()],
        "去空白/空规则/不分大小写去重，保持顺序"
    );
    assert_eq!(
        service::settings::load(dir.path()).disabled_models,
        vec!["*FABLE*".to_owned(), "m9".to_owned()],
        "settings.json 里是规范化后的整表"
    );
    let event = events.try_recv().expect("存好后发设置变化事件");
    assert_eq!(event.event_type, "keyring/changed", "设置变化事件");
}

#[tokio::test]
async fn 模型设置响应旧字段照旧新字段只增() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    set_disabled(&host, vec!["*preview*"]).await;

    let response = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("model-settings");
    let Response::ModelSettings(response) = response else {
        panic!("expected ModelSettings");
    };
    assert_eq!(response.disabled_models, vec!["*preview*".to_owned()]);
    assert!(!response.disabled_unknown, "这一版还不会报「不知道」");
    assert!(response.disabled_hits.is_empty(), "匹配数留给后续实现");
    assert!(response.listed.is_empty(), "存下的厂商列表留给后续实现");

    let json = serde_json::to_value(&response).expect("json");
    assert!(
        json["settings"]["connections"].is_array(),
        "旧字段 settings 照旧：{json}"
    );
    assert_eq!(json["disabledModels"][0], "*preview*");
    assert_eq!(json["disabledUnknown"], false);
    assert!(json["disabledHits"].as_array().is_some());
    assert!(json["listed"].as_object().is_some());
    assert_eq!(host.protocol_version(), 8, "ready 帧的协议版本是 8");
}

#[tokio::test]
async fn 查询规则返回形状() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    let response = host
        .dispatch(Call::DisabledModelsCheck(DisabledModelsCheckRequest {
            pattern: "p".to_owned(),
        }))
        .await
        .expect("disabled-models-check 的形状");
    let Response::DisabledModelsCheck(response) = response else {
        panic!("expected DisabledModelsCheck");
    };
    assert!(response.hits.is_empty());
    assert!(response.users.is_empty());
    let json =
        serde_json::to_value(service::DisabledModelUser::KindChat { running: true }).expect("json");
    assert_eq!(json["kind"], "kind-chat");
    assert_eq!(json["running"], true);
    let new_sessions = serde_json::to_value(service::DisabledModelUser::NewSessions).expect("json");
    assert_eq!(new_sessions["kind"], "new-sessions");
}

/// 先回一次工具调用（read hello.txt），下一轮再回文字。
fn tool_read_sse() -> &'static str {
    r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_tool","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_read","name":"read","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"hello.txt\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#
}

/// 一段写到一半的 SSE（打开着 text 块，没有 content_block_stop/message_stop）。
fn partial_text_sse() -> &'static str {
    r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_partial","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"已经流出来的"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"部分文字。"}}

"#
}

/// 正在跑的一轮命中屏蔽：以 hook 原因停下；已做的工具结果和已流出的前缀都在，
/// 排队的消息还在（下一轮被拦成 MODEL_DISABLED）；重开会话还能接着用。
#[tokio::test]
async fn 屏蔽正在跑的一轮时以钩子停下并保留已做的和排队的() {
    let mut server = ScriptedSseServer::start(vec![
        SseScript {
            frames: tool_read_sse().to_owned(),
            hold: false,
        },
        SseScript {
            frames: partial_text_sse().to_owned(),
            hold: true,
        },
        SseScript {
            frames: TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "hello\n").unwrap();
    let root = host(dir.path(), &server.base_url()).await;
    let mut agent_events = root.subscribe_agent_events();

    send(&root, "r1", "读一下再回答").await;
    server.wait_ready().await;
    let mut streamed = String::new();
    while streamed != "已经流出来的部分文字。" {
        match agent_events.recv().await {
            Ok(agent::AgentEvent::AssistantTextDelta { text }) => streamed.push_str(&text),
            Ok(_) => {}
            Err(error) => panic!("agent 事件通道关闭：{error}"),
        }
    }
    // 排一句话：钩子取消不清理输入队列，它要在下一轮被领取。
    send(&root, "r2", "排队的一句").await;
    set_disabled(&root, vec!["deepseek/deepseek-flash"]).await;
    // 被打断的一轮要自己停下；不等连接关闭（否则分不清「停下」和「流断了」）。
    let stopped = tokio::time::timeout(Duration::from_secs(2), wait_host_idle(&root)).await;
    assert!(stopped.is_ok(), "屏蔽正在跑的模型后这一轮要停下");
    server.release();

    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    let events = snapshot.events();
    let reasons: Vec<session::TurnEndReason> = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reasons.len(), 1, "只打断了这一轮：{reasons:?}");
    match &reasons[0] {
        session::TurnEndReason::Aborted {
            reason: session::AbortCause::Hook { reason },
        } => assert_eq!(reason, "model-disabled", "打断原因是稳定的钩子名"),
        other => panic!("第一轮应以 hook 原因收尾：{other:?}"),
    }
    let interrupted: String = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::AssistantMessage {
                message,
                interrupted: Some(true),
                ..
            } => Some(
                message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .expect("打断的那一轮留下已流出的文本");
    assert_eq!(interrupted, "已经流出来的部分文字。", "已出来的字留着");
    let calls = events
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::ToolCall { .. }))
        .count();
    let results = events
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::ToolResult { .. }))
        .count();
    assert_eq!(calls, 1, "这一轮确实用了一次工具");
    assert_eq!(calls, results, "打断之后没有悬空的 tool/call");

    let follow = root
        .dispatch(Call::Follow(FollowRequest { last_seq: 0 }))
        .await
        .expect("follow");
    let Response::Follow(follow) = follow else {
        panic!("expected Follow");
    };
    let first_end = follow
        .events
        .iter()
        .find(|event| event.event_type == "turn/end")
        .expect("turn/end 事件");
    assert_eq!(
        first_end.cause,
        Some(service::FailureCause::Masked),
        "被打断的一轮也归 masked，页面出同一张失败块"
    );

    // 解除屏蔽、重开会话：排队的消息还在，重开时被领取并跑完。
    set_disabled(&root, vec![]).await;
    drop(root);
    let reopened = host(dir.path(), &server.base_url()).await;
    let mut queued_completed = false;
    for _ in 0..600 {
        let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
        let mut after_queue = false;
        for event in snapshot.events() {
            match &event.kind {
                session::EventKind::UserMessage { message, .. }
                    if message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>()
                        .contains("排队的一句") =>
                {
                    after_queue = true;
                }
                session::EventKind::TurnEnd {
                    reason: session::TurnEndReason::Completed,
                    ..
                } if after_queue => {
                    queued_completed = true;
                }
                _ => {}
            }
        }
        if queued_completed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(queued_completed, "重开会话领取排队的消息并跑完");
    wait_host_idle(&reopened).await;
}

/// 屏蔽一个不在用的模型：在跑的那一轮照常跑完，请求不受影响。
#[tokio::test]
async fn 屏蔽一个不在用的模型时在跑的一轮照常跑完() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TEXT_SSE)
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let root = host(dir.path(), &server.uri()).await;

    send(&root, "r1", "跑一轮").await;
    wait_host_running(&root).await;
    set_disabled(&root, vec!["deepseek/deepseek-v4-flash"]).await;
    wait_host_idle(&root).await;

    assert_eq!(
        request_count(&server).await,
        1,
        "屏蔽别的模型不影响这一轮出站"
    );
    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    let last = snapshot
        .events()
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("这一轮有收尾");
    assert_eq!(last, session::TurnEndReason::Completed, "这一轮照常跑完");
}

/// 运行中先换到 B、再屏蔽 B：跑着的 A 那一轮不动；屏蔽 A 才停。
/// 判断只用本轮冻结的连接 × 模型，不看会变的当前选择。
#[tokio::test]
async fn 打断判断用本轮冻结的连接和模型不用会变的当前选择() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TEXT_SSE)
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let root = host(dir.path(), &server.uri()).await;

    send(&root, "r1", "用 A 跑一轮").await;
    wait_host_running(&root).await;
    let selected = root
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            connection: None,
            model: Some("deepseek-v4-flash".to_owned()),
            effort_wanted: None,
        }))
        .await
        .expect("换到 B");
    let Response::SessionModelSelect(selected) = selected else {
        panic!("expected SessionModelSelect");
    };
    assert_eq!(selected.model, "deepseek-v4-flash", "选择已经换成 B");

    set_disabled(&root, vec!["deepseek/deepseek-v4-flash"]).await;
    let status = root
        .dispatch(Call::Status(StatusRequest {}))
        .await
        .expect("status");
    let Response::Status(status) = status else {
        panic!("expected Status");
    };
    assert!(status.running, "跑着的是 A：屏蔽 B 不能停 A");

    set_disabled(&root, vec!["deepseek/deepseek-flash"]).await;
    wait_host_idle(&root).await;
    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    let last = snapshot
        .events()
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("这一轮有收尾");
    match last {
        session::TurnEndReason::Aborted {
            reason: session::AbortCause::Hook { reason },
        } => assert_eq!(reason, "model-disabled"),
        other => panic!("屏蔽 A 之后这一轮应以 hook 停下：{other:?}"),
    }
    let requests = server.received_requests().await.expect("请求记录");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("body json");
    assert_eq!(body["model"], "deepseek-flash", "跑着的一轮一直用冻结的 A");
}

/// 查询一条规则：命中表和谁在用都由服务算；新会话只在 lastSelection 命中时出现，
/// running 和实际状态一致。
#[tokio::test]
async fn 查询规则返回命中的模型和使用者() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TEXT_SSE)
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let root = host(dir.path(), &server.uri()).await;
    // 命中表列的是已保存连接的可选模型；先存一个连接。
    root.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "deepseek".to_owned(),
        name: "DeepSeek".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: server.uri(),
        api_key: "sk-test-not-a-real-key".to_owned(),
        preset: Some("deepseek".to_owned()),
        ..ConnectionSaveRequest::default()
    }))
    .await
    .expect("save connection");
    select_model(&root, "deepseek-flash").await;

    let a = check_rule(&root, "deepseek/deepseek-flash").await;
    assert!(
        a.hits.contains(&"deepseek/deepseek-flash".to_owned()),
        "命中表按「连接/模型」给：{:?}",
        a.hits
    );
    assert_eq!(
        a.users,
        vec![
            DisabledModelUser::KindChat { running: false },
            DisabledModelUser::NewSessions,
        ],
        "Kind 对话接着用 A，新会话会用上一次选的 A"
    );

    // 换到 B：A 不是当前选择、也不是上一次选的，就没有使用者。
    select_model(&root, "deepseek-v4-flash").await;
    let old = check_rule(&root, "deepseek/deepseek-flash").await;
    assert!(old.users.is_empty(), "A 已经不在用：{:?}", old.users);
    let b = check_rule(&root, "deepseek/deepseek-v4-flash").await;
    assert_eq!(
        b.users,
        vec![
            DisabledModelUser::KindChat { running: false },
            DisabledModelUser::NewSessions,
        ],
        "B 是当前选择和上一次选的"
    );

    // 开跑 B：Kind 对话的 running 跟着实际状态。
    send(&root, "r1", "跑 B").await;
    wait_host_running(&root).await;
    let running = check_rule(&root, "deepseek/deepseek-v4-flash").await;
    assert_eq!(
        running.users,
        vec![
            DisabledModelUser::KindChat { running: true },
            DisabledModelUser::NewSessions,
        ],
        "在跑 B：running 为真，新会话照旧列"
    );
    let not_used = check_rule(&root, "deepseek/deepseek-flash").await;
    assert!(not_used.users.is_empty(), "跑的是 B，A 没有使用者");
    root.dispatch(Call::Cancel(CancelRequest {}))
        .await
        .expect("cancel");
    wait_host_idle(&root).await;
    let after = check_rule(&root, "deepseek/deepseek-v4-flash").await;
    assert_eq!(
        after.users,
        vec![
            DisabledModelUser::KindChat { running: false },
            DisabledModelUser::NewSessions,
        ],
        "没在跑时 running 为假"
    );
}
