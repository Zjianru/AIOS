//! 工具用的会话上下文：文件观察状态、任务注册表、spill 存储。
//!
//! T005.0 只定形状；读写能力由 T005.1（观察）、T005.2（spill）、T005.4（任务）填。
//! 这些状态挂在一次会话上：Agent::create/resume 各建一份，续接不继承上一次的观察。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// 文件版本 token（T005.1 从 metadata 生成；这里只保存不透明字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileVersion(String);

impl FileVersion {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 一个文件在本会话里的观察结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedFile {
    /// 读过，且读到这个版本。
    Present(FileVersion),
    /// 读过，确认当时不存在（之后 write 可以走 createIfAbsent）。
    Absent,
}

/// 会话内的文件观察表：canonical target key -> 观察结果。
#[derive(Debug, Default)]
pub struct ObservationStore {
    files: HashMap<PathBuf, ObservedFile>,
}

impl ObservationStore {
    pub fn get(&self, path: impl AsRef<Path>) -> Option<&ObservedFile> {
        self.files.get(path.as_ref())
    }

    pub fn record_present(&mut self, path: impl Into<PathBuf>, version: FileVersion) {
        self.files
            .insert(path.into(), ObservedFile::Present(version));
    }

    pub fn record_absent(&mut self, path: impl Into<PathBuf>) {
        self.files.insert(path.into(), ObservedFile::Absent);
    }

    pub fn clear(&mut self) {
        self.files.clear();
    }

    /// 已观察的文件路径（T005.5 指令刷新用：读到更深目录时补指令）。
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.files.keys()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

pub use crate::jobs::TaskRegistry;

/// spill 存储句柄：根目录固定为 data-dir/spill/session-<id>；None 表示禁用。
#[derive(Debug, Clone)]
pub struct SpillStore {
    root: Option<PathBuf>,
}

impl SpillStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    /// 单元测试/纯工具场景：没有会话数据目录，spill 不可用。
    pub fn disabled() -> Self {
        Self { root: None }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }
}

/// 一次会话共享的工具上下文状态；ToolContext::new 的测试路径用 detached 空状态。
#[derive(Debug)]
pub struct ToolSessionState {
    observations: Mutex<ObservationStore>,
    tasks: TaskRegistry,
    spill: SpillStore,
    /// 会话 id；受管环境变量 AIOS_SESSION_ID 用（T005.3）。
    session_id: Option<String>,
    /// 会话 data-dir；受管环境变量 AIOS_HOME 用（T005.3）。
    data_dir: Option<PathBuf>,
}

impl ToolSessionState {
    /// Agent 建会话时用：spill 根固定为 data-dir 下按会话分的目录。
    pub fn new(data_dir: &Path, session_id: &str) -> Self {
        Self {
            observations: Mutex::new(ObservationStore::default()),
            tasks: TaskRegistry::new(data_dir, session_id),
            spill: SpillStore::new(data_dir.join("spill").join(format!("session-{session_id}"))),
            session_id: Some(session_id.to_owned()),
            data_dir: Some(data_dir.to_path_buf()),
        }
    }

    /// 与真实会话无关的工具测试：空观察表、空任务表、禁用 spill。
    pub fn detached() -> Self {
        Self {
            observations: Mutex::new(ObservationStore::default()),
            tasks: TaskRegistry::detached(),
            spill: SpillStore::disabled(),
            session_id: None,
            data_dir: None,
        }
    }

    pub fn observations(&self) -> MutexGuard<'_, ObservationStore> {
        self.observations.lock().expect("工具观察状态锁被毒化")
    }

    pub fn tasks(&self) -> &TaskRegistry {
        &self.tasks
    }

    pub fn spill(&self) -> &SpillStore {
        &self.spill
    }

    /// 会话 id（bash 受管环境变量用；detached 时 None）。
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// 会话 data-dir（bash 受管环境变量用；detached 时 None）。
    pub(crate) fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }
}

/// 会话级状态的共享句柄；ToolContext 与 Agent 各持一份 Arc。
pub type SharedToolSession = Arc<ToolSessionState>;
