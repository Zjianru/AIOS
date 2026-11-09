//! T013.9 DoD 发现：空库建连接后 recheck 要把根会话救活。
//!
//! 审计要求：覆盖真实路径（带 model_factory：空库 → unavailable → connection-save → recheck → ok），
//! 而不是只断言 settings.last_selection 被写。

use std::path::Path;
use std::sync::Arc;

use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    Call, ConnectionSaveRequest, RecheckRequest, RootHost, RootSessionOptions, ServiceError,
};
use tempfile::tempdir;

fn deepseek_config() -> DeepSeekConfig {
    DeepSeekConfig {
        provider: "deepseek".to_owned(),
        base_url: "http://127.0.0.1:1".to_owned(),
        model: "deepseek-flash".to_owned(),
        max_tokens: 2048,
        context_window: DEFAULT_CONTEXT_WINDOW,
        effort: Effort::new("high"),
        tool_update: None,
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
        // 和 cli 的 model_with_keyring 一样：会话选择 > settings 上次选择 > deepseek > 第一个连接。
        let last = selection
            .cloned()
            .or_else(|| service::settings::load(&data).last_selection);
        let id = last
            .as_ref()
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
        let Some(connection) = id.and_then(|id| {
            view.connections
                .into_iter()
                .find(|connection| connection.id == id)
        }) else {
            return Err(
                "还没有接模型：先从页面接上，或运行 aios-dev keyring set（key 从标准输入读）"
                    .to_owned(),
            );
        };
        // 同一连接才沿用上次的模型/档；换了连接整组取连接默认（和 resolve_selection 一致）。
        let same = last
            .as_ref()
            .is_some_and(|last| last.connection == connection.id);
        let model = last
            .as_ref()
            .filter(|_| same)
            .map(|last| last.model.clone())
            .unwrap_or_else(|| connection.primary_model().to_owned());
        let effort = last
            .as_ref()
            .filter(|_| same)
            .map(|last| last.effort_wanted.clone())
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
            Some(crate::common::test_credential(&data)),
        ))
    })
}

fn hotfix_options(data_dir: &Path, identity: &Path, cwd: &Path) -> RootSessionOptions {
    let mut options = crate::common::options(data_dir);
    options.cwd = cwd.to_path_buf();
    options.identity = Some(identity.to_path_buf());
    options.model = deepseek_config();
    options.model_factory = Some(factory(data_dir, identity));
    options
}

/// 空库 → root unavailable；connection-save 后再 recheck → root Ok（真实恢复路径）。
#[tokio::test]
async fn connection_save_recovers_root_after_recheck() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let identity = data.path().join("identity.txt");
    let host = RootHost::open(
        hotfix_options(data.path(), &identity, cwd.path()),
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
        hotfix_options(data.path(), &identity, cwd.path()),
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

fn save_request(id: &str, preset: &str, model: &str, endpoint: &str) -> ConnectionSaveRequest {
    ConnectionSaveRequest {
        id: id.to_owned(),
        name: id.to_owned(),
        model: model.to_owned(),
        endpoint: endpoint.to_owned(),
        api_key: "test-key".to_owned(),
        preset: Some(preset.to_owned()),
        models: None,
        auth_header: None,
    }
}

/// 空库新建连接：lastSelection 的档取模型自己的默认档（MiniMax-M3.1-Flash 是 medium，
/// 不是写死的 high）；recheck 后的 current/ready/统计都落在它上面。
#[tokio::test]
async fn connection_save_uses_model_default_effort_and_recheck_reports_it() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let identity = data.path().join("identity.txt");
    let host = RootHost::open(
        hotfix_options(data.path(), &identity, cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
    crate::settings::seed_fx_cache(data.path());
    host.dispatch(Call::ConnectionSave(save_request(
        "minimax",
        "minimax",
        "MiniMax-M3.1-Flash",
        "http://127.0.0.1:1/anthropic",
    )))
    .await
    .expect("connection-save");
    let last = service::settings::load(data.path())
        .last_selection
        .expect("新建连接要成为上次选择");
    assert_eq!(last.connection, "minimax");
    assert_eq!(last.model, "MiniMax-M3.1-Flash");
    assert_eq!(
        last.effort_wanted, "medium",
        "MiniMax-M3.1-Flash 的默认档是 medium"
    );
    let rechecked = host
        .dispatch(Call::Recheck(RecheckRequest {}))
        .await
        .expect("recheck");
    let json = serde_json::to_value(&rechecked).unwrap();
    assert_eq!(json["root"]["state"], "ok", "recheck 要恢复根会话：{json}");
    let status = host
        .dispatch(Call::Status(service::StatusRequest {}))
        .await
        .expect("status");
    let service::Response::Status(status) = status else {
        panic!("expected Status");
    };
    let stats = status.stats.expect("stats");
    assert_eq!(stats.connection, "minimax", "当前连接跟新连接一致");
    assert_eq!(stats.model, "MiniMax-M3.1-Flash", "当前模型跟新连接一致");
    assert_eq!(
        stats.effort_wanted, "medium",
        "内存选择跟 lastSelection 一致"
    );
}

/// 新建连接后只改档：连接/模型沿用刚建的那个（内存 selection 与 lastSelection 是同一份事实）。
#[tokio::test]
async fn new_connection_is_current_selection_for_following_select() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let identity = data.path().join("identity.txt");
    let host = RootHost::open(
        hotfix_options(data.path(), &identity, cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
    host.dispatch(Call::ConnectionSave(save_request(
        "minimax",
        "minimax",
        "MiniMax-M3.1-Flash",
        "http://127.0.0.1:1/anthropic",
    )))
    .await
    .expect("connection-save");
    // 根会话还没接上：选择本身是 durable 偏好，照写；但不能把当前连接退回 deepseek 占位。
    let failed = host
        .dispatch(Call::SessionModelSelect(
            service::SessionModelSelectRequest {
                connection: None,
                model: None,
                effort_wanted: Some("low".to_owned()),
            },
        ))
        .await;
    assert!(
        matches!(failed, Err(ServiceError::RootUnavailable(_))),
        "根会话还没接上：{failed:?}"
    );
    let last = service::settings::load(data.path())
        .last_selection
        .expect("上次选择还在");
    assert_eq!(last.connection, "minimax");
    assert_eq!(last.model, "MiniMax-M3.1-Flash");
    assert_eq!(last.effort_wanted, "low");
}

/// 空库新建连接时 settings.json 写不进去：返回 settings/unavailable，不把选择当已保存。
#[tokio::test]
async fn connection_save_reports_when_selection_cannot_be_saved() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let identity = data.path().join("identity.txt");
    let host = RootHost::open(
        hotfix_options(data.path(), &identity, cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
    // 原子的临时文件位置被目录占住，rename 前的写入一定失败。
    std::fs::create_dir(data.path().join("settings.json.tmp")).expect("tmp dir");
    let mut events = host.subscribe_events();
    let result = host
        .dispatch(Call::ConnectionSave(save_request(
            "minimax",
            "minimax",
            "MiniMax-M3.1-Flash",
            "http://127.0.0.1:1/anthropic",
        )))
        .await;
    // 连接本身已进 Keyring：设置变化事件照发（页面刷新、监控脱敏列表都要知道 key 换了）。
    let event = events.try_recv().expect("连接已保存，要有设置变化事件");
    assert_eq!(event.event_type, "keyring/changed");
    let Err(error) = result else {
        panic!("写上次选择失败不能报成功");
    };
    assert_eq!(error.code().as_str(), "settings/unavailable");
    assert!(matches!(error, ServiceError::SettingsInvalid(_)));
    assert!(
        service::settings::load(data.path())
            .last_selection
            .is_none(),
        "没有把上次选择当成已保存"
    );
}
