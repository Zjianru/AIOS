//! 屏蔽的模型：整表替换、出站拦截、失败原因、解除（service 公开边界）。

use std::path::Path;
use std::time::Duration;

use credentials::CheckState;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, CancelRequest, CompactRequest, ConnectionSaveRequest, ConnectionVerifyRequest,
    DeliveryMode, DisabledModelUser, DisabledModelsCheckRequest, DisabledModelsSetRequest,
    FollowRequest, ModelSettingsRequest, Response, RootHost, RootSessionOptions, SendRequest,
    ServiceError, SessionModelSelectRequest, SettingsUpdateRequest, StatusRequest,
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

async fn host(dir: &Path, endpoint: &str) -> RootHost {
    let mut options = crate::common::options(dir);
    options.model.base_url = endpoint.to_owned();
    crate::common::open(options).await
}

/// 跨连接测试用：照 cli 的 model_factory 从 Keyring 按连接 id 解析配置和凭证。
fn keyring_factory(data_dir: &Path, identity: &Path) -> service::ModelFactory {
    let data = data_dir.to_path_buf();
    let identity = identity.to_path_buf();
    std::sync::Arc::new(move |selection: Option<&session::ModelSelection>| {
        let keyring = credentials::Keyring::open(data.clone(), Some(&identity))
            .map_err(|error| error.to_string())?;
        let view = keyring.view();
        if let Some(problem) = view.key_file.problem.clone() {
            return Err(format!("Keyring 读不出来：{problem}"));
        }
        let wanted = selection
            .map(|selection| selection.connection.clone())
            .or_else(|| {
                service::settings::load(&data)
                    .last_selection
                    .map(|selection| selection.connection)
            })
            .unwrap_or_else(|| "deepseek".to_owned());
        let connection = view
            .connections
            .iter()
            .find(|connection| connection.id == wanted)
            .or_else(|| {
                view.connections
                    .iter()
                    .find(|connection| connection.id == "deepseek")
            })
            .or_else(|| view.connections.first())
            .ok_or_else(|| "还没有接模型".to_owned())?;
        // 同连接才沿用选择里的模型/档；跨连接整组取连接默认（和 cli 的 resolve_selection 一致）。
        let same = selection.is_some_and(|selection| selection.connection == connection.id);
        let model = selection
            .filter(|_| same)
            .map(|selection| selection.model.clone())
            .unwrap_or_else(|| connection.primary_model().to_owned());
        let effort = selection
            .filter(|_| same)
            .map(|selection| selection.effort_wanted.clone())
            .unwrap_or_else(|| "high".to_owned());
        Ok((
            connection.id.clone(),
            DeepSeekConfig {
                provider: model::catalog::provider_id_for_connection(
                    &connection.preset,
                    &connection.endpoint,
                ),
                base_url: connection.endpoint.clone(),
                model,
                max_tokens: 2048,
                context_window: DEFAULT_CONTEXT_WINDOW,
                effort: Effort::new(effort),
                tool_update: None,
            },
            Some(std::sync::Arc::new(ConnectionCredential {
                data_dir: data.clone(),
                connection: connection.id.clone(),
            })
                as std::sync::Arc<dyn model::CredentialSource>),
        ))
    })
}

/// 按连接签发：屏蔽检查带这个连接的 id（和产品 Keyring 来源一致）。
struct ConnectionCredential {
    data_dir: std::path::PathBuf,
    connection: String,
}

impl model::CredentialSource for ConnectionCredential {
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
    assert_eq!(request_count(&server).await, 0, "设置变更不出站");
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
    assert!(
        !response.disabled_unknown,
        "读到过就是 false；坏文件沿用上次时才不是「不知道」"
    );
    assert_eq!(
        response.disabled_hits,
        vec![Vec::<String>::new()],
        "一条都没命中时给空表，和规则等长同序"
    );
    assert!(response.listed.is_empty(), "存下的厂商列表留给后续实现");

    let json = serde_json::to_value(&response).expect("json");
    assert!(
        json["settings"]["connections"].is_array(),
        "旧字段 settings 照旧：{json}"
    );
    assert_eq!(json["disabledModels"][0], "*preview*");
    assert_eq!(json["disabledUnknown"], false);
    assert_eq!(
        json["disabledHits"],
        serde_json::json!([[]]),
        "线上是空表数组，页面只读这个字段"
    );
    assert!(json["listed"].as_object().is_some());
    assert_eq!(host.protocol_version(), 9, "ready 帧的协议版本是 9");
    assert_eq!(request_count(&server).await, 0, "读设置不出站");
}

/// 每条规则下面写「现在匹配 n 个」：`disabledHits` 与 `disabledModels` 等长同序，
/// 给的是现在真正能选的「连接/模型」；一条都不命中的规则给空表。
#[tokio::test]
async fn 屏蔽表的每条规则给出现在命中的连接和模型() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let root = host(dir.path(), &server.uri()).await;
    // A 跟着厂商走：可选的是内置目录里的模型；B 只勾了一个。
    root.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "deepseek".to_owned(),
        name: "DeepSeek".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: server.uri(),
        api_key: "sk-test-not-a-real-key".to_owned(),
        preset: Some("deepseek".to_owned()),
        models: Some(Vec::new()),
        ..ConnectionSaveRequest::default()
    }))
    .await
    .expect("save A");
    root.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "other".to_owned(),
        name: "Other".to_owned(),
        model: "sonnet-x".to_owned(),
        endpoint: server.uri(),
        api_key: "sk-test-not-a-real-key".to_owned(),
        preset: Some("custom".to_owned()),
        ..ConnectionSaveRequest::default()
    }))
    .await
    .expect("save B");
    set_disabled(&root, vec!["*flash*", "*gpt-9*", "other/*"]).await;

    let response = root
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("model-settings");
    let Response::ModelSettings(response) = response else {
        panic!("expected ModelSettings");
    };
    assert_eq!(
        response.disabled_hits,
        vec![
            vec![
                "deepseek/deepseek-flash".to_owned(),
                "deepseek/deepseek-v4-flash".to_owned(),
                "deepseek/deepseek-v4-flash-vision-exp".to_owned(),
            ],
            Vec::new(),
            vec!["other/sonnet-x".to_owned()],
        ],
        "每条规则给出现在命中的「连接/模型」，一条都不命中的给空表"
    );
    assert_eq!(request_count(&server).await, 0, "保存连接与读设置不出站");
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
    assert_eq!(request_count(&server).await, 0, "查询规则不出站");
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
        .expect(1)
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
                // 只要这一轮停在等模型上；给足余量，避免并行负载下 3s 内跑完导致断言时已 idle。
                .set_delay(Duration::from_secs(30)),
        )
        .expect(1)
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

/// 跨连接：A 跑着时切到 B 只记选择、不重建；屏蔽 B 不能停 A，屏蔽 A 才以 hook 停。
/// check 的 Kind 行必须还是冻结的 A——读 selection.connection 会红。
#[tokio::test]
async fn 跨连接切换后屏蔽判断用根会话冻结的连接不用当前选择() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(TEXT_SSE)
                .set_delay(Duration::from_secs(10)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let identity = dir.path().join("identity.txt");
    // 真的准备 A/B 两个已保存连接：factory 和 check 都从同一份 Keyring 读。
    let keyring = credentials::Keyring::open(dir.path(), Some(&identity)).expect("keyring");
    for (id, model) in [
        ("deepseek", "deepseek-flash"),
        ("deepseek-b", "deepseek-v4-flash"),
    ] {
        keyring
            .save_connection(credentials::NewConnection {
                id: id.to_owned(),
                name: id.to_owned(),
                model: model.to_owned(),
                endpoint: server.uri(),
                api_key: "sk-test-not-a-real-key".to_owned(),
                preset: Some("deepseek".to_owned()),
                ..credentials::NewConnection::default()
            })
            .expect("save connection");
    }
    let mut root_options = crate::common::options(dir.path());
    root_options.model.base_url = server.uri();
    root_options.identity = Some(identity.clone());
    root_options.credentials = None;
    root_options.model_factory = Some(keyring_factory(dir.path(), &identity));
    let root = RootHost::open(root_options, DEFAULT_CONTEXT_WINDOW).await;

    send(&root, "r1", "用 A 跑一轮").await;
    wait_host_running(&root).await;
    for _ in 0..600 {
        if request_count(&server).await == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let requests = server.received_requests().await.expect("请求记录");
    assert_eq!(requests.len(), 1, "A 的一轮已经出站");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("body json");
    assert_eq!(body["model"], "deepseek-flash", "跑着的一轮用连接 A 的模型");

    // 运行中跨连接切到 B：只记选择，根会话仍是 A（不重建）。
    let selected = root
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            connection: Some("deepseek-b".to_owned()),
            model: Some("deepseek-v4-flash".to_owned()),
            effort_wanted: None,
        }))
        .await
        .expect("换到 B");
    let Response::SessionModelSelect(selected) = selected else {
        panic!("expected SessionModelSelect");
    };
    assert_eq!(selected.connection, "deepseek-b", "选择已经换成 B");

    // check 的 Kind 行必须还是冻结的 A：读 selection.connection 的话，这里会只剩命中表。
    let a = check_rule(&root, "deepseek/deepseek-flash").await;
    assert!(
        a.hits.contains(&"deepseek/deepseek-flash".to_owned()),
        "命中表列出 A：{:?}",
        a.hits
    );
    assert_eq!(
        a.users,
        vec![DisabledModelUser::KindChat { running: true }],
        "Kind 对话是冻结的 A，不在已经选中的 B 上"
    );

    // 屏蔽 B：跑着的 A 不动。
    set_disabled(&root, vec!["deepseek-b/deepseek-v4-flash"]).await;
    let status = root
        .dispatch(Call::Status(StatusRequest {}))
        .await
        .expect("status");
    let Response::Status(status) = status else {
        panic!("expected Status");
    };
    assert!(status.running, "屏蔽 B 不能停跑着的 A");

    // 屏蔽 A：才以 hook 停。
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
    assert_eq!(request_count(&server).await, 1, "打断后没有新出站");
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
        .expect(1)
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

/// 空 Keyring 的宿主：根会话 Unavailable，`connection-save` 会走写 lastSelection 的那条路。
fn unavailable_options(data_dir: &Path) -> RootSessionOptions {
    let mut options = crate::common::options(data_dir);
    let factory: service::ModelFactory =
        std::sync::Arc::new(|_selection| Err("还没有接模型".to_owned()));
    options.model_factory = Some(factory);
    options
}

async fn model_settings(host: &RootHost) -> service::ModelSettingsResponse {
    match host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("model-settings")
    {
        Response::ModelSettings(response) => response,
        other => panic!("expected ModelSettings：{other:?}"),
    }
}

#[track_caller]
fn assert_settings_unreadable<T: std::fmt::Debug>(result: Result<T, ServiceError>) {
    let Err(error) = result else {
        panic!("坏文件不能写成功：{result:?}");
    };
    assert_eq!(error.code().as_str(), "settings/unreadable");
    assert!(
        matches!(error, ServiceError::SettingsUnreadable(_)),
        "结构化错误：{error:?}"
    );
}

/// C1：读到合法文件就记下「上次读到的」——不经写入口（save 也会记，覆盖不到 read 这条路径）。
#[tokio::test]
async fn 读到合法设置文件就记下屏蔽表() {
    let dir = tempdir().unwrap();
    std::fs::write(
        service::settings::path(dir.path()),
        br#"{"disabledModels":["m1"]}"#,
    )
    .expect("写合法设置");
    let host = host(dir.path(), "http://127.0.0.1:1").await;
    let read = model_settings(&host).await;
    assert_eq!(read.disabled_models, vec!["m1".to_owned()]);
    assert!(!read.disabled_unknown, "读到过就不是「不知道」");

    std::fs::write(service::settings::path(dir.path()), b"{not json").expect("写坏");
    let after = model_settings(&host).await;
    assert_eq!(
        after.disabled_models,
        vec!["m1".to_owned()],
        "读到时记下的表要在坏文件上沿用"
    );
    assert!(!after.disabled_unknown, "不是「从没读到过」");
}

/// 设置文件坏掉：读到过的那份屏蔽表继续生效；从没读到过的数据目录仍然是「不知道」，
/// 不串用别的目录读到的表。
#[tokio::test]
async fn 坏设置文件沿用各自路径上次读到的屏蔽表() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let read_dir = tempdir().unwrap();
    let never_dir = tempdir().unwrap();
    let read_host = host(read_dir.path(), &server.uri()).await;
    let never_host = host(never_dir.path(), &server.uri()).await;

    // 读到一份表：这一步同时记下「上次读到的」。
    read_host
        .dispatch(Call::DisabledModelsSet(DisabledModelsSetRequest {
            models: vec!["m1".to_owned()],
        }))
        .await
        .expect("set");
    std::fs::write(service::settings::path(read_dir.path()), b"{not json").expect("写坏");
    std::fs::write(service::settings::path(never_dir.path()), b"{not json").expect("写坏");
    let read_before =
        std::fs::read(service::settings::path(read_dir.path())).expect("read settings");
    let never_before =
        std::fs::read(service::settings::path(never_dir.path())).expect("never settings");

    let read_view = model_settings(&read_host).await;
    assert_eq!(
        read_view.disabled_models,
        vec!["m1".to_owned()],
        "坏文件沿用上次读到的表"
    );
    assert!(!read_view.disabled_unknown, "读到过就不是「不知道」");

    let never_view = model_settings(&never_host).await;
    assert!(
        never_view.disabled_models.is_empty(),
        "从没读到过没有表：{:?}",
        never_view.disabled_models
    );
    assert!(
        never_view.disabled_unknown,
        "从没读到过又坏了：不知道哪些被屏蔽，不能当空表"
    );

    // 出站拦截用同一份判断：读到过的按表拦；从没读到过的全拒，说明带路径。
    let blocked = service::disabled::admit(read_dir.path(), "deepseek", "m1").expect_err("被拦");
    assert_eq!(blocked.code, model::ErrorCode::ModelDisabled);
    assert!(
        blocked.message.contains("规则「m1」"),
        "{}",
        blocked.message
    );
    assert!(
        !blocked.message.contains("settings.json 读不出来"),
        "上次读到的表还在，不该说不知道：{}",
        blocked.message
    );
    let unknown =
        service::disabled::admit(never_dir.path(), "deepseek", "deepseek-flash").expect_err("全拒");
    assert_eq!(unknown.code, model::ErrorCode::ModelDisabled);
    assert!(
        unknown.message.contains("settings.json 读不出来"),
        "{}",
        unknown.message
    );
    assert!(
        unknown.message.contains(
            &service::settings::path(never_dir.path())
                .display()
                .to_string()
        ),
        "说明里带 settings.json 路径：{}",
        unknown.message
    );
    assert!(
        unknown.message.contains("不知道哪些模型被屏蔽了"),
        "说明原因：{}",
        unknown.message
    );

    // 读不出来的文件一个字节都不写。
    assert_eq!(
        std::fs::read(service::settings::path(read_dir.path())).expect("settings"),
        read_before,
        "读不能改文件"
    );
    assert_eq!(
        std::fs::read(service::settings::path(never_dir.path())).expect("settings"),
        never_before,
        "读不能改文件"
    );
    assert_eq!(request_count(&server).await, 0, "读坏设置不出站");
}

/// 文件不存在算空表：不是「不知道」，所有模型照常能用，也不会被读操作建出来。
#[tokio::test]
async fn 设置文件不存在时屏蔽表为空也不当成不知道() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    assert!(
        !service::settings::path(dir.path()).exists(),
        "起宿主和读设置都不该建 settings.json"
    );

    let view = model_settings(&host).await;
    assert!(view.disabled_models.is_empty());
    assert!(!view.disabled_unknown, "文件不存在算空表，不是不知道");

    service::disabled::admit(dir.path(), "deepseek", "deepseek-flash")
        .expect("文件不存在：所有模型照常能用");
    assert!(
        !service::settings::path(dir.path()).exists(),
        "读一遍不能把文件写出来"
    );
    assert_eq!(request_count(&server).await, 0, "读设置不出站");
}

/// 坏文件上三个写设置的方法都拒绝（码 settings/unreadable），文件字节不变。
#[tokio::test]
async fn 坏设置文件时三个写设置入口都拒绝且不写() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    std::fs::write(service::settings::path(dir.path()), b"{not json").expect("写坏");
    let before = std::fs::read(service::settings::path(dir.path())).expect("settings.json");

    let set = host
        .dispatch(Call::DisabledModelsSet(DisabledModelsSetRequest {
            models: vec!["m1".to_owned()],
        }))
        .await;
    assert_settings_unreadable(set);

    let select = host
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            connection: Some("deepseek".to_owned()),
            model: Some("deepseek-flash".to_owned()),
            effort_wanted: Some("low".to_owned()),
        }))
        .await;
    assert_settings_unreadable(select);

    let update = host
        .dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
            currency: Some("cny".to_owned()),
        }))
        .await;
    assert_settings_unreadable(update);

    assert_eq!(
        std::fs::read(service::settings::path(dir.path())).expect("settings.json"),
        before,
        "写入口都不能写坏文件"
    );
    assert_eq!(request_count(&server).await, 0, "被拒绝的写入口不出站");
}

/// 未接模型时 connection-save 会写 lastSelection：坏文件上同样拒绝，文件字节不变。
#[tokio::test]
async fn 坏设置文件时未接模型的连接保存拒绝且不写() {
    let data = tempdir().unwrap();
    let host = RootHost::open(unavailable_options(data.path()), DEFAULT_CONTEXT_WINDOW).await;
    std::fs::write(service::settings::path(data.path()), b"{not json").expect("写坏");
    let before = std::fs::read(service::settings::path(data.path())).expect("settings.json");

    let result = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "minimax".to_owned(),
            name: "MiniMax".to_owned(),
            model: "MiniMax-M3.1-Flash".to_owned(),
            endpoint: "http://127.0.0.1:1/anthropic".to_owned(),
            api_key: "sk-test-not-a-real-key".to_owned(),
            preset: Some("custom".to_owned()),
            ..ConnectionSaveRequest::default()
        }))
        .await;
    assert_settings_unreadable(result);
    assert_eq!(
        std::fs::read(service::settings::path(data.path())).expect("settings.json"),
        before,
        "不写坏文件"
    );
}

/// 已经接上模型时 connection-save 不碰 lastSelection：坏文件上不报错、也不写。
#[tokio::test]
async fn 已接模型时坏设置文件上的连接保存不写也不报错() {
    let server = MockServer::start().await;
    mock_turn(&server).await;
    let dir = tempdir().unwrap();
    let host = host(dir.path(), &server.uri()).await;
    std::fs::write(service::settings::path(dir.path()), b"{not json").expect("写坏");
    let before = std::fs::read(service::settings::path(dir.path())).expect("settings.json");

    let response = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "other".to_owned(),
            name: "Other".to_owned(),
            model: "sonnet-x".to_owned(),
            endpoint: server.uri(),
            api_key: "sk-test-not-a-real-key".to_owned(),
            preset: Some("custom".to_owned()),
            ..ConnectionSaveRequest::default()
        }))
        .await
        .expect("已接模型：connection-save 不写 lastSelection，不该报设置读不出来");
    let Response::ConnectionSave(response) = response else {
        panic!("expected ConnectionSave");
    };
    assert_eq!(response.connection.id, "other");
    assert_eq!(
        std::fs::read(service::settings::path(dir.path())).expect("settings.json"),
        before,
        "没有写 settings.json"
    );
    assert_eq!(request_count(&server).await, 0, "连接保存不出站");
}
