use std::path::Path;
use std::time::Duration;

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, ConnectionDeleteRequest, ConnectionSaveRequest, ConnectionVerifyRequest, FirstUseRequest,
    IssuesRequest, KeyringRelocateRequest, KeyringReopenRequest, KeyringRestartRequest,
    ModelSettingsRequest, RootHost, RootSessionOptions, SessionModelSelectRequest,
};
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_KEY: &str = "sk-t008-service-0123456789";

fn options(data_dir: &Path, identity: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: data_dir.to_path_buf(),
        model: DeepSeekConfig {
            base_url: "http://127.0.0.1:1".to_owned(),
            api_key: "test-key".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::High,
        },
        config: AgentConfig::default(),
        credentials: None,
        identity: Some(identity.to_path_buf()),
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
            model: "deepseek-pro".to_owned(),
        }))
        .await;
    assert!(response.is_ok(), "session-model-select 应成功：{response:?}");
    let json = serde_json::to_string(&response.unwrap()).unwrap();
    assert!(json.contains("deepseek-pro"), "json={json}");
    no_key(&json);
}
