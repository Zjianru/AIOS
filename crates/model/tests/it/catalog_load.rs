//! T012.5 测试清单 6：目录拉取/缓存/快照三种来源给同样字段；获取可用模型时厂商字段优先。

use std::time::{Duration, SystemTime};

use model::catalog::{
    CatalogSource, LiveModel, cache_path, load_with, merge_live, snapshot, snapshot_json,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "aios-catalog-it-{name}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn flash(snapshot: &model::catalog::CatalogSnapshot) -> model::catalog::ModelInfo {
    snapshot
        .models_for("deepseek")
        .into_iter()
        .find(|model| model.id == "deepseek-flash")
        .expect("deepseek-flash")
}

#[tokio::test]
async fn network_cache_and_snapshot_give_the_same_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/catalog.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(snapshot_json()))
        .mount(&server)
        .await;
    let url = format!("{}/catalog.json", server.uri());
    let dir = temp_dir("network");
    let now = SystemTime::now();

    let (network, source) = load_with(&dir, &url, now).await;
    assert_eq!(source, CatalogSource::Network);
    assert!(cache_path(&dir).exists(), "网络成功要写缓存");
    let expected = flash(&snapshot());
    assert_eq!(flash(&network), expected, "网络与内置快照字段一致");

    let (cached, source) = load_with(&dir, "http://127.0.0.1:1/nope", now).await;
    assert_eq!(source, CatalogSource::Cache);
    assert_eq!(flash(&cached), expected, "缓存与网络字段一致");

    let stale = now + Duration::from_secs(48 * 60 * 60);
    let (fallback, source) = load_with(&dir, "http://127.0.0.1:1/nope", stale).await;
    assert_eq!(source, CatalogSource::Cache);
    assert_eq!(flash(&fallback), expected, "过期缓存+网络失败仍用缓存");

    let empty = temp_dir("snapshot");
    let (snap, source) = load_with(&empty, "http://127.0.0.1:1/nope", now).await;
    assert_eq!(source, CatalogSource::Snapshot);
    assert_eq!(flash(&snap), expected, "内置快照字段与网络/缓存一致");
    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(empty).ok();
}

#[test]
fn live_models_win_where_they_say_something() {
    let catalog = snapshot().models_for("deepseek");
    let merged = merge_live(
        catalog,
        vec![
            LiveModel {
                id: "deepseek-flash".to_owned(),
                display_name: Some("Vendor Name".to_owned()),
                context_window: Some(1234),
                max_tokens: Some(5678),
                tool_update: None,
            },
            LiveModel {
                id: "deepseek-new".to_owned(),
                display_name: Some("New".to_owned()),
                context_window: Some(9000),
                max_tokens: None,
                tool_update: None,
            },
        ],
    );
    let flash = merged
        .iter()
        .find(|model| model.id == "deepseek-flash")
        .expect("flash");
    assert_eq!(flash.name, "Vendor Name");
    assert_eq!(flash.context_window, Some(1234));
    assert_eq!(flash.max_tokens, Some(5678));
    assert!(flash.price_input.is_some(), "目录价格保留");
    assert!(flash.efforts.contains(&"none".to_owned()), "目录档保留");
    let new = merged
        .iter()
        .find(|model| model.id == "deepseek-new")
        .expect("new");
    assert_eq!(new.context_window, Some(9000));
}
