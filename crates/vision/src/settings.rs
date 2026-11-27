//! settings.json 里的看图设置（只新增 optional 字段；旧文件按空链/关读）。

use serde::{Deserialize, Serialize};

/// 设置里的一个看图后端：连接 id + 模型 id，顺序即尝试顺序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionModelRef {
    pub connection: String,
    pub model: String,
}

/// 看图设置；空链表合法，screen_capture 默认关。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionSettings {
    #[serde(default)]
    pub models: Vec<VisionModelRef>,
    #[serde(default, rename = "screenCapture")]
    pub screen_capture: bool,
}
