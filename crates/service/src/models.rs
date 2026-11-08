//! 连接的可选模型：显式勾的模型行；显式空表（跟着厂商走）时用存下的 live 列表，
//! 还没存过就用内置目录。可选的第一个就是连接的默认模型（新会话、验证、余额查询用）。
//!
//! 页面小标签/选择器从 `model-settings` 的 `listed` 和内置目录自己映射（页面不读磁盘）。

use std::path::Path;

use credentials::ConnectionInfo;
use model::catalog::ModelInfo;

/// 连接现在可选的模型：显式勾的，或者跟厂商走时的存下列表/内置目录。
pub fn selectable_models(data_dir: &Path, connection: &ConnectionInfo) -> Vec<ModelInfo> {
    if let Some(rows) = connection.models.as_deref()
        && !rows.is_empty()
    {
        return rows.iter().map(info_from_row).collect();
    }
    if let Some(live) = model::catalog::read_live(data_dir, &connection.id)
        && !live.models.is_empty()
    {
        return live.models;
    }
    let provider =
        model::catalog::provider_id_for_connection(&connection.preset, &connection.endpoint);
    if provider == "custom" {
        return Vec::new();
    }
    model::catalog::global()
        .unwrap_or_else(model::catalog::snapshot)
        .models_for(&provider)
}

/// 连接现在的默认模型：显式勾的第一个，或者跟厂商走时可选的第一个。
pub fn primary_model(data_dir: &Path, connection: &ConnectionInfo) -> String {
    selectable_models(data_dir, connection)
        .first()
        .map(|model| model.id.clone())
        .unwrap_or_else(|| connection.primary_model().to_owned())
}

fn info_from_row(row: &credentials::ModelRow) -> ModelInfo {
    ModelInfo {
        id: row.id.clone(),
        name: row.name.clone().unwrap_or_else(|| row.id.clone()),
        context_window: row.context_window,
        max_tokens: row.max_tokens,
        price_input: row.price_input,
        price_output: row.price_output.or(row.price_input),
        price_cache_read: row.price_cache_read,
        price_cache_write: row.price_cache_write,
        thinks: false,
        efforts: Vec::new(),
    }
}
