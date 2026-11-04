use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, ModelError};
use service::{
    Call, ConnectionSaveRequest, DeliveryMode, FirstUseRequest, ModelFactory, ModelSettingsRequest,
    RootHost, RootSessionOptions, RootState, SendRequest,
};

const TEST_KEY: &str = "sk-t008-firstuse-0123456789";

struct KeyringSource {
    keyring: Arc<credentials::Keyring>,
    connection: String,
}

impl model::CredentialSource for KeyringSource {
    fn issue(&self, purpose: &str) -> Result<String, ModelError> {
        self.keyring
            .issue(&self.connection, purpose, None)
            .map(credentials::IssuedSecret::into_secret)
            .map_err(|error| {
                ModelError::new(model::ErrorCode::MissingCredential, error.to_string())
            })
    }
}

fn factory(
    data_dir: PathBuf,
    identity: PathBuf,
    seen_model: Arc<Mutex<Option<String>>>,
) -> ModelFactory {
    Arc::new(move || {
        let keyring = credentials::Keyring::open(&data_dir, Some(&identity))
            .map_err(|error| error.to_string())?;
        let view = keyring.view();
        if let Some(problem) = view.key_file.problem {
            return Err(format!("Keyring 读不出来：{problem}"));
        }
        let connection = view
            .connections
            .into_iter()
            .find(|connection| connection.id == "deepseek")
            .ok_or_else(|| "还没有接模型".to_owned())?;
        *seen_model.lock().unwrap() = Some(connection.model.clone());
        let config = DeepSeekConfig {
            base_url: connection.endpoint.clone(),
            api_key: String::new(),
            model: connection.model.clone(),
            ..DeepSeekConfig::default()
        };
        let source = Arc::new(KeyringSource {
            keyring: Arc::new(keyring),
            connection: "deepseek".to_owned(),
        });
        Ok((config, Some(source as Arc<dyn model::CredentialSource>)))
    })
}

fn options(dir: &std::path::Path, seen_model: Arc<Mutex<Option<String>>>) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: dir.to_path_buf(),
        cwd: dir.to_path_buf(),
        model: DeepSeekConfig::default(),
        config: AgentConfig::default(),
        credentials: None,
        identity: Some(dir.join("identity.txt")),
        model_factory: Some(factory(
            dir.to_path_buf(),
            dir.join("identity.txt"),
            seen_model,
        )),
    }
}

#[tokio::test]
async fn 空库起服务存连接后recheck用新模型() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(None));
    let host = RootHost::open(
        options(dir.path(), Arc::clone(&seen)),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;

    // 空库：根会话不可用，但首次使用/设置可读；发送类被拒。
    assert!(
        !matches!(host.info().root, RootState::Ok),
        "空库时根会话应不可用：{:?}",
        host.info().root
    );
    let first = host
        .dispatch(Call::FirstUse(FirstUseRequest {}))
        .await
        .expect("first-use");
    let first_json = serde_json::to_string(&first).unwrap();
    assert!(
        first_json.contains("first_use") || first_json.contains("firstUse"),
        "{first_json}"
    );
    assert!(!first_json.contains(TEST_KEY));
    let send = host
        .dispatch(Call::Send(SendRequest {
            request_id: "r1".to_owned(),
            content: "你好".to_owned(),
            delivery: DeliveryMode::FollowUp,
        }))
        .await;
    assert!(send.is_err(), "空库时发送应被拒：{send:?}");

    // 存连接（真 key 只在这一步进入请求体）。
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-pro".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            api_key: TEST_KEY.to_owned(),
        }))
        .await
        .expect("save connection");
    assert!(!serde_json::to_string(&saved).unwrap().contains(TEST_KEY));
    let settings = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("settings");
    assert!(!serde_json::to_string(&settings).unwrap().contains(TEST_KEY));

    // recheck 用新连接重建 root agent。
    let rechecked = host
        .dispatch(Call::Recheck(Default::default()))
        .await
        .expect("recheck");
    assert!(
        matches!(host.info().root, RootState::Ok),
        "存连接后 recheck 应恢复根会话：{:?}",
        host.info().root
    );
    assert!(serde_json::to_string(&rechecked).unwrap().contains("ok"));
    let status = host.dispatch(Call::Status(Default::default())).await;
    assert!(status.is_ok(), "recheck 后 status 应可用：{status:?}");
    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some("deepseek-pro"),
        "重建时应用新连接的模型"
    );
    assert!(
        host.dispatch(Call::Send(SendRequest {
            request_id: "r2".to_owned(),
            content: "你好".to_owned(),
            delivery: DeliveryMode::FollowUp,
        }))
        .await
        .is_ok()
    );
}
