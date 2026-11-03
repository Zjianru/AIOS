use std::path::{Path, PathBuf};

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, ConnectionDeleteRequest, ConnectionSaveRequest, FirstUseRequest, IssuesRequest,
    KeyringRelocateRequest, KeyringReopenRequest, KeyringRestartRequest, ModelSettingsRequest,
    Response, RootHost, RootSessionOptions,
};
use tempfile::tempdir;

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
