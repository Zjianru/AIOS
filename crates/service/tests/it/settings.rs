use std::path::Path;
use std::time::Duration;

use agent::AgentConfig;
use credentials::CheckState;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, ConnectionDeleteRequest, ConnectionRevokeRequest, ConnectionSaveRequest,
    ConnectionUpdateRequest, ConnectionVerifyRequest, FirstUseRequest, IssuesRequest,
    KeyringRelocateRequest, KeyringReopenRequest, KeyringRestartRequest, ModelSettingsRequest,
    RootHost, RootSessionOptions, SessionModelSelectRequest,
};
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TEST_KEY: &str = "sk-t008-service-0123456789";

/// T008.F9：测试凭证来源（产品不再有固定 key 构造入口，测试显式注入）。
struct TestCredential;

impl model::CredentialSource for TestCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok("test-key".to_owned())
    }
}

fn test_credential() -> std::sync::Arc<dyn model::CredentialSource> {
    std::sync::Arc::new(TestCredential)
}

/// 按脚本顺序回放（第 N 次请求拿第 N 个响应；耗尽 500）。
struct SequenceResponder {
    responses: std::sync::Mutex<std::collections::VecDeque<ResponseTemplate>>,
}

impl SequenceResponder {
    fn new(responses: Vec<ResponseTemplate>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses.into()),
        }
    }
}

impl Respond for SequenceResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| ResponseTemplate::new(500))
    }
}

fn options(data_dir: &Path, identity: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: data_dir.to_path_buf(),
        model: DeepSeekConfig {
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::new("high"),
        },
        config: AgentConfig::default(),
        credentials: Some(test_credential()),
        identity: Some(identity.to_path_buf()),
        model_factory: None,
        append_hook_factory: None,
        keyring_changed: None,
    }
}

async fn host(dir: &Path) -> RootHost {
    RootHost::open(
        options(dir, &dir.join("identity.txt")),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await
}

fn no_key(json: &str) {
    assert!(
        !json.contains(TEST_KEY),
        "响应/事件里不能出现 key 正文：{json}"
    );
}

#[tokio::test]
async fn 第一次使用视图没有正文() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let response = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await;
    assert!(response.is_ok(), "model-settings 应可用：{response:?}");
    let json = serde_json::to_string(&response.unwrap()).unwrap();
    no_key(&json);
    assert!(
        json.contains("first_use") || json.contains("firstUse"),
        "json={json}"
    );
}

#[tokio::test]
async fn 存连接删连接都无正文() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "connection-save 应成功：{saved:?}");
    no_key(&serde_json::to_string(&saved.unwrap()).unwrap());

    let view = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("settings");
    no_key(&serde_json::to_string(&view).unwrap());

    let deleted = host
        .dispatch(Call::ConnectionDelete(ConnectionDeleteRequest {
            id: "deepseek".to_owned(),
        }))
        .await;
    assert!(deleted.is_ok(), "connection-delete 应成功：{deleted:?}");
    no_key(&serde_json::to_string(&deleted.unwrap()).unwrap());
}

#[tokio::test]
async fn 换位置指回重新开始和记录() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");

    let issues = host.dispatch(Call::Issues(IssuesRequest {})).await;
    assert!(issues.is_ok(), "issues: {issues:?}");
    no_key(&serde_json::to_string(&issues.unwrap()).unwrap());

    let first = host.dispatch(Call::FirstUse(FirstUseRequest {})).await;
    assert!(first.is_ok(), "first-use: {first:?}");
    let json = serde_json::to_string(&first.unwrap()).unwrap();
    assert!(
        !json.contains("\"first_use\":true") && !json.contains("\"firstUse\":true"),
        "已有连接：{json}"
    );
}

#[tokio::test]
async fn 根会话不可用时设置仍可用() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(dir.path().join("kind-root.json"), "{not json").unwrap();
    let host = RootHost::open(
        options(dir.path(), &dir.path().join("identity.txt")),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
    let response = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await;
    assert!(response.is_ok(), "根会话坏了设置也应可用：{response:?}");
}

#[tokio::test]
async fn 换位置指回重新开始() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let moved = dir.path().join("moved-identity.txt");
    let relocated = host
        .dispatch(Call::KeyringRelocate(KeyringRelocateRequest {
            to: moved.display().to_string(),
        }))
        .await;
    assert!(relocated.is_ok(), "relocate: {relocated:?}");
    assert!(moved.exists(), "新位置应有钥匙文件");
    let reopened = host
        .dispatch(Call::KeyringReopen(KeyringReopenRequest {
            at: moved.display().to_string(),
        }))
        .await;
    assert!(reopened.is_ok(), "reopen: {reopened:?}");
    let restarted = host
        .dispatch(Call::KeyringRestart(KeyringRestartRequest {}))
        .await;
    assert!(restarted.is_ok(), "restart: {restarted:?}");
    let json = serde_json::to_string(&restarted.unwrap()).unwrap();
    no_key(&json);
}

#[tokio::test]
async fn 验证成功写注册器且响应无正文() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let verified = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "deepseek".to_owned(),
        }))
        .await;
    assert!(
        verified.is_ok(),
        "verify 应返回结果而不是协议错误：{verified:?}"
    );
    let json = serde_json::to_string(&verified.unwrap()).unwrap();
    no_key(&json);
    assert!(json.contains("ok"), "验证成功应写进注册器：{json}");
}

#[tokio::test]
async fn 验证失败写原因且响应无正文() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {"message": "bad key", "type": "authentication_error"}
        })))
        .mount(&server)
        .await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let verified = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "deepseek".to_owned(),
        }))
        .await;
    assert!(verified.is_ok(), "verify 结果应返回：{verified:?}");
    let json = serde_json::to_string(&verified.unwrap()).unwrap();
    no_key(&json);
    assert!(json.contains("fail"), "验证失败原因应写进注册器：{json}");
}

#[tokio::test]
async fn 设置变化会发事件且事件无正文() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let mut events = host.subscribe_events();
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("应收到设置变化事件")
        .expect("事件通道不应关闭");
    assert_eq!(event.event_type, "keyring/changed", "event={event:?}");
    no_key(&serde_json::to_string(&event).unwrap());
}

#[tokio::test]
async fn 选模型下一轮生效() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let response = host
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            model: Some("deepseek-pro".to_owned()),
            ..Default::default()
        }))
        .await;
    assert!(
        response.is_ok(),
        "session-model-select 应成功：{response:?}"
    );
    let json = serde_json::to_string(&response.unwrap()).unwrap();
    assert!(json.contains("deepseek-pro"), "json={json}");
    no_key(&json);
}

#[tokio::test]
async fn 编辑连接保留key且不换正文() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "deepseek".to_owned(),
        name: "DeepSeek".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: "https://api.deepseek.com/anthropic".to_owned(),
        api_key: TEST_KEY.to_owned(),
        ..Default::default()
    }))
    .await
    .expect("save");
    let updated = host
        .dispatch(Call::ConnectionUpdate(ConnectionUpdateRequest {
            id: "deepseek".to_owned(),
            endpoint: "https://new.example/anthropic".to_owned(),
            model: "deepseek-pro".to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(updated.is_ok(), "connection-update 应成功：{updated:?}");
    let json = serde_json::to_string(&updated.unwrap()).unwrap();
    no_key(&json);
    assert!(json.contains("https://new.example/anthropic"), "{json}");
    assert!(json.contains("deepseek-pro"), "{json}");
    assert!(
        json.contains("key_saved") && json.contains("true"),
        "{json}"
    );
    let keyring =
        credentials::Keyring::open(dir.path(), Some(&dir.path().join("identity.txt"))).unwrap();
    assert_eq!(
        keyring
            .issue("deepseek", "test", None, None)
            .unwrap()
            .into_secret(),
        TEST_KEY
    );
}

#[tokio::test]
async fn 撤销key后连接还在但签发失败() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "deepseek".to_owned(),
        name: "DeepSeek".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: "https://api.deepseek.com/anthropic".to_owned(),
        api_key: TEST_KEY.to_owned(),
        ..Default::default()
    }))
    .await
    .expect("save");
    let revoked = host
        .dispatch(Call::ConnectionRevoke(ConnectionRevokeRequest {
            id: "deepseek".to_owned(),
        }))
        .await;
    assert!(revoked.is_ok(), "connection-revoke 应成功：{revoked:?}");
    let json = serde_json::to_string(&revoked.unwrap()).unwrap();
    no_key(&json);
    assert!(
        json.contains("key_saved") && json.contains("false"),
        "{json}"
    );
    let keyring =
        credentials::Keyring::open(dir.path(), Some(&dir.path().join("identity.txt"))).unwrap();
    assert!(
        keyring.issue("deepseek", "test", None, None).is_err(),
        "撤销后签发应失败"
    );
}

#[tokio::test]
async fn 验证连接走模型适配器成功时记可用() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(""),
        )
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "verify-ok".to_owned(),
            name: "Verify".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let verified = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "verify-ok".to_owned(),
        }))
        .await;
    let service::Response::ConnectionVerify(response) = verified.expect("verify 应成功") else {
        panic!("expected ConnectionVerify");
    };
    assert!(
        matches!(response.connection.check, CheckState::Ok { .. }),
        "验证成功记可用：{:?}",
        response.connection.check
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "验证必须只发一条最小请求");
    let request = &requests[0];
    assert_eq!(
        request
            .headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok()),
        Some(TEST_KEY),
        "验证请求带 Keyring 签发的 key"
    );
    assert!(
        request
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json")),
        "验证请求走模型适配器的 JSON 头"
    );
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json");
    assert!(
        body.get("messages")
            .and_then(serde_json::Value::as_array)
            .is_some(),
        "验证请求是对话形状：{body}"
    );
    assert_eq!(body["messages"][0]["role"], "user", "body={body}");
    assert!(body.get("max_tokens").is_some(), "body={body}");
}

#[tokio::test]
async fn 验证连接失败时注册器按码记() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {"type": "authentication_error", "message": "bad key"}
        })))
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "verify-bad".to_owned(),
        name: "Bad".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: server.uri(),
        api_key: TEST_KEY.to_owned(),
        ..Default::default()
    }))
    .await
    .expect("save");
    let verified = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "verify-bad".to_owned(),
        }))
        .await;
    let service::Response::ConnectionVerify(response) = verified.expect("verify 返回响应")
    else {
        panic!("expected ConnectionVerify");
    };
    match response.connection.check {
        CheckState::Fail { reason, .. } => assert_eq!(reason, "AUTH", "按码记原因"),
        other => panic!("失败应记 Fail：{other:?}"),
    }
}
#[tokio::test]
async fn 验证新形状用主模型且请求符合思考档规则() {
    // K3：models[0]=MiniMax-M3.1-Flash，旧的 model 字段是 deepseek-flash；
    // 验证必须用主模型 M3.1，并且请求体满足 R3（max_tokens>2048，thinking enabled 且 budget>1）。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "k3".to_owned(),
            name: "K3".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            models: Some(vec![credentials::ModelRow {
                id: "MiniMax-M3.1-Flash".to_owned(),
                name: None,
                context_window: Some(1_000_000),
                max_tokens: Some(524_288),
                price: None,
            }]),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let verified = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "k3".to_owned(),
        }))
        .await;
    let service::Response::ConnectionVerify(response) = verified.expect("verify 返回") else {
        panic!("expected ConnectionVerify");
    };
    assert!(matches!(response.connection.check, CheckState::Ok { .. }));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("json");
    assert_eq!(body["model"], "MiniMax-M3.1-Flash", "K3 用主模型：{body}");
    let max_tokens = body["max_tokens"].as_u64().expect("max_tokens") as u32;
    assert!(max_tokens > 2048, "验证请求不能压到 2048 以下：{body}");
    assert_eq!(
        body["thinking"]["type"], "enabled",
        "M3.1 关不掉思考：{body}"
    );
    let budget = body["thinking"]["budget_tokens"].as_u64().expect("budget");
    assert!(budget > 1, "budget 不能被 max_tokens=1 压到 1：{body}");
}
#[tokio::test]
async fn 获取可用模型合并目录与厂商列表且用户字段不动() {
    model::catalog::set_global(model::catalog::snapshot());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "deepseek-flash", "display_name": "Vendor Flash", "context_length": 1000, "max_output_tokens": 200},
                {"id": "deepseek-v4-pro", "display_name": "Vendor Pro", "context_length": 2000, "max_output_tokens": 300},
                {"id": "deepseek-future", "display_name": "Future", "context_length": 3000}
            ]
        })))
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "list".to_owned(),
            name: "List".to_owned(),
            model: "old-model".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            preset: Some("deepseek".to_owned()),
            models: Some(vec![
                credentials::ModelRow {
                    id: "deepseek-flash".to_owned(),
                    name: Some("我的名字".to_owned()),
                    context_window: Some(42),
                    max_tokens: Some(111),
                    price: Some(9.9),
                },
                credentials::ModelRow {
                    id: "my-only-model".to_owned(),
                    name: None,
                    context_window: None,
                    max_tokens: None,
                    price: None,
                },
            ]),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let response = host
        .dispatch(Call::AvailableModels(service::AvailableModelsRequest {
            id: "list".to_owned(),
        }))
        .await
        .expect("available models");
    let service::Response::AvailableModels(response) = response else {
        panic!("expected AvailableModels");
    };
    assert!(response.live, "厂商列表成功");
    let find = |id: &str| {
        response
            .models
            .iter()
            .find(|model| model.id == id)
            .expect(id)
    };
    let flash = find("deepseek-flash");
    assert_eq!(flash.name, "我的名字", "用户填的名字不被厂商覆盖");
    assert_eq!(flash.context_window, Some(42), "用户填的窗口不动");
    assert_eq!(flash.max_tokens, Some(111), "用户填的输出上限不动");
    assert_eq!(flash.price_input, Some(9.9), "用户价格不动");
    assert!(flash.thinks, "thinks 来自目录");
    assert!(flash.efforts.contains(&"none".to_owned()), "档来自目录");
    let pro = find("deepseek-v4-pro");
    assert_eq!(pro.name, "Vendor Pro", "厂商字段优先于目录");
    assert_eq!(pro.context_window, Some(2000));
    assert!(pro.price_input.is_some(), "目录价格仍在");
    assert_eq!(
        find("deepseek-future").context_window,
        Some(3000),
        "厂商多出的模型也列出"
    );
    assert!(!find("my-only-model").thinks);
}

#[tokio::test]
async fn 获取可用模型时_anthropic_端点请求_chat_base_的列表() {
    // 审计 T012.5 fix-now 1：endpoint 是 …/anthropic 时，列表必须请求 Chat base 的
    // /v1/models（不是 …/anthropic/v1/models）；用带 /anthropic 的 endpoint 断言路径。
    model::catalog::set_global(model::catalog::snapshot());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "deepseek-flash", "display_name": "Chat base 列表"}]
        })))
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "list-chat-base".to_owned(),
            name: "Anthropic base".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: format!("{}/anthropic", server.uri()),
            api_key: TEST_KEY.to_owned(),
            preset: Some("deepseek".to_owned()),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let response = host
        .dispatch(Call::AvailableModels(service::AvailableModelsRequest {
            id: "list-chat-base".to_owned(),
        }))
        .await
        .expect("available models");
    let service::Response::AvailableModels(response) = response else {
        panic!("expected AvailableModels");
    };
    assert!(
        response.live,
        "带 /anthropic 的端点要请求 Chat base 的 /v1/models"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "只发一次列表请求");
    assert_eq!(
        requests[0].url.path(),
        "/v1/models",
        "列表路径是 Chat base：{}",
        requests[0].url
    );
}

#[tokio::test]
async fn 验证连接时学到的鉴权头写回连接() {
    // T012.6：验证走真实适配器；双头被拒后各试一次单头，记住通过的那个并写回连接。
    let server = MockServer::start().await;
    let both_refused = ResponseTemplate::new(401).set_body_json(serde_json::json!({
        "type": "error",
        "error": {
            "type": "authentication_error",
            "message": "x-api-key 和 Authorization 不可以同时存在，只能使用一个"
        }
    }));
    let unauthorized = ResponseTemplate::new(401).set_body_json(serde_json::json!({
        "type": "error",
        "error": {"type": "authentication_error", "message": "bad key"}
    }));
    let ok = ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            both_refused,
            unauthorized,
            ok.clone(),
            ok,
        ]))
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "learn-auth".to_owned(),
            name: "Learn auth".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let verified = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "learn-auth".to_owned(),
        }))
        .await;
    let service::Response::ConnectionVerify(response) = verified.expect("verify 返回") else {
        panic!("expected ConnectionVerify");
    };
    assert!(
        matches!(response.connection.check, CheckState::Ok { .. }),
        "单头轮换后验证成功：{:?}",
        response.connection.check
    );
    assert_eq!(
        response.connection.auth_header.as_deref(),
        Some("authorization"),
        "学到的鉴权头写回连接"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3, "双头被拒 + 两个单头各试一次");
    let has = |index: usize, header: &str| requests[index].headers.contains_key(header);
    assert!(
        has(0, "x-api-key") && has(0, "authorization"),
        "第一次两个头一起发"
    );
    assert!(
        has(1, "x-api-key") && !has(1, "authorization"),
        "先只试 x-api-key"
    );
    assert!(
        !has(2, "x-api-key") && has(2, "authorization"),
        "再只试 authorization"
    );

    // T012.6 P2：第二次验证读连接上记住的 auth_header，第一次请求就只带 authorization。
    let again = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "learn-auth".to_owned(),
        }))
        .await;
    let service::Response::ConnectionVerify(again) = again.expect("verify 返回") else {
        panic!("expected ConnectionVerify");
    };
    assert!(
        matches!(again.connection.check, CheckState::Ok { .. }),
        "第二次验证用记住的单头成功"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        4,
        "第二次验证只发一次请求（不再从双头开始）"
    );
    assert!(
        !requests[3].headers.contains_key("x-api-key")
            && requests[3].headers.contains_key("authorization"),
        "第二次验证直接只带学到的 authorization"
    );
}

/// 根会话 id（读状态文件；测试断言 session/model 事件用）。
fn root_session_id(dir: &std::path::Path) -> session::SessionId {
    let text = std::fs::read_to_string(dir.join(service::ROOT_STATE_FILE)).expect("root state");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    session::SessionId::parse(value["sessionId"].as_str().expect("sessionId")).expect("id")
}

#[tokio::test]
async fn 选模型写会话日志事件并保存上次选择() {
    // T012.7 故事 2/8：选择是 durable 事实（session/model 事件 + settings 上次选择）。
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let response = host
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            connection: Some("deepseek".to_owned()),
            model: Some("deepseek-pro".to_owned()),
            effort_wanted: Some("low".to_owned()),
        }))
        .await
        .expect("session-model-select");
    let service::Response::SessionModelSelect(response) = response else {
        panic!("expected SessionModelSelect");
    };
    assert_eq!(response.connection, "deepseek");
    assert_eq!(response.model, "deepseek-pro");
    assert_eq!(response.effort_wanted, "low");
    // durable：会话日志里有 session/model 事件（重开会话从日志恢复）。
    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    let selection = session::last_model_selection(snapshot.events()).expect("session/model 事件");
    assert_eq!(
        selection,
        session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-pro".to_owned(),
            effort_wanted: "low".to_owned(),
        }
    );
    // settings.json：新会话默认用上一次选的。
    let settings = service::settings::load(dir.path());
    assert_eq!(settings.last_selection, Some(selection));
}

#[tokio::test]
async fn 重开会话从日志恢复模型选择() {
    // T012.7 故事 2：重开会话还是它（工厂收到日志里的选择）。
    fn recording_factory(
        seen: std::sync::Arc<std::sync::Mutex<Option<session::ModelSelection>>>,
    ) -> service::ModelFactory {
        std::sync::Arc::new(move |selection: Option<&session::ModelSelection>| {
            *seen.lock().expect("seen") = selection.cloned();
            Ok((
                "deepseek".to_owned(),
                DeepSeekConfig::default(),
                Some(test_credential()),
            ))
        })
    }
    let dir = tempdir().unwrap();
    let identity = dir.path().join("identity.txt");
    let first_seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let mut first_options = options(dir.path(), &identity);
    first_options.model_factory = Some(recording_factory(std::sync::Arc::clone(&first_seen)));
    let host = RootHost::open(first_options, DEFAULT_CONTEXT_WINDOW).await;
    assert_eq!(
        *first_seen.lock().expect("seen"),
        None,
        "新会话没有会话选择"
    );
    host.dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
        connection: Some("deepseek".to_owned()),
        model: Some("deepseek-pro".to_owned()),
        effort_wanted: Some("low".to_owned()),
    }))
    .await
    .expect("select");
    drop(host);
    // 重开：工厂收到日志里的选择。
    let second_seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let mut second_options = options(dir.path(), &identity);
    second_options.model_factory = Some(recording_factory(std::sync::Arc::clone(&second_seen)));
    let _host = RootHost::open(second_options, DEFAULT_CONTEXT_WINDOW).await;
    assert_eq!(
        *second_seen.lock().expect("seen"),
        Some(session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-pro".to_owned(),
            effort_wanted: "low".to_owned(),
        }),
        "重开会话从日志恢复选择"
    );
}

/// T012.7 审计：空闲时跨连接切换重建根 agent——factory 收到新选择、下一轮走新连接、
/// settings 的上次选择同步。（运行中跨连接的 known：不打断本轮，见 handback。）
#[tokio::test]
async fn 空闲时跨连接切换重建根agent并走新连接() {
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;
    for server in [&server_a, &server_b] {
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
            )
            .mount(server)
            .await;
    }
    let dir = tempdir().unwrap();
    let identity = dir.path().join("identity.txt");
    let keyring = credentials::Keyring::open(dir.path(), Some(&identity)).unwrap();
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server_a.uri(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        })
        .expect("save A");
    keyring
        .save_connection(credentials::NewConnection {
            id: "minimax-cn".to_owned(),
            name: "MiniMax".to_owned(),
            model: "MiniMax-M3.1-Flash".to_owned(),
            endpoint: server_b.uri(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        })
        .expect("save B");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(
        Vec::<Option<session::ModelSelection>>::new(),
    ));
    let seen_for_factory = std::sync::Arc::clone(&seen);
    let uri_a = server_a.uri();
    let uri_b = server_b.uri();
    let factory: service::ModelFactory =
        std::sync::Arc::new(move |selection: Option<&session::ModelSelection>| {
            seen_for_factory
                .lock()
                .expect("seen")
                .push(selection.cloned());
            let connection = selection
                .map(|selection| selection.connection.clone())
                .unwrap_or_else(|| "deepseek".to_owned());
            let (endpoint, model) = if connection == "minimax-cn" {
                (uri_b.clone(), "MiniMax-M3.1-Flash")
            } else {
                (uri_a.clone(), "deepseek-flash")
            };
            Ok((
                connection,
                DeepSeekConfig {
                    base_url: endpoint,
                    model: model.to_owned(),
                    ..DeepSeekConfig::default()
                },
                Some(test_credential()),
            ))
        });
    let mut options = options(dir.path(), &identity);
    options.model_factory = Some(factory);
    let host = RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await;
    assert_eq!(
        *seen.lock().expect("seen"),
        vec![None],
        "新会话 factory 收到 None"
    );
    // 先跑一轮：走连接 A（初始选择）。
    host.dispatch(Call::Send(service::SendRequest {
        request_id: "r1".to_owned(),
        content: "hi".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
    }))
    .await
    .expect("send A");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        server_a.received_requests().await.unwrap().len(),
        1,
        "初始走连接 A"
    );
    // 空闲时换到连接 B：重建根 agent，factory 收到新选择。
    let selected = host
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            connection: Some("minimax-cn".to_owned()),
            model: Some("MiniMax-M3.1-Flash".to_owned()),
            effort_wanted: Some("low".to_owned()),
        }))
        .await
        .expect("select B");
    let service::Response::SessionModelSelect(selected) = selected else {
        panic!("expected SessionModelSelect");
    };
    assert_eq!(selected.connection, "minimax-cn");
    let selections = seen.lock().expect("seen").clone();
    assert_eq!(selections.len(), 2, "重建时 factory 被调用：{selections:?}");
    assert_eq!(
        selections[1]
            .as_ref()
            .map(|selection| selection.connection.as_str()),
        Some("minimax-cn"),
        "factory 收到新选择"
    );
    // 下一轮走连接 B。
    host.dispatch(Call::Send(service::SendRequest {
        request_id: "r2".to_owned(),
        content: "hi again".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
    }))
    .await
    .expect("send B");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        server_a.received_requests().await.unwrap().len(),
        1,
        "连接 A 不再收到请求"
    );
    assert_eq!(
        server_b.received_requests().await.unwrap().len(),
        1,
        "下一轮走新连接 B"
    );
    assert_eq!(
        service::settings::load(dir.path())
            .last_selection
            .map(|selection| selection.connection),
        Some("minimax-cn".to_owned()),
        "settings 的上次选择同步"
    );
}
