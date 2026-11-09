use std::path::Path;
use std::time::Duration;

use crate::common::SequenceResponder;
use credentials::CheckState;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, ConnectionDeleteRequest, ConnectionQuotaRequest, ConnectionRevokeRequest,
    ConnectionSaveRequest, ConnectionUpdateRequest, ConnectionVerifyRequest, FirstUseRequest,
    IssuesRequest, KeyringRelocateRequest, KeyringReopenRequest, KeyringRestartRequest,
    ModelSettingsRequest, RootHost, ServiceError, SessionModelSelectRequest,
};
use tempfile::tempdir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_KEY: &str = "sk-t008-service-0123456789";

/// 一轮成功并报 usage 的 SSE：A→B 热切换要证明已用上下文没被归零。
const USAGE_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_usage","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":9,"output_tokens":2}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"完成。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

async fn host(dir: &Path) -> RootHost {
    crate::common::open(crate::common::options(dir)).await
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
    let host = RootHost::open(crate::common::options(dir.path()), DEFAULT_CONTEXT_WINDOW).await;
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
        .expect(1)
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
        .expect(1)
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
        .expect(1)
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
        .expect(1)
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
        .expect(1)
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
                price_input: None,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
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
        .expect(1)
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
                    price_input: Some(9.9),
                    price_output: None,
                    price_cache_read: None,
                    price_cache_write: None,
                },
                credentials::ModelRow {
                    id: "my-only-model".to_owned(),
                    name: None,
                    context_window: None,
                    max_tokens: None,
                    price_input: None,
                    price_output: None,
                    price_cache_read: None,
                    price_cache_write: None,
                },
            ]),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");
    let response = host
        .dispatch(Call::AvailableModels(discover_request("list")))
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
            .find(|row| row.model.id == id)
            .expect(id)
    };
    let flash = find("deepseek-flash");
    assert_eq!(flash.model.name, "我的名字", "用户填的名字不被厂商覆盖");
    assert_eq!(flash.model.context_window, Some(42), "用户填的窗口不动");
    assert_eq!(flash.model.max_tokens, Some(111), "用户填的输出上限不动");
    assert_eq!(flash.model.price_input, Some(9.9), "用户价格不动");
    assert!(flash.model.thinks, "thinks 来自目录");
    assert!(
        flash.model.efforts.contains(&"none".to_owned()),
        "档来自目录"
    );
    let pro = find("deepseek-v4-pro");
    assert_eq!(pro.model.name, "Vendor Pro", "厂商字段优先于目录");
    assert_eq!(pro.model.context_window, Some(2000));
    assert!(pro.model.price_input.is_some(), "目录价格仍在");
    assert_eq!(
        find("deepseek-future").model.context_window,
        Some(3000),
        "厂商多出的模型也列出"
    );
    assert!(!find("my-only-model").model.thinks);
}

/// 「获取可用模型」（用户按了按钮）：服务端会把这一次的厂商列表存下来。
fn discover_request(id: &str) -> service::AvailableModelsRequest {
    service::AvailableModelsRequest {
        id: id.to_owned(),
        mode: service::AvailableModelsMode::Fetch,
    }
}

/// 页面只是看一眼（补价格/窗口/档）：服务端不写盘，也不挪动「新」的基准。
fn query_request(id: &str) -> service::AvailableModelsRequest {
    service::AvailableModelsRequest {
        id: id.to_owned(),
        mode: service::AvailableModelsMode::Query,
    }
}

/// 走接口问一次可用模型，返回整份响应。
async fn available_models(
    host: &RootHost,
    request: service::AvailableModelsRequest,
) -> service::AvailableModelsResponse {
    let response = host
        .dispatch(Call::AvailableModels(request))
        .await
        .expect("available models");
    let service::Response::AvailableModels(response) = response else {
        panic!("expected AvailableModels");
    };
    response
}

/// 设置页里某个连接存下的厂商列表（没有这个连接就是空表）。
async fn listed_ids(host: &RootHost, id: &str) -> Vec<String> {
    let response = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("model-settings");
    let service::Response::ModelSettings(settings) = response else {
        panic!("expected ModelSettings");
    };
    settings
        .listed
        .get(id)
        .map(|listed| listed.models.iter().map(|model| model.id.clone()).collect())
        .unwrap_or_default()
}

/// 从 Keyring 视图里取一个连接（设置页看到的形状）。
async fn connection_of(host: &RootHost, id: &str) -> credentials::ConnectionInfo {
    let response = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("model-settings");
    let service::Response::ModelSettings(settings) = response else {
        panic!("expected ModelSettings");
    };
    settings
        .settings
        .connections
        .into_iter()
        .find(|connection| connection.id == id)
        .unwrap_or_else(|| panic!("没有连接 {id}"))
}

/// 把一次 available-models 响应里某个模型的 fresh 读出来。
fn fresh_of(response: &service::AvailableModelsResponse, id: &str) -> bool {
    response
        .models
        .iter()
        .find(|row| row.model.id == id)
        .unwrap_or_else(|| panic!("响应里没有 {id}"))
        .fresh
}

fn fresh_ids(response: &service::AvailableModelsResponse) -> Vec<&str> {
    response
        .models
        .iter()
        .filter(|row| row.fresh)
        .map(|row| row.model.id.as_str())
        .collect()
}

/// 读出 live 列表文件里的模型 id 和 fetchedAt。
fn stored_live(data_dir: &Path, connection: &str) -> (Vec<String>, u64) {
    let path = model::catalog::live_path(data_dir, connection);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到存下的厂商列表 {}：{error}", path.display()));
    assert!(!text.contains(TEST_KEY), "厂商列表文件里不能有 key：{text}");
    let value: serde_json::Value = serde_json::from_str(&text).expect("列表 JSON");
    let ids = value["models"]
        .as_array()
        .expect("models 数组")
        .iter()
        .map(|row| row["id"].as_str().expect("模型 id").to_owned())
        .collect();
    let fetched_at = value["fetchedAt"].as_u64().expect("fetchedAt");
    (ids, fetched_at)
}

/// 厂商列表接口只认 Authorization（MiniMax 国内站真 key：messages 双头都过，列表只认 Bearer）：
/// 连接没记过单个头时，列表请求要照 T012.6/Magpie 默认把两个头都发上，不能只发 x-api-key。
#[tokio::test]
async fn 厂商列表只认小写bearer时默认两个鉴权头都发() {
    model::catalog::set_global(model::catalog::snapshot());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header(
            "authorization",
            format!("Bearer {TEST_KEY}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "vendor-bearer-only"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "bearer".to_owned(),
            name: "Bearer".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            preset: Some("deepseek".to_owned()),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");

    let response = host
        .dispatch(Call::AvailableModels(discover_request("bearer")))
        .await
        .expect("获取");
    let service::Response::AvailableModels(response) = response else {
        panic!("expected AvailableModels");
    };
    assert!(
        response.live,
        "只认 Bearer 的列表接口也要成功（默认两个头都发）"
    );
    let (ids, _) = stored_live(dir.path(), "bearer");
    assert_eq!(ids, vec!["vendor-bearer-only".to_owned()]);
}

#[tokio::test]
async fn 获取成功只存厂商列表且第二次只标新增() {
    model::catalog::set_global(model::catalog::snapshot());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(SequenceResponder::new(vec![
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "deepseek-flash", "display_name": "Flash"},
                    {"id": "vendor-new-1", "display_name": "New One"}
                ]
            })),
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "deepseek-flash"},
                    {"id": "vendor-new-1"},
                    {"id": "vendor-new-2"}
                ]
            })),
        ]))
        .expect(2)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "fresh".to_owned(),
            name: "Fresh".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            preset: Some("deepseek".to_owned()),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "save: {saved:?}");

    let first = host
        .dispatch(Call::AvailableModels(discover_request("fresh")))
        .await
        .expect("第一次获取");
    let service::Response::AvailableModels(first) = first else {
        panic!("expected AvailableModels");
    };
    assert!(first.live, "厂商列表成功");
    assert!(
        fresh_ids(&first).is_empty(),
        "第一次获取没有上一次的列表可比较，一条都不标新"
    );
    assert!(
        !fresh_of(&first, "deepseek-v4-pro"),
        "目录独有项不是厂商这次给的"
    );
    let (ids, first_fetched_at) = stored_live(dir.path(), "fresh");
    assert_eq!(
        ids,
        vec!["deepseek-flash", "vendor-new-1"],
        "文件只含厂商这次给的模型（顺序也照厂商）"
    );
    assert!(first_fetched_at > 0);

    let second = host
        .dispatch(Call::AvailableModels(discover_request("fresh")))
        .await
        .expect("第二次获取");
    let service::Response::AvailableModels(second) = second else {
        panic!("expected AvailableModels");
    };
    assert_eq!(
        fresh_ids(&second),
        vec!["vendor-new-2"],
        "第二次只有厂商新增的那一个是新"
    );
    let (ids, second_fetched_at) = stored_live(dir.path(), "fresh");
    assert_eq!(
        ids,
        vec!["deepseek-flash", "vendor-new-1", "vendor-new-2"],
        "第二次覆盖成新的厂商列表"
    );
    assert!(second_fetched_at >= first_fetched_at);
}

/// 列表跟厂商 live 顺序走（不按版本重排）；目录里的字段照常合并，目录没有的模型没有标价。
#[tokio::test]
async fn minimax_列表保留厂商顺序() {
    model::catalog::set_global(model::catalog::snapshot());
    // 故意不按版本排：厂商先给 M3.1，再给 M2，最后给目录里没有的 M2.1-highspeed。
    let ids = vec!["MiniMax-M3.1-Flash", "MiniMax-M2", "MiniMax-M2.1-highspeed"];
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": ids.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>()
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "minimax-order".to_owned(),
            name: "MiniMax".to_owned(),
            model: "MiniMax-M3.1-Flash".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            preset: Some("minimax-cn".to_owned()),
            models: Some(Vec::new()),
            ..Default::default()
        }))
        .await
        .expect("save");
    let service::Response::ConnectionSave(saved) = saved else {
        panic!("expected ConnectionSave");
    };
    let response = available_models(&host, discover_request("minimax-order")).await;
    let find = |id: &str| {
        response
            .models
            .iter()
            .find(|row| row.model.id == id)
            .unwrap_or_else(|| panic!("响应里没有 {id}"))
    };
    let order: Vec<&str> = response
        .models
        .iter()
        .map(|row| row.model.id.as_str())
        .collect();
    assert_eq!(&order[..3], ids.as_slice(), "厂商顺序原样保留");
    assert!(
        find("MiniMax-M3.1-Flash").model.price_input.is_some(),
        "目录标价照常合并"
    );
    assert!(
        find("MiniMax-M2.1-highspeed").model.price_input.is_none(),
        "目录没有的模型没有标价"
    );
    assert_eq!(
        service::models::primary_model(dir.path(), &saved.connection),
        "MiniMax-M3.1-Flash",
        "跟着厂商走时默认模型是厂商列表的第一个"
    );
}

#[tokio::test]
async fn 第一次获取内置和自定义都一条都不标新() {
    model::catalog::set_global(model::catalog::snapshot());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "deepseek-flash", "display_name": "Flash"},
                {"id": "vendor-builtin", "display_name": "Builtin"}
            ]
        })))
        .expect(2)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    for (id, preset) in [("builtin-fresh", Some("deepseek")), ("custom-fresh", None)] {
        let saved = host
            .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
                id: id.to_owned(),
                name: id.to_owned(),
                model: "deepseek-flash".to_owned(),
                endpoint: server.uri(),
                api_key: TEST_KEY.to_owned(),
                preset: preset.map(str::to_owned),
                ..Default::default()
            }))
            .await;
        assert!(saved.is_ok(), "{id} save: {saved:?}");
        let response = available_models(&host, discover_request(id)).await;
        assert!(response.live, "{id} 厂商列表成功");
        assert!(
            fresh_ids(&response).is_empty(),
            "{id} 第一次获取没有上一次的列表，一条都不标新"
        );
        assert_eq!(
            stored_live(dir.path(), id).0,
            vec!["deepseek-flash", "vendor-builtin"],
            "{id} 存的是厂商这次给的列表"
        );
    }
}

#[tokio::test]
async fn 显式空模型列表跟着厂商走且主模型取可选第一个() {
    model::catalog::set_global(model::catalog::snapshot());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(SequenceResponder::new(vec![
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "zeta"}, {"id": "alpha"}]
            })),
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "omega"}, {"id": "alpha"}]
            })),
        ]))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(2)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "follow".to_owned(),
            name: "Follow".to_owned(),
            model: "ignored".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            preset: Some("deepseek".to_owned()),
            models: Some(Vec::new()),
            ..Default::default()
        }))
        .await
        .expect("save");
    let service::Response::ConnectionSave(saved) = saved else {
        panic!("expected ConnectionSave");
    };
    assert_eq!(
        saved.connection.models,
        Some(Vec::new()),
        "显式空表原样保存"
    );
    assert_eq!(
        service::models::primary_model(dir.path(), &saved.connection),
        "deepseek-flash",
        "还没存过厂商列表时用内置目录的第一个"
    );

    host.dispatch(Call::AvailableModels(discover_request("follow")))
        .await
        .expect("第一次获取");
    let settings = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("model-settings");
    let service::Response::ModelSettings(settings) = settings else {
        panic!("expected ModelSettings");
    };
    let listed = settings.listed.get("follow").expect("listed 有 follow");
    assert_eq!(
        listed
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["zeta", "alpha"],
        "listed 是存下的厂商列表"
    );
    let (ids, fetched_at) = stored_live(dir.path(), "follow");
    assert_eq!(ids, vec!["zeta", "alpha"]);
    assert_eq!(
        listed.fetched_at, fetched_at,
        "listed 的 fetchedAt 与文件一致"
    );

    let verified = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "follow".to_owned(),
        }))
        .await
        .expect("verify");
    let service::Response::ConnectionVerify(verified) = verified else {
        panic!("expected ConnectionVerify");
    };
    assert!(matches!(verified.connection.check, CheckState::Ok { .. }));

    // 厂商列表变了再获取：可选的跟着变，主模型是新列表的第一个。
    host.dispatch(Call::AvailableModels(discover_request("follow")))
        .await
        .expect("第二次获取");
    let verified = host
        .dispatch(Call::ConnectionVerify(ConnectionVerifyRequest {
            id: "follow".to_owned(),
        }))
        .await
        .expect("verify again");
    let service::Response::ConnectionVerify(verified) = verified else {
        panic!("expected ConnectionVerify");
    };
    assert!(matches!(verified.connection.check, CheckState::Ok { .. }));

    let requests = server.received_requests().await.unwrap();
    let models: Vec<String> = requests
        .iter()
        .filter(|request| !request.body.is_empty())
        .map(|request| {
            let body: serde_json::Value =
                serde_json::from_slice(&request.body).expect("verify body");
            body["model"].as_str().expect("model").to_owned()
        })
        .collect();
    assert_eq!(
        models,
        vec!["zeta".to_owned(), "omega".to_owned()],
        "验证请求的模型跟着可选的第一个变"
    );
}

#[tokio::test]
async fn 只读查询不落盘也不推进新基准() {
    model::catalog::set_global(model::catalog::snapshot());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(SequenceResponder::new(vec![
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "deepseek-flash"}, {"id": "vendor-new-1"}]
            })),
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "deepseek-flash"}, {"id": "vendor-new-1"}]
            })),
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "deepseek-flash"}, {"id": "vendor-new-1"}, {"id": "vendor-new-2"}]
            })),
        ]))
        .expect(3)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "peek".to_owned(),
        name: "Peek".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: server.uri(),
        api_key: TEST_KEY.to_owned(),
        preset: Some("deepseek".to_owned()),
        models: Some(Vec::new()),
        ..Default::default()
    }))
    .await
    .expect("save");

    // 打开选择器/设置页会问一次厂商（补价格、窗口、档）：这一次不能落盘，也不建立基准。
    let peeked = available_models(&host, query_request("peek")).await;
    assert!(peeked.live, "厂商列表拿到了");
    assert!(
        fresh_ids(&peeked).is_empty(),
        "第一次查询还没有上一次的列表，一条都不标新"
    );
    assert!(
        !model::catalog::live_path(dir.path(), "peek").exists(),
        "只读查询不存厂商列表"
    );

    // 用户按「获取可用模型」：这一次才存下厂商列表。
    available_models(&host, discover_request("peek")).await;
    let (ids, fetched_at) = stored_live(dir.path(), "peek");
    assert_eq!(ids, vec!["deepseek-flash", "vendor-new-1"]);

    // 厂商多了一个再只看一眼：不落盘，也不推进「新」的基准。
    let peeked = available_models(&host, query_request("peek")).await;
    assert!(
        fresh_of(&peeked, "vendor-new-2"),
        "基准还是上一次存下的列表"
    );
    let (ids, peeked_fetched_at) = stored_live(dir.path(), "peek");
    assert_eq!(
        ids,
        vec!["deepseek-flash", "vendor-new-1"],
        "只读查询不改文件"
    );
    assert_eq!(peeked_fetched_at, fetched_at, "只读查询不刷新时间");
}

#[tokio::test]
async fn 换端点后旧厂商列表作废获取后新表生效() {
    model::catalog::set_global(model::catalog::snapshot());
    let old_vendor = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "old-vendor-model"}]
        })))
        .expect(1)
        .mount(&old_vendor)
        .await;
    let new_vendor = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "new-vendor-model"}]
        })))
        .expect(1)
        .mount(&new_vendor)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "moved".to_owned(),
        name: "Moved".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: old_vendor.uri(),
        api_key: TEST_KEY.to_owned(),
        preset: Some("deepseek".to_owned()),
        models: Some(Vec::new()),
        ..Default::default()
    }))
    .await
    .expect("save");

    // 旧地址的厂商列表存下了：跟厂商走时主模型是它的第一个。
    available_models(&host, discover_request("moved")).await;
    assert_eq!(stored_live(dir.path(), "moved").0, vec!["old-vendor-model"]);
    assert_eq!(
        service::models::primary_model(dir.path(), &connection_of(&host, "moved").await),
        "old-vendor-model"
    );

    // 换 API 地址：旧地址存下的列表立刻作废，小标签和主模型回到内置目录。
    host.dispatch(Call::ConnectionUpdate(ConnectionUpdateRequest {
        id: "moved".to_owned(),
        endpoint: new_vendor.uri(),
        model: "deepseek-flash".to_owned(),
        ..Default::default()
    }))
    .await
    .expect("update");
    assert!(
        !model::catalog::live_path(dir.path(), "moved").exists(),
        "换地址要清掉旧厂商列表"
    );
    assert!(
        listed_ids(&host, "moved").await.is_empty(),
        "设置页不再显示旧表"
    );
    assert_eq!(
        service::models::primary_model(dir.path(), &connection_of(&host, "moved").await),
        "deepseek-flash",
        "回到内置目录的第一个"
    );

    // 在新地址按获取：新表生效，旧表不再出现。
    let fetched = available_models(&host, discover_request("moved")).await;
    assert!(
        fresh_ids(&fetched).is_empty(),
        "换到新地址后第一次获取没有基准，一条都不标新"
    );
    assert_eq!(stored_live(dir.path(), "moved").0, vec!["new-vendor-model"]);
    assert_eq!(listed_ids(&host, "moved").await, vec!["new-vendor-model"]);
    assert_eq!(
        service::models::primary_model(dir.path(), &connection_of(&host, "moved").await),
        "new-vendor-model"
    );
}

/// 页面在编辑器里重填密钥、换 API 地址：保存走 connection-save。旧地址存下的厂商列表
/// 不能留着；小标签和主模型回到这个连接自己的模型，直到在新地址按一次「获取可用模型」。
#[tokio::test]
async fn 重填密钥保存换地址后旧厂商列表作废获取后新表生效() {
    model::catalog::set_global(model::catalog::snapshot());
    let old_vendor = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "old-vendor-model"}]
        })))
        .expect(1)
        .mount(&old_vendor)
        .await;
    let new_vendor = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "new-vendor-model"}]
        })))
        .expect(1)
        .mount(&new_vendor)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let save = |endpoint: &str| ConnectionSaveRequest {
        id: "edited".to_owned(),
        name: "Edited".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: endpoint.to_owned(),
        api_key: TEST_KEY.to_owned(),
        models: Some(Vec::new()),
        ..Default::default()
    };

    host.dispatch(Call::ConnectionSave(save(&old_vendor.uri())))
        .await
        .expect("save");
    available_models(&host, discover_request("edited")).await;
    assert_eq!(
        stored_live(dir.path(), "edited").0,
        vec!["old-vendor-model"]
    );

    // 只重填密钥、地址没换：存下的列表还在，不该顺手清掉。
    host.dispatch(Call::ConnectionSave(save(&old_vendor.uri())))
        .await
        .expect("re-save");
    assert_eq!(
        listed_ids(&host, "edited").await,
        vec!["old-vendor-model"],
        "地址和预设都没变，旧列表要留着"
    );

    // 换地址：旧表当场作废。
    host.dispatch(Call::ConnectionSave(save(&new_vendor.uri())))
        .await
        .expect("save moved");
    assert!(
        !model::catalog::live_path(dir.path(), "edited").exists(),
        "换地址要清掉旧厂商列表"
    );
    assert!(
        listed_ids(&host, "edited").await.is_empty(),
        "设置页不再显示旧表"
    );
    assert_eq!(
        service::models::primary_model(dir.path(), &connection_of(&host, "edited").await),
        "deepseek-flash",
        "回到连接自己的模型"
    );

    // 在新地址按获取：新表生效，旧表不再出现。
    let fetched = available_models(&host, discover_request("edited")).await;
    assert!(fetched.live, "新地址获取成功");
    assert_eq!(
        stored_live(dir.path(), "edited").0,
        vec!["new-vendor-model"]
    );
    assert_eq!(listed_ids(&host, "edited").await, vec!["new-vendor-model"]);
    assert_eq!(
        service::models::primary_model(dir.path(), &connection_of(&host, "edited").await),
        "new-vendor-model"
    );
}

/// 同一个地址只换预设，也算换了厂商身份：旧表作废，小标签和主模型回到新预设的内置目录。
#[tokio::test]
async fn 保存换预设后旧厂商列表作废() {
    model::catalog::set_global(model::catalog::snapshot());
    let vendor = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(SequenceResponder::new(vec![
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "old-vendor-model"}]
            })),
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "preset-vendor-model"}]
            })),
        ]))
        .expect(2)
        .mount(&vendor)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let save = |preset: &str| ConnectionSaveRequest {
        id: "preset-moved".to_owned(),
        name: "Preset moved".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: vendor.uri(),
        api_key: TEST_KEY.to_owned(),
        preset: Some(preset.to_owned()),
        models: Some(Vec::new()),
        ..Default::default()
    };

    host.dispatch(Call::ConnectionSave(save("deepseek")))
        .await
        .expect("save");
    available_models(&host, discover_request("preset-moved")).await;
    assert_eq!(
        service::models::primary_model(dir.path(), &connection_of(&host, "preset-moved").await),
        "old-vendor-model"
    );

    host.dispatch(Call::ConnectionSave(save("minimax")))
        .await
        .expect("save preset");
    assert!(
        !model::catalog::live_path(dir.path(), "preset-moved").exists(),
        "换预设也要清掉旧厂商列表"
    );
    assert!(listed_ids(&host, "preset-moved").await.is_empty());
    assert_eq!(
        service::models::primary_model(dir.path(), &connection_of(&host, "preset-moved").await),
        "MiniMax-M2",
        "回到 minimax 的内置目录（目录原序）"
    );

    let fetched = available_models(&host, discover_request("preset-moved")).await;
    assert!(
        fresh_ids(&fetched).is_empty(),
        "换预设后旧表已删，第一次获取没有基准"
    );
    assert_eq!(
        listed_ids(&host, "preset-moved").await,
        vec!["preset-vendor-model"]
    );
}

/// 删连接时把存下的厂商列表一起删；同一个 id 再加回来，不能复活旧表。
#[tokio::test]
async fn 删除连接清掉旧厂商列表且同id重加不复活() {
    model::catalog::set_global(model::catalog::snapshot());
    let vendor = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "old-vendor-model"}]
        })))
        .expect(1)
        .mount(&vendor)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let save = || ConnectionSaveRequest {
        id: "gone".to_owned(),
        name: "Gone".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: vendor.uri(),
        api_key: TEST_KEY.to_owned(),
        preset: Some("deepseek".to_owned()),
        models: Some(Vec::new()),
        ..Default::default()
    };
    host.dispatch(Call::ConnectionSave(save()))
        .await
        .expect("save");
    available_models(&host, discover_request("gone")).await;
    assert_eq!(stored_live(dir.path(), "gone").0, vec!["old-vendor-model"]);

    let deleted = host
        .dispatch(Call::ConnectionDelete(ConnectionDeleteRequest {
            id: "gone".to_owned(),
        }))
        .await
        .expect("delete");
    let service::Response::ConnectionDelete(deleted) = deleted else {
        panic!("expected ConnectionDelete");
    };
    assert!(deleted.deleted);
    assert!(
        !model::catalog::live_path(dir.path(), "gone").exists(),
        "删连接要一起删掉旧厂商列表"
    );

    // 同一个 id 再加回来：旧表不能复活。
    host.dispatch(Call::ConnectionSave(save()))
        .await
        .expect("re-add");
    assert!(listed_ids(&host, "gone").await.is_empty());
    assert_eq!(
        service::models::primary_model(dir.path(), &connection_of(&host, "gone").await),
        "deepseek-flash"
    );
}

/// 旧页面发 available-models 不带 mode：按「获取」读，行为跟以前一样。
#[test]
fn 旧页面不传mode时按获取处理() {
    let call: Call = serde_json::from_value(serde_json::json!({
        "method": "available-models",
        "id": "deepseek",
    }))
    .expect("旧页面的 available-models 要读得出来");
    let Call::AvailableModels(request) = call else {
        panic!("expected AvailableModels");
    };
    assert_eq!(request.mode, service::AvailableModelsMode::Fetch);
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
        .expect(1)
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
        .dispatch(Call::AvailableModels(discover_request("list-chat-base")))
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
        .expect(4)
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
    // 内存里的当前选择与它一致（status 从内存选择算）。
    let status = host
        .dispatch(Call::Status(service::StatusRequest {}))
        .await
        .expect("status");
    let service::Response::Status(status) = status else {
        panic!("expected Status");
    };
    let stats = status.stats.expect("stats");
    assert_eq!(stats.connection, "deepseek");
    assert_eq!(stats.model, "deepseek-pro", "内存选择跟 lastSelection 一致");
    assert_eq!(stats.effort_wanted, "low");
}

#[tokio::test]
async fn 重开会话从日志恢复模型选择() {
    // T012.7 故事 2：重开会话还是它（工厂收到日志里的选择）。
    fn recording_factory(
        seen: std::sync::Arc<std::sync::Mutex<Option<session::ModelSelection>>>,
        data_dir: std::path::PathBuf,
    ) -> service::ModelFactory {
        std::sync::Arc::new(move |selection: Option<&session::ModelSelection>| {
            *seen.lock().expect("seen") = selection.cloned();
            Ok((
                "deepseek".to_owned(),
                DeepSeekConfig::default(),
                Some(crate::common::test_credential(&data_dir)),
            ))
        })
    }
    let dir = tempdir().unwrap();
    let first_seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let mut first_options = crate::common::options(dir.path());
    first_options.model_factory = Some(recording_factory(
        std::sync::Arc::clone(&first_seen),
        dir.path().to_path_buf(),
    ));
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
    let mut second_options = crate::common::options(dir.path());
    second_options.model_factory = Some(recording_factory(
        std::sync::Arc::clone(&second_seen),
        dir.path().to_path_buf(),
    ));
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
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(USAGE_SSE),
        )
        .expect(1)
        .mount(&server_a)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
        )
        .expect(1)
        .mount(&server_b)
        .await;
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
    let test_data = dir.path().to_path_buf();
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
            let (endpoint, model, context_window) = if connection == "minimax-cn" {
                (uri_b.clone(), "MiniMax-M3.1-Flash", 222_000)
            } else {
                (uri_a.clone(), "deepseek-flash", 111_000)
            };
            Ok((
                connection,
                DeepSeekConfig {
                    provider: "deepseek".to_owned(),
                    base_url: endpoint,
                    model: model.to_owned(),
                    context_window,
                    ..DeepSeekConfig::default()
                },
                Some(crate::common::test_credential(&test_data)),
            ))
        });
    let mut options = crate::common::options(dir.path());
    options.model_factory = Some(factory);
    let hook_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook_calls_for_hook = std::sync::Arc::clone(&hook_calls);
    options.keyring_changed = Some(std::sync::Arc::new(
        move |_keyring: &credentials::Keyring| {
            hook_calls_for_hook.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    ));
    seed_fx_cache(dir.path());
    let host = RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await;
    assert_eq!(
        *seen.lock().expect("seen"),
        vec![None],
        "新会话 factory 收到 None"
    );
    assert_eq!(
        hook_calls.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "打开根会话不改 Keyring，不触发脱敏钩子"
    );
    // 先跑一轮：走连接 A（初始选择）。
    host.dispatch(Call::Send(service::SendRequest {
        request_id: "r1".to_owned(),
        content: "hi".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("send A");
    wait_for_requests(&server_a, 1).await;
    wait_host_idle(&host).await;
    wait_context_used(&host, 9).await;
    assert_eq!(
        stats_of(&host).await.context_window,
        111_000,
        "初始连接 A 的窗口"
    );
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
    // 热切换只换窗口：已用上下文还是同一会话日志里的事实；Keyring 没动过，不能多触发钩子。
    let stats = stats_of(&host).await;
    assert_eq!(stats.context_window, 222_000, "热切换后窗口跟新模型");
    assert_eq!(stats.context_used, 9, "热切换保留已用上下文，不归零");
    assert_eq!(
        hook_calls.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "Keyring 没变，热切换不触发脱敏钩子"
    );
    // 下一轮走连接 B。
    host.dispatch(Call::Send(service::SendRequest {
        request_id: "r2".to_owned(),
        content: "hi again".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("send B");
    wait_for_requests(&server_b, 1).await;
    wait_host_idle(&host).await;
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

/// 同一连接内换模型：窗口跟新模型走，下一轮请求用新模型（不先撞溢出再压缩）。
#[tokio::test]
async fn 同连接换模型窗口跟新模型() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(USAGE_SSE),
        )
        .expect(2)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let identity = dir.path().join("identity.txt");
    let keyring = credentials::Keyring::open(dir.path(), Some(&identity)).unwrap();
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "model-a".to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            preset: Some("deepseek".to_owned()),
            models: Some(vec![
                model_row("model-a", 1_000_000),
                model_row("model-b", 204_800),
            ]),
            ..credentials::NewConnection::default()
        })
        .expect("save connection");
    let data = dir.path().to_path_buf();
    let uri = server.uri();
    let factory: service::ModelFactory =
        std::sync::Arc::new(move |selection: Option<&session::ModelSelection>| {
            Ok((
                "deepseek".to_owned(),
                DeepSeekConfig {
                    base_url: uri.clone(),
                    model: selection
                        .map(|selection| selection.model.clone())
                        .unwrap_or_else(|| "model-a".to_owned()),
                    context_window: DEFAULT_CONTEXT_WINDOW,
                    ..DeepSeekConfig::default()
                },
                Some(crate::common::test_credential(&data)),
            ))
        });
    let mut options = crate::common::options(dir.path());
    options.model.base_url = server.uri();
    options.credentials = None;
    options.identity = Some(identity);
    options.model_factory = Some(factory);
    let host = RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await;
    assert_eq!(
        stats_of(&host).await.context_window,
        1_000_000,
        "初始模型 model-a 的窗口"
    );

    host.dispatch(Call::Send(service::SendRequest {
        request_id: "window-r1".to_owned(),
        content: "第一轮".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("send 第一轮");
    wait_for_requests(&server, 1).await;
    wait_host_idle(&host).await;

    let selected = host
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            connection: Some("deepseek".to_owned()),
            model: Some("model-b".to_owned()),
            effort_wanted: None,
        }))
        .await
        .expect("换到 model-b");
    let service::Response::SessionModelSelect(selected) = selected else {
        panic!("expected SessionModelSelect");
    };
    assert_eq!(selected.model, "model-b");
    assert_eq!(
        stats_of(&host).await.context_window,
        204_800,
        "同一连接换模型后窗口跟着新模型"
    );

    host.dispatch(Call::Send(service::SendRequest {
        request_id: "window-r2".to_owned(),
        content: "第二轮".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("send 第二轮");
    wait_for_requests(&server, 2).await;
    wait_host_idle(&host).await;
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&requests[1].body).expect("第二轮请求 JSON");
    assert_eq!(body["model"], "model-b", "下一轮请求用新模型");
}

fn model_row(id: &str, context_window: u64) -> credentials::ModelRow {
    credentials::ModelRow {
        id: id.to_owned(),
        name: None,
        context_window: Some(context_window),
        max_tokens: None,
        price_input: None,
        price_output: None,
        price_cache_read: None,
        price_cache_write: None,
    }
}

/// 轮询 mock 收到至少 n 个请求（D57：不拿固定 sleep 等结果）。
async fn wait_for_requests(server: &MockServer, n: usize) {
    for _ in 0..600 {
        let received = server
            .received_requests()
            .await
            .map(|requests| requests.len())
            .unwrap_or(0);
        if received >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等待 mock 收到 {n} 个请求超时");
}

/// 轮询根会话空闲（一轮结束）。
async fn wait_host_idle(host: &RootHost) {
    for _ in 0..600 {
        let response = host
            .dispatch(Call::Status(service::StatusRequest {}))
            .await
            .expect("status");
        let service::Response::Status(status) = response else {
            panic!("expected Status");
        };
        if !status.running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等待根会话空闲超时");
}

/// 轮询上下文占用变成期望值（usage 事件由转发任务异步落进 context）。
async fn wait_context_used(host: &RootHost, used: u64) {
    for _ in 0..600 {
        if stats_of(host).await.context_used == used {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等待 context used 变成 {used} 超时");
}

async fn stats_of(host: &RootHost) -> service::stats::SessionStats {
    let response = host
        .dispatch(Call::Status(service::StatusRequest {}))
        .await
        .expect("status");
    let service::Response::Status(status) = response else {
        panic!("expected Status");
    };
    status.stats.expect("stats")
}

pub(crate) fn seed_fx_cache(dir: &std::path::Path) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    std::fs::write(
        dir.join(service::fx::FX_FILE),
        format!("{{\"rate\":7.11,\"fetchedAt\":{now}}}"),
    )
    .expect("fx cache");
}

/// T012.8a：status 带会话统计（轮/步/token/上下文/花费字段）、汇率与币种。
#[tokio::test]
async fn 状态带统计汇率与币种() {
    let dir = tempdir().unwrap();
    seed_fx_cache(dir.path());
    let host = host(dir.path()).await;
    let response = host
        .dispatch(Call::Status(service::StatusRequest {}))
        .await
        .expect("status");
    let service::Response::Status(status) = response else {
        panic!("expected Status");
    };
    let stats = status.stats.expect("根宿主填统计");
    assert_eq!(stats.turns, 0, "空会话没有轮");
    assert_eq!(stats.steps, 0, "空会话没有步");
    assert_eq!(stats.connection, "deepseek", "统计带当前连接");
    assert!(stats.cost_usd.is_none(), "一个价格都没有就不显示花费");
    assert!(!stats.unpriced, "没有用量时不算未定价");
    let fx = status.fx.expect("汇率");
    assert_eq!(fx.rate, 7.11, "12h 内用缓存");
    assert_eq!(status.currency.as_deref(), Some("usd"), "默认币种 usd");
}

/// T012.8b：余额/余量按厂商形状查询（DeepSeek /user/balance、MiniMax /v1/token_plan/remains）；
/// 自定义地址不查（字段为空）、查不到不报错。
#[tokio::test]
async fn 余额和余量按厂商形状查询且查不到不报错() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/balance"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "is_available": true,
            "balance_infos": [{"currency": "CNY", "total_balance": "110.00"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/token_plan/remains"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model_remains": [{
                "model_name": "general",
                "current_interval_remaining_percent": 99,
                "current_weekly_remaining_percent": 95
            }],
            "base_resp": {"status_code": 0}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let save = |id: &str, preset: &str| ConnectionSaveRequest {
        id: id.to_owned(),
        name: id.to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: server.uri(),
        api_key: TEST_KEY.to_owned(),
        preset: Some(preset.to_owned()),
        ..Default::default()
    };
    // DeepSeek：余额（厂商币种）。
    host.dispatch(Call::ConnectionSave(save("deepseek", "deepseek")))
        .await
        .expect("save deepseek");
    let response = host
        .dispatch(Call::ConnectionQuota(ConnectionQuotaRequest {
            id: "deepseek".to_owned(),
        }))
        .await
        .expect("quota deepseek");
    let service::Response::ConnectionQuota(response) = response else {
        panic!("expected ConnectionQuota");
    };
    let balance = response.balance.clone().expect("余额");
    assert_eq!(balance.currency, "CNY");
    assert!((balance.amount - 110.0).abs() < 1e-9);
    assert!(response.plan.is_none());
    assert!(response.checked_at > 0, "带查询时间");
    no_key(&serde_json::to_string(&response).unwrap());
    // MiniMax：套餐余量（5 小时窗口/每周剩余百分比）。
    host.dispatch(Call::ConnectionSave(save("minimax-cn", "minimax-cn")))
        .await
        .expect("save minimax");
    let response = host
        .dispatch(Call::ConnectionQuota(ConnectionQuotaRequest {
            id: "minimax-cn".to_owned(),
        }))
        .await
        .expect("quota minimax");
    let service::Response::ConnectionQuota(response) = response else {
        panic!("expected ConnectionQuota");
    };
    let plan = response.plan.expect("余量");
    assert_eq!(plan.interval_remaining_percent, Some(99.0));
    assert_eq!(plan.weekly_remaining_percent, Some(95.0));
    assert!(response.balance.is_none());
    // 自定义地址：不查（字段为空，不报错）。
    host.dispatch(Call::ConnectionSave(save("custom", "custom")))
        .await
        .expect("save custom");
    let response = host
        .dispatch(Call::ConnectionQuota(ConnectionQuotaRequest {
            id: "custom".to_owned(),
        }))
        .await
        .expect("quota custom");
    let service::Response::ConnectionQuota(response) = response else {
        panic!("expected ConnectionQuota");
    };
    assert!(response.balance.is_none() && response.plan.is_none());
}

/// T012.8 审计：打开会话时把初始模型选择写成 durable 事实（session/model），
/// 重开时第一条 session/model 就是真正初始选择（stats 定价用）。
#[tokio::test]
async fn 打开会话写初始模型选择事件() {
    let dir = tempdir().unwrap();
    let _host = host(dir.path()).await;
    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    let selection = session::last_model_selection(snapshot.events())
        .expect("初始选择要写成 session/model 事件");
    assert_eq!(selection.connection, "deepseek");
    assert_eq!(selection.model, "deepseek-flash");
}

/// T012.9：写非密钥设置（currency）——settings.json 生效，status 也带新币种。
#[tokio::test]
async fn 设置更新写币种且状态里带新币种() {
    let dir = tempdir().unwrap();
    seed_fx_cache(dir.path());
    let host = host(dir.path()).await;
    let response = host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            currency: Some("cny".to_owned()),
        }))
        .await
        .expect("settings-update 应成功");
    let service::Response::SettingsUpdate(response) = response else {
        panic!("expected SettingsUpdate");
    };
    assert_eq!(response.currency, "cny");
    assert_eq!(
        service::settings::load(dir.path()).currency,
        "cny",
        "settings.json 里是 cny"
    );
    let status = host
        .dispatch(Call::Status(service::StatusRequest {}))
        .await
        .expect("status");
    let service::Response::Status(status) = status else {
        panic!("expected Status");
    };
    assert_eq!(status.currency.as_deref(), Some("cny"), "status 带新币种");
    // 非法币种：拒绝且不改 settings。
    let rejected = host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            currency: Some("eur".to_owned()),
        }))
        .await;
    assert!(rejected.is_err(), "只接受 usd/cny");
    assert_eq!(service::settings::load(dir.path()).currency, "cny");
}

/// 编辑连接（地址/模型/价格）不动 settings.json 里的上次选择。
#[tokio::test]
async fn 编辑连接不改上次选择() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "deepseek".to_owned(),
        name: "DeepSeek".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: "http://127.0.0.1:1".to_owned(),
        api_key: TEST_KEY.to_owned(),
        preset: Some("deepseek".to_owned()),
        ..ConnectionSaveRequest::default()
    }))
    .await
    .expect("save");
    host.dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
        connection: Some("deepseek".to_owned()),
        model: Some("deepseek-pro".to_owned()),
        effort_wanted: Some("low".to_owned()),
    }))
    .await
    .expect("select");
    let before = std::fs::read(service::settings::path(dir.path())).expect("settings.json");
    host.dispatch(Call::ConnectionUpdate(ConnectionUpdateRequest {
        id: "deepseek".to_owned(),
        endpoint: "http://127.0.0.1:2".to_owned(),
        model: "deepseek-v4-pro".to_owned(),
        preset: Some("deepseek".to_owned()),
        models: None,
        auth_header: None,
    }))
    .await
    .expect("connection-update");
    let after = std::fs::read(service::settings::path(dir.path())).expect("settings.json");
    assert_eq!(before, after, "编辑连接不能改上次选择");
}

/// 选模型时 settings.json 写不进去：报 settings/unavailable，内存选择和会话日志都不改。
#[tokio::test]
async fn 选模型写上次选择失败时报设置不可用且不改内存与日志() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    // 先正常选一次，让 settings.json 和日志里都已有上一次选择。
    host.dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
        connection: Some("deepseek".to_owned()),
        model: Some("deepseek-pro".to_owned()),
        effort_wanted: Some("low".to_owned()),
    }))
    .await
    .expect("第一次选择");
    let id = root_session_id(dir.path());
    let before = {
        let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
        session::last_model_selection(snapshot.events()).expect("session/model")
    };
    let before_settings =
        std::fs::read(service::settings::path(dir.path())).expect("settings.json");
    // 原子的临时文件位置被目录占住，rename 前的写入一定失败。
    std::fs::create_dir(dir.path().join("settings.json.tmp")).expect("tmp dir");
    let result = host
        .dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
            connection: Some("deepseek".to_owned()),
            model: Some("deepseek-flash".to_owned()),
            effort_wanted: Some("high".to_owned()),
        }))
        .await;
    let Err(error) = result else {
        panic!("写上次选择失败不能报成功");
    };
    assert_eq!(error.code().as_str(), "settings/unavailable");
    assert!(matches!(error, ServiceError::SettingsInvalid(_)));
    assert_eq!(
        std::fs::read(service::settings::path(dir.path())).expect("settings.json"),
        before_settings,
        "settings.json 字节不变"
    );
    let after = {
        let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
        session::last_model_selection(snapshot.events()).expect("session/model")
    };
    assert_eq!(after, before, "日志里的选择不变");
    let status = host
        .dispatch(Call::Status(service::StatusRequest {}))
        .await
        .expect("status");
    let service::Response::Status(status) = status else {
        panic!("expected Status");
    };
    let stats = status.stats.expect("stats");
    assert_eq!(stats.model, before.model, "内存里的选择不变");
    assert_eq!(stats.effort_wanted, before.effort_wanted);
}

/// 可用模型响应的字段是 camelCase，目录里的四类价格都在。
#[tokio::test]
async fn 可用模型响应字段是驼峰命名且四类价格都在() {
    model::catalog::set_global(model::catalog::snapshot());
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "minimax".to_owned(),
        name: "MiniMax".to_owned(),
        model: "MiniMax-M2.7".to_owned(),
        endpoint: "http://127.0.0.1:1/anthropic".to_owned(),
        api_key: TEST_KEY.to_owned(),
        preset: Some("minimax".to_owned()),
        ..ConnectionSaveRequest::default()
    }))
    .await
    .expect("save");
    let response = host
        .dispatch(Call::AvailableModels(query_request("minimax")))
        .await
        .expect("available models");
    let service::Response::AvailableModels(response) = response else {
        panic!("expected AvailableModels");
    };
    let row = response
        .models
        .iter()
        .find(|row| row.model.id == "MiniMax-M2.7")
        .expect("MiniMax-M2.7");
    assert_eq!(row.model.context_window, Some(204_800));
    assert_eq!(row.model.max_tokens, Some(131_072));
    assert_eq!(row.model.price_input, Some(0.3));
    assert_eq!(row.model.price_output, Some(1.2));
    assert_eq!(row.model.price_cache_read, Some(0.06));
    assert_eq!(row.model.price_cache_write, Some(0.375));
    let json = serde_json::to_value(&response).expect("json");
    let entry = json["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|model| model["id"] == "MiniMax-M2.7")
        .expect("JSON 里的 MiniMax-M2.7");
    for key in [
        "contextWindow",
        "maxTokens",
        "priceInput",
        "priceOutput",
        "priceCacheRead",
        "priceCacheWrite",
        "fresh",
    ] {
        assert!(entry.get(key).is_some(), "缺 {key}: {entry}");
    }
    assert_eq!(entry["fresh"], false, "没取到厂商列表时谁都不标新");
    for key in [
        "context_window",
        "max_tokens",
        "price_input",
        "price_output",
        "price_cache_read",
        "price_cache_write",
    ] {
        assert!(entry.get(key).is_none(), "旧字段 {key} 还在: {entry}");
    }
}

/// 按连接身份发一轮消息，返回会话日志里 assistant/message 的 provider。
async fn assistant_message_provider(connection_id: &str, preset: &str, model_id: &str) -> String {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(USAGE_SSE),
        )
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let identity = dir.path().join("identity.txt");
    credentials::Keyring::open(dir.path(), Some(&identity))
        .expect("keyring")
        .save_connection(credentials::NewConnection {
            id: connection_id.to_owned(),
            name: connection_id.to_owned(),
            model: model_id.to_owned(),
            endpoint: server.uri(),
            api_key: TEST_KEY.to_owned(),
            preset: Some(preset.to_owned()),
            ..credentials::NewConnection::default()
        })
        .expect("save connection");
    let data = dir.path().to_path_buf();
    let identity_for_factory = identity.clone();
    // 和 cli 的 model_with_keyring 一样：连接的厂商 id 进模型配置。
    let factory: service::ModelFactory =
        std::sync::Arc::new(move |selection: Option<&session::ModelSelection>| {
            let keyring = credentials::Keyring::open(data.clone(), Some(&identity_for_factory))
                .map_err(|error| error.to_string())?;
            let view = keyring.view();
            let wanted = selection
                .map(|selection| selection.connection.clone())
                .or_else(|| {
                    view.connections
                        .first()
                        .map(|connection| connection.id.clone())
                })
                .ok_or_else(|| "还没有接模型".to_owned())?;
            let connection = view
                .connections
                .into_iter()
                .find(|connection| connection.id == wanted)
                .ok_or_else(|| "连接不在".to_owned())?;
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
                    context_window: 111_000,
                    effort: Effort::new(effort),
                    tool_update: None,
                },
                Some(crate::common::test_credential(&data)),
            ))
        });
    let mut options = crate::common::options(dir.path());
    options.model_factory = Some(factory);
    let host = RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await;
    host.dispatch(Call::Send(service::SendRequest {
        request_id: "r1".to_owned(),
        content: "hi".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("send");
    wait_for_requests(&server, 1).await;
    wait_host_idle(&host).await;
    let id = root_session_id(dir.path());
    let snapshot = session::SessionStore::open_read_only(dir.path(), &id).expect("snapshot");
    snapshot
        .events()
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            session::EventKind::AssistantMessage { message, .. } => match &message.source {
                model::MessageSource::Model { provider, .. } => Some(provider.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("assistant/message")
}

/// MiniMax 连接的助手消息按连接身份记厂商，不记成 deepseek。
#[tokio::test]
async fn minimax连接的助手消息记真实厂商() {
    assert_eq!(
        assistant_message_provider("minimax", "minimax", "MiniMax-M3.1-Flash").await,
        "minimax"
    );
}

/// DeepSeek 连接仍记 deepseek（厂商身份来自连接，不是写死的）。
#[tokio::test]
async fn deepseek连接的助手消息仍记deepseek() {
    assert_eq!(
        assistant_message_provider("deepseek", "deepseek", "deepseek-flash").await,
        "deepseek"
    );
}
