//! 屏蔽的模型：整表替换、出站拦截、失败原因、解除（service 公开边界）。

use std::path::Path;
use std::time::Duration;

use agent::AgentConfig;
use credentials::CheckState;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, CompactRequest, ConnectionSaveRequest, ConnectionVerifyRequest, DeliveryMode,
    DisabledModelsCheckRequest, DisabledModelsSetRequest, FollowRequest, ModelSettingsRequest,
    Response, RootHost, RootSessionOptions, SendRequest, StatusRequest,
};
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
    let expected: Vec<String> = models.iter().map(|m| m.to_string()).collect();
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
    assert!(!response.disabled_unknown, "本票的读法还不会报「不知道」");
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
