//! 工具用的会话上下文：文件观察状态、任务注册表、spill 存储。
//!
//! T005.0 只定形状；读写能力由 T005.1（观察）、T005.2（spill）、T005.4（任务）填。
//! 这些状态挂在一次会话上：Agent::create/resume 各建一份，续接不继承上一次的观察。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use session::TodoItem;

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

pub use crate::fs::observation::{ObservationPolicy, ObservedFile, WriteIntent};

pub use crate::jobs::TaskRegistry;

/// spill 存储句柄：根目录固定为 data-dir/spill/session-<id>；None 表示禁用。
#[derive(Debug, Clone)]
pub struct SpillStore {
    root: Option<PathBuf>,
    session_id: Option<String>,
}

impl SpillStore {
    /// Agent 建会话时用：root = <data-dir>/spill；文件落在 root/session-<hash>/（照 DSH spill-local）。
    pub fn new(root: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        Self {
            root: Some(root.into()),
            session_id: Some(session_id.into()),
        }
    }

    /// 单元测试/纯工具场景：没有会话数据目录，spill 不可用。
    pub fn disabled() -> Self {
        Self {
            root: None,
            session_id: None,
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// 本会话的 spill 子目录（DSH spill-local sessionDir：root/session-<hash>）。
    pub fn session_dir(&self) -> Option<PathBuf> {
        let root = self.root.as_deref()?;
        let session_id = self.session_id.as_deref()?;
        Some(root.join(crate::spill::session_dir_name(session_id)))
    }
}

/// 一次会话共享的工具上下文状态；ToolContext::new 的测试路径用 detached 空状态。
#[derive(Debug)]
pub struct ToolSessionState {
    observations: Mutex<ObservationPolicy>,
    tasks: TaskRegistry,
    spill: SpillStore,
    /// 会话 id；受管环境变量 AIOS_SESSION_ID 用（T005.3）。
    session_id: Option<String>,
    /// 会话 data-dir；受管环境变量 AIOS_HOME 用（T005.3）。
    data_dir: Option<PathBuf>,
    /// 配置的 skill 根（AgentConfig::skill_roots；T005.6）。
    skill_roots: Mutex<Vec<PathBuf>>,
    /// 工具刚写入的 todo 列表快照（T005.7；driver 追加 todo/write 事件后取走）。
    todos: Mutex<Option<Vec<TodoItem>>>,
}

impl ToolSessionState {
    /// Agent 建会话时用：spill 根固定为 data-dir 下按会话分的目录。
    pub fn new(data_dir: &Path, session_id: &str) -> Self {
        Self {
            observations: Mutex::new(ObservationPolicy::default()),
            tasks: TaskRegistry::new(data_dir, session_id),
            spill: SpillStore::new(data_dir.join("spill"), session_id),
            session_id: Some(session_id.to_owned()),
            data_dir: Some(data_dir.to_path_buf()),
            skill_roots: Mutex::new(Vec::new()),
            todos: Mutex::new(None),
        }
    }

    /// 与真实会话无关的工具测试：空观察表、空任务表、禁用 spill。
    pub fn detached() -> Self {
        Self {
            observations: Mutex::new(ObservationPolicy::default()),
            tasks: TaskRegistry::detached(),
            spill: SpillStore::disabled(),
            session_id: None,
            data_dir: None,
            skill_roots: Mutex::new(Vec::new()),
            todos: Mutex::new(None),
        }
    }

    pub fn observations(&self) -> MutexGuard<'_, ObservationPolicy> {
        self.observations.lock().expect("工具观察状态锁被毒化")
    }

    pub fn tasks(&self) -> &TaskRegistry {
        &self.tasks
    }

    pub fn spill(&self) -> &SpillStore {
        &self.spill
    }

    /// todo_write 工具写入最新整表快照（T005.7）。
    pub fn set_todos(&self, todos: Vec<TodoItem>) {
        *self.todos.lock().expect("todo 状态锁被毒化") = Some(todos);
    }

    /// driver 取走待落日志的 todo 快照（成功追加事件后调用）。
    pub fn take_todos(&self) -> Option<Vec<TodoItem>> {
        self.todos.lock().expect("todo 状态锁被毒化").take()
    }

    /// 当前 todo 列表（续接恢复/测试用）。
    pub fn todos(&self) -> Option<Vec<TodoItem>> {
        self.todos.lock().expect("todo 状态锁被毒化").clone()
    }

    /// Agent 建/续接会话时写入配置的 skill 根（T005.6）。
    pub fn set_skill_roots(&self, roots: Vec<PathBuf>) {
        *self.skill_roots.lock().expect("skill 根锁被毒化") = roots;
    }

    /// 配置的 skill 根（skill 工具用）。
    pub(crate) fn skill_roots(&self) -> Vec<PathBuf> {
        self.skill_roots.lock().expect("skill 根锁被毒化").clone()
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
