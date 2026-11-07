//! 连接的可选模型：显式勾的模型行；显式空表（跟着厂商走）时用存下的 live 列表，
//! 还没存过就用内置目录。可选的第一个就是连接的默认模型（新会话、验证、余额查询用）。
//!
//! 页面小标签/选择器从 `model-settings` 的 `listed` 和内置目录自己映射（页面不读磁盘）。

use std::path::Path;

use credentials::ConnectionInfo;

/// 连接现在的默认模型：显式勾的第一个，或者跟厂商走时可选的第一个。
pub fn primary_model(data_dir: &Path, connection: &ConnectionInfo) -> String {
    let _ = data_dir;
    connection.primary_model().to_owned()
}
