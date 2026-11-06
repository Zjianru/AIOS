//! T013.9 DoD 发现：空库建连接后 recheck 要把根会话救活。
//!
//! 审计要求：覆盖真实路径（带 model_factory：空库 → unavailable → connection-save → recheck → ok），
//! 而不是只断言 settings.last_selection 被写。

use std::path::Path;
use std::sync::Arc;

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, ConnectionSaveRequest, RecheckRequest, RootHost, RootSessionOptions, ServiceError,
};
use tempfile::tempdir;

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

fn deepseek_config() -> DeepSeekConfig {
    DeepSeekConfig {
        base_url: "http://127.0.0.1:1".to_owned(),
        model: "deepseek-flash".to_owned(),
        max_tokens: 2048,
        context_window: DEFAULT_CONTEXT_WINDOW,
        effort: Effort::new("high"),
    }
}

/// 模拟 cli 的 model_factory：keyring 里没有连接时返回「还没有接模型」，有就返回它。
/// （和 cli 的 model_with_keyring → resolve_selection 同一条件：没有选择且没有连接时解析失败。）
fn factory(data_dir: &Path, identity: &Path) -> service::ModelFactory {
    let data = data_dir.to_path_buf();
    let identity = identity.to_path_buf();
    Arc::new(move |selection: Option<&session::ModelSelection>| {
        let keyring = credentials::Keyring::open(data.clone(), Some(&identity))
            .map_err(|error| error.to_string())?;
        let view = keyring.view();
        if let Some(problem) = view.key_file.problem.clone() {
            return Err(format!("Keyring 读不出来：{problem}"));
        }
        // 修法 A 之后 connection-save 会写 last_selection；空库时都没有 → 报「还没有接模型」。
        let id = selection
            .map(|selection| selection.connection.clone())
            .or_else(|| {
                view.connections
                    .iter()
                    .find(|connection| connection.id == "deepseek")
                    .map(|connection| connection.id.clone())
            })
            .or_else(|| {
                view.connections
                    .first()
                    .map(|connection| connection.id.clone())
            });
        match id.and_then(|id| view.connections.iter().find(|item| item.id == id).cloned()) {
            Some(connection) => Ok((
                connection.id,
                deepseek_config(),
                Some(Arc::new(TestCredential) as Arc<dyn model::CredentialSource>),
            )),
            None => Err(
                "还没有接模型：先从页面接上，或运行 aios-dev keyring set（key 从标准输入读）"
                    .to_owned(),
            ),
        }
    })
}

fn options(data_dir: &Path, identity: &Path, cwd: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        model: deepseek_config(),
        config: AgentConfig::default(),
        credentials: Some(Arc::new(TestCredential)),
        identity: Some(identity.to_path_buf()),
        model_factory: Some(factory(data_dir, identity)),
        append_hook_factory: None,
        keyring_changed: None,
    }
}

/// 空库 → root unavailable；connection-save 后再 recheck → root Ok（真实恢复路径）。
#[tokio::test]
async fn connection_save_recovers_root_after_recheck() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let identity = data.path().join("identity.txt");
    let host = RootHost::open(
        options(data.path(), &identity, cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
    // 空库：root 不可用（factory 报「还没有接模型」）。
    let status = host.dispatch(Call::Status(service::StatusRequest {})).await;
    match status {
        Err(ServiceError::RootUnavailable(service::RootState::Unavailable { message, .. })) => {
            assert!(message.contains("还没有接模型"), "message={message}");
        }
        other => panic!("空库时 Status 应报 root 不可用，实际：{other:?}"),
    }
    // 建连接：修法 A 把它设为当前选择。
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: "minimax-cn".to_owned(),
        name: "MiniMax（国内站）".to_owned(),
        model: "MiniMax-M3.1-Flash".to_owned(),
        endpoint: "http://127.0.0.1:1/anthropic".to_owned(),
        api_key: "test-key".to_owned(),
        preset: Some("minimax-cn".to_owned()),
        models: None,
        price_rate: None,
        auth_header: None,
    }))
    .await
    .expect("connection-save 在 root 不可用时也应可用");
    let settings = service::settings::load(data.path());
    assert_eq!(
        settings
            .last_selection
            .map(|selection| selection.connection),
        Some("minimax-cn".to_owned()),
        "空库 connection-save 后 last_selection 要落在新连接上"
    );
    // 再检查一次：根会话应变 ok（真实恢复路径）。
    let rechecked = host
        .dispatch(Call::Recheck(RecheckRequest {}))
        .await
        .expect("recheck");
    let json = serde_json::to_value(&rechecked).unwrap();
    assert_eq!(json["root"]["state"], "ok", "recheck 要恢复根会话：{json}");
}

/// root 已恢复（Ok）后存第二连接 B：last_selection 仍是 A（审计第二轮：已接上不换）。
#[tokio::test]
async fn connection_save_keeps_existing_selection_after_root_ok() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let identity = data.path().join("identity.txt");
    let host = RootHost::open(
        options(data.path(), &identity, cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
    let save = |id: &str, preset: &str, endpoint: &str| ConnectionSaveRequest {
        id: id.to_owned(),
        name: id.to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: endpoint.to_owned(),
        api_key: "test-key".to_owned(),
        preset: Some(preset.to_owned()),
        models: None,
        price_rate: None,
        auth_header: None,
    };
    // 空库（root unavailable）：存 A → 被设为当前选择。
    host.dispatch(Call::ConnectionSave(save(
        "deepseek",
        "deepseek",
        "http://127.0.0.1:1",
    )))
    .await
    .expect("save A");
    // recheck 把 root 拉起来（A 在，factory 能解析）。
    let rechecked = host
        .dispatch(Call::Recheck(RecheckRequest {}))
        .await
        .expect("recheck");
    let json = serde_json::to_value(&rechecked).unwrap();
    assert_eq!(json["root"]["state"], "ok", "recheck 要恢复：{json}");
    // root Ok 之后存 B：不能换掉当前选择。
    host.dispatch(Call::ConnectionSave(save(
        "minimax-cn",
        "minimax-cn",
        "http://127.0.0.1:1/anthropic",
    )))
    .await
    .expect("save B");
    assert_eq!(
        service::settings::load(data.path())
            .last_selection
            .map(|selection| selection.connection),
        Some("deepseek".to_owned()),
        "root 已 Ok 时存第二连接不该改 last_selection"
    );
}
