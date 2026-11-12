//! 接缝 1：models.dev 目录的 status（已下线）过滤、发布日期排序，以及和厂商列表合并时的过滤。

use model::catalog::{
    CatalogSnapshot, LiveModel, ModelInfo, deserialize_snapshot, merge_live, snapshot_json,
};

fn snapshot() -> CatalogSnapshot {
    deserialize_snapshot(snapshot_json())
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

fn live(id: &str) -> LiveModel {
    LiveModel {
        id: id.to_owned(),
        display_name: None,
        context_window: None,
        max_tokens: None,
        tool_update: None,
    }
}

/// 目录标了 deprecated 的模型不出现在 models_for；其余按 release_date 从新到旧，
/// 同一天按 id。
#[test]
fn models_for_drops_deprecated_and_sorts_newest_first() {
    let snapshot = snapshot();
    assert_eq!(
        ids(&snapshot.models_for("minimax-cn")),
        vec!["MiniMax-M3.1-Flash", "MiniMax-M3"],
    );
    // deepseek-flash 和 deepseek-v4-pro 同日（2026-10-17），按 id 升序；
    // 两个 v4-flash 在 models.dev 标了 deprecated。
    assert_eq!(
        ids(&snapshot.models_for("deepseek")),
        vec!["deepseek-flash", "deepseek-v4-pro"],
    );
}

/// 快照带着 models.dev 的 status：MiniMax 国内站的 M2.x 全部 deprecated，M3/M3.1-Flash 没有。
#[test]
fn snapshot_keeps_models_dev_status() {
    let snapshot = snapshot();
    for id in [
        "MiniMax-M2",
        "MiniMax-M2.1",
        "MiniMax-M2.5",
        "MiniMax-M2.5-highspeed",
        "MiniMax-M2.7",
        "MiniMax-M2.7-highspeed",
    ] {
        assert_eq!(
            snapshot
                .model("minimax-cn", id)
                .and_then(|model| model.status.as_deref()),
            Some("deprecated"),
            "{id} 应标 deprecated",
        );
    }
    for id in ["MiniMax-M3", "MiniMax-M3.1-Flash"] {
        assert_eq!(
            snapshot
                .model("minimax-cn", id)
                .and_then(|model| model.status.as_deref()),
            None,
            "{id} 不应标 deprecated",
        );
    }
}

/// merge_live 去掉目录标了 deprecated 的 live id；目录没有的 id 保留；厂商列表自身的顺序不动。
#[test]
fn merge_live_drops_catalog_deprecated_and_keeps_unknown_ids() {
    let snapshot = snapshot();
    let merged = merge_live(
        &snapshot,
        "minimax-cn",
        vec![
            live("MiniMax-M3"),
            live("MiniMax-M2.7-highspeed"),
            live("MiniMax-M3.1-Flash"),
            live("MiniMax-M2"),
            live("vendor-future"),
        ],
    );
    assert_eq!(
        ids(&merged),
        vec!["MiniMax-M3", "MiniMax-M3.1-Flash", "vendor-future"],
        "厂商顺序保留；目录 deprecated 的去掉；目录没有的保留",
    );
    let m3 = merged
        .iter()
        .find(|model| model.id == "MiniMax-M3")
        .expect("MiniMax-M3");
    assert!(m3.price_input.is_some(), "目录字段照常合并");
    let future = merged
        .iter()
        .find(|model| model.id == "vendor-future")
        .expect("vendor-future");
    assert_eq!(future.price_input, None, "目录没有的模型没有标价");
}
