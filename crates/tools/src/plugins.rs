//! 插件开关：编译期装配的能力开关单位；只有官方 Office 一个。
//!
//! 不建加载器、不跑插件进程：PluginSwitches 只是 ToolSet/页面注册表都要查的一份共享开关。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use serde::{Deserialize, Serialize};

/// 官方插件 id（只有 Office）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginId {
    Office,
}

impl PluginId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Office => "office",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "office" => Some(Self::Office),
            _ => None,
        }
    }
}

/// 一份可共享的插件开关集合；默认全关。
#[derive(Debug, Clone, Default)]
pub struct PluginSwitches {
    on: Arc<RwLock<BTreeSet<PluginId>>>,
}

impl PluginSwitches {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(on: impl IntoIterator<Item = PluginId>) -> Self {
        let switches = Self::new();
        switches.set(on);
        switches
    }

    pub fn is_on(&self, plugin: PluginId) -> bool {
        self.on
            .read()
            .map(|on| on.contains(&plugin))
            .unwrap_or(false)
    }

    /// 整表替换（服务端 PluginSet 用）。
    pub fn set(&self, on: impl IntoIterator<Item = PluginId>) {
        if let Ok(mut current) = self.on.write() {
            *current = on.into_iter().collect();
        }
    }

    pub fn enable(&self, plugin: PluginId) {
        if let Ok(mut current) = self.on.write() {
            current.insert(plugin);
        }
    }

    pub fn disable(&self, plugin: PluginId) {
        if let Ok(mut current) = self.on.write() {
            current.remove(&plugin);
        }
    }

    /// 当前开着的插件（排序稳定；协议/页面用）。
    pub fn snapshot(&self) -> Vec<PluginId> {
        self.on
            .read()
            .map(|on| on.iter().copied().collect())
            .unwrap_or_default()
    }
}

/// 同一数据目录的插件开关只有一份：服务、根会话、所有 ThinAgent 共享同一个 Arc。
///
/// 产品里一个进程通常只有一个 data-dir；测试用临时目录隔离，互不影响。
pub fn switches_for(data_dir: &Path) -> PluginSwitches {
    static REGISTRY: LazyLock<Mutex<HashMap<PathBuf, PluginSwitches>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let mut registry = REGISTRY.lock().expect("插件开关表锁被毒化");
    registry.entry(key).or_default().clone()
}
