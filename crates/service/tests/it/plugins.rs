//! 插件开关协议面（settings › 插件）。

use service::{
    Call, PluginListRequest, PluginListResponse, PluginSetRequest, Response, ServiceError,
};
use tempfile::tempdir;
use tools::{PluginId, plugins};

fn office_of(response: &PluginListResponse) -> &service::PluginView {
    response
        .plugins
        .iter()
        .find(|plugin| plugin.id == "office")
        .expect("列表里要有 office")
}

#[tokio::test]
async fn plugin_list_defaults_off_and_has_office_card() {
    let dir = tempdir().unwrap();
    let host = crate::common::open(crate::common::options(dir.path())).await;
    let response = host
        .dispatch(Call::PluginList(PluginListRequest {}))
        .await
        .unwrap();
    let Response::PluginList(list) = response else {
        panic!("应回 plugin_list");
    };
    let office = office_of(&list);
    assert!(!office.enabled, "默认关");
    assert_eq!(office.name, "Office");
    assert!(office.skill_summary.contains("Office 文件"));
    assert!(office.data_dir.ends_with("plugins/office"));
    assert!(list.content_packs.is_empty());
    assert!(!plugins::switches_for(dir.path()).is_on(PluginId::Office));
}

#[tokio::test]
async fn plugin_set_persists_switches_and_close_appends_one_fixed_notice() {
    let dir = tempdir().unwrap();
    let host = crate::common::open(crate::common::options(dir.path())).await;
    let before = crate::common::read_root_events(dir.path())
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::SystemMessage { .. }))
        .count();
    let opened = host
        .dispatch(Call::PluginSet(PluginSetRequest {
            id: "office".to_owned(),
            enabled: true,
        }))
        .await
        .unwrap();
    let Response::PluginSet(list) = opened else {
        panic!("应回 plugin_set");
    };
    assert!(office_of(&list).enabled);
    assert!(plugins::switches_for(dir.path()).is_on(PluginId::Office));
    assert!(service::settings::load(dir.path()).plugins.office);
    let after_open = crate::common::read_root_events(dir.path());
    let open_notices = after_open
        .iter()
        .filter(|event| {
            let session::EventKind::SystemMessage { message, .. } = &event.kind else {
                return false;
            };
            message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .any(|text| text.starts_with("【插件】Office 已开启"))
        })
        .count();
    assert_eq!(open_notices, 0, "打开不发 SystemMessage");

    let closed = host
        .dispatch(Call::PluginSet(PluginSetRequest {
            id: "office".to_owned(),
            enabled: false,
        }))
        .await
        .unwrap();
    let Response::PluginSet(list) = closed else {
        panic!("应回 plugin_set");
    };
    assert!(!office_of(&list).enabled);
    assert!(!plugins::switches_for(dir.path()).is_on(PluginId::Office));
    assert!(!service::settings::load(dir.path()).plugins.office);
    let after_close = crate::common::read_root_events(dir.path());
    let closed_notices: Vec<_> = after_close
        .iter()
        .filter_map(|event| {
            let session::EventKind::SystemMessage { message, .. } = &event.kind else {
                return None;
            };
            message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .find(|text| text.starts_with("【插件】Office 已关闭"))
        })
        .collect();
    assert_eq!(closed_notices.len(), 1, "关掉只追加一句固定通知");
    assert!(closed_notices[0].contains("已生成的文件和数据保留"));
    assert!(
        after_close
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::SystemMessage { .. }))
            .count()
            == before + 1,
        "除固定通知外没有别的 SystemMessage"
    );
}

#[tokio::test]
async fn plugin_set_rejects_unknown_id() {
    let dir = tempdir().unwrap();
    let host = crate::common::open(crate::common::options(dir.path())).await;
    let error = host
        .dispatch(Call::PluginSet(PluginSetRequest {
            id: "calendar".to_owned(),
            enabled: true,
        }))
        .await
        .unwrap_err();
    assert!(matches!(error, ServiceError::SettingsInvalid(_)));
}

#[test]
fn old_settings_without_plugins_section_still_loads() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"currency":"cny","lastSelection":null}"#,
    )
    .unwrap();
    let settings = service::settings::load(dir.path());
    assert_eq!(settings.currency, "cny");
    assert!(!settings.plugins.office, "旧文件缺 plugins 节按默认关读");
}

#[tokio::test]
async fn plugin_switch_card_and_settings_page_land_same_result() {
    let dir = tempdir().unwrap();
    let host = crate::common::open(crate::common::options(dir.path())).await;
    let registry = service::ActionRegistry::settings(dir.path().to_path_buf(), None);
    let action = registry
        .get("set_plugin")
        .expect("set_plugin 要登记")
        .clone();
    assert_eq!(action.skill(), "aios-settings");
    let events: Vec<session::Event> = Vec::new();
    let env = service::ActionEnv {
        data_dir: dir.path(),
        identity: None,
        events: &events,
        conversation: None,
    };
    let details = action
        .details(&serde_json::json!({ "id": "office", "enabled": true }))
        .expect("打开 Office 的 details");
    let facts = action.describe(&details, &env).expect("读卡事实");
    let text = facts
        .iter()
        .map(|fact| format!("{}：{}", fact.name, fact.value))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    assert!(text.contains("插件：Office"), "{text}");
    assert!(text.contains("操作：打开"), "{text}");
    assert!(text.contains("影响："), "{text}");
    assert!(text.contains("当前：已关闭"), "{text}");

    // 卡点确认：和设置页同一条 PluginSet。
    action.act(&details, &host).await.expect("卡确认打开");
    assert!(service::settings::load(dir.path()).plugins.office);
    assert!(plugins::switches_for(dir.path()).is_on(PluginId::Office));
    let listed = host
        .dispatch(Call::PluginList(PluginListRequest {}))
        .await
        .unwrap();
    let Response::PluginList(list) = listed else {
        panic!("应回 plugin_list");
    };
    assert!(office_of(&list).enabled);

    // 设置页关掉：同一份状态，卡这边也看到关着。
    host.dispatch(Call::PluginSet(PluginSetRequest {
        id: "office".to_owned(),
        enabled: false,
    }))
    .await
    .unwrap();
    assert!(!service::settings::load(dir.path()).plugins.office);
    assert!(!plugins::switches_for(dir.path()).is_on(PluginId::Office));
    // 卡这边重新读事实：页面关掉以后，再提议打开必须看到「当前：已关闭」。
    let reopen_details = action
        .details(&serde_json::json!({ "id": "office", "enabled": true }))
        .unwrap();
    let reopen_facts = action
        .describe(&reopen_details, &env)
        .expect("重新打开卡事实");
    assert!(
        reopen_facts
            .iter()
            .any(|fact| fact.name == "当前" && fact.value == "已关闭"),
        "{reopen_facts:?}"
    );
}
