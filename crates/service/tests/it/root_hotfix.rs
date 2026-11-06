//! T013.9 DoD 发现：空库建连接后 recheck 要把根会话救活（修法 A：connection-save 写 last_selection）。

use std::path::Path;

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{Call, ConnectionSaveRequest, RecheckRequest, RootHost, RootSessionOptions};
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

fn options(data_dir: &Path, identity: &Path, cwd: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        model: DeepSeekConfig {
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::new("high"),
        },
        config: AgentConfig::default(),
        credentials: Some(std::sync::Arc::new(TestCredential)),
        identity: Some(identity.to_path_buf()),
        model_factory: None,
        append_hook_factory: None,
        keyring_changed: None,
    }
}

/// 空库 → connection-save：修法 A 之后写 settings.last_selection（recheck 的 resolve_selection 靠它）。
///
/// 注意：这里的 RootHost 没有 model_factory（测试宿主），open 时 root 已是 Ok；
/// 真正「unavailable → save → recheck 恢复」的路径由 T013.9 的 HTTP 验证覆盖：
///   空库 serve → connection-save → connection-verify → recheck → root: {state:"ok"} → status 200
/// 本测试守住的是最关键的那条事实：connection-save 会把 last_selection 写到新连接上。
#[tokio::test]
async fn connection_save_sets_last_selection_and_recheck_recovers() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = RootHost::open(
        options(data.path(), &data.path().join("identity.txt"), cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
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
        .expect("connection-save");
    let saved_json = serde_json::to_value(&saved).unwrap();
    assert_eq!(saved_json["connection"]["id"], "minimax-cn");
    // 修法 A：保存连接即写 settings.last_selection（RootHost recheck 的 resolve_selection 靠它）。
    let settings = service::settings::load(data.path());
    assert_eq!(
        settings
            .last_selection
            .map(|selection| selection.connection),
        Some("minimax-cn".to_owned()),
        "connection-save 后 last_selection 要落在新连接上"
    );
    // 再检查一次：根会话应变 ok（不再报「还没有接模型」）。
    let rechecked = host
        .dispatch(Call::Recheck(RecheckRequest {}))
        .await
        .expect("recheck");
    let json = serde_json::to_value(&rechecked).unwrap();
    assert_eq!(json["root"]["state"], "ok", "recheck 要恢复根会话：{json}");
}
