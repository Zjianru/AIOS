use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Keyring 的错误。错误文本里不带 key 正文。
#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error("Keyring 的文件读写失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("Keyring 的密文解不开：{0}")]
    Crypto(String),
    #[error("Keyring 的钥匙文件不在：{0}")]
    KeyFileMissing(PathBuf),
    #[error("Keyring 的存储格式不对：{0}")]
    Corrupt(String),
    #[error("没有这个连接：{0}")]
    MissingConnection(String),
    #[error("连接的内容不完整：{0}")]
    InvalidConnection(String),
    #[error("Keyring 还没实现（T008.1 骨架）")]
    NotImplemented,
}

/// 一次签发的凭证值。只在本次出站请求里用；Debug 不打印正文。
pub struct IssuedSecret {
    pub connection: String,
    pub secret: String,
}

impl std::fmt::Debug for IssuedSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IssuedSecret")
            .field("connection", &self.connection)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// 新建/覆盖一个连接时传入的内容（key 只在这一步进来）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConnection {
    pub id: String,
    pub name: String,
    pub model: String,
    pub endpoint: String,
    pub api_key: String,
}

/// 连接的描述，不含 key 正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub name: String,
    pub model: String,
    pub endpoint: String,
    /// Unix 秒；界面自己格式化。
    pub saved_at: u64,
    pub check: CheckState,
}

/// 「连接可用」的验证状态；已保存和最近一次验证分开记。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CheckState {
    Never,
    Ok { at: u64 },
    Fail { at: u64, reason: String },
}

/// 验证结果（由调用方在验证后写回注册器）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    Ok { at: u64 },
    Fail { at: u64, reason: String },
}

/// 一条签发记录：只追加，不记正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueRecord {
    pub id: u64,
    pub at: u64,
    pub connection: String,
    pub purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

/// 钥匙文件的位置和当前问题（读不出来时才有 problem）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFileInfo {
    pub path: PathBuf,
    pub since: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// 设置页/CLI 看的视图；没有任何 key 正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyringView {
    pub connections: Vec<ConnectionInfo>,
    pub key_file: KeyFileInfo,
    pub issued_today: u64,
    pub issues: Vec<IssueRecord>,
    /// 第一次使用：还没有任何连接，且钥匙文件能读。
    pub first_use: bool,
}

/// Keyring 本体。一个数据目录一份存储，一把钥匙文件。
pub struct Keyring {
    data_dir: PathBuf,
    identity_path: PathBuf,
    changes: Mutex<Vec<Sender<()>>>,
}

impl Keyring {
    /// 打开（或初始化）数据目录的 Keyring。
    ///
    /// identity_path 为 None 时用数据目录之外的用户配置目录（spec 默认位置）；
    /// 测试和 DoD 都显式传临时目录里的路径。
    pub fn open(
        data_dir: impl Into<PathBuf>,
        identity_path: Option<&Path>,
    ) -> Result<Self, KeyringError> {
        let data_dir = data_dir.into();
        let identity_path = match identity_path {
            Some(path) => path.to_path_buf(),
            None => default_identity_path(&data_dir),
        };
        Ok(Self {
            data_dir,
            identity_path,
            changes: Mutex::new(Vec::new()),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn identity_path(&self) -> &Path {
        &self.identity_path
    }

    /// 存/覆盖一个连接。
    pub fn save_connection(
        &self,
        connection: NewConnection,
    ) -> Result<ConnectionInfo, KeyringError> {
        Ok(ConnectionInfo {
            id: connection.id,
            name: connection.name,
            model: connection.model,
            endpoint: connection.endpoint,
            saved_at: now_seconds(),
            check: CheckState::Never,
        })
    }

    /// 删一个连接；返回是否真的存在过。
    pub fn delete_connection(&self, id: &str) -> Result<bool, KeyringError> {
        let _ = id;
        Ok(false)
    }

    /// 设置页/CLI 的描述视图；不含任何 key 正文。
    pub fn view(&self) -> KeyringView {
        KeyringView {
            connections: Vec::new(),
            key_file: KeyFileInfo {
                path: self.identity_path.clone(),
                since: now_seconds(),
                problem: Some("Keyring 还没实现（T008.1 骨架）".to_owned()),
            },
            issued_today: 0,
            issues: Vec::new(),
            first_use: true,
        }
    }

    /// 申请一次签发：找到连接、记一条记录、返回这一次要用的值。
    pub fn issue(
        &self,
        connection: &str,
        purpose: &str,
        run: Option<&str>,
    ) -> Result<IssuedSecret, KeyringError> {
        let _ = (connection, purpose, run);
        Err(KeyringError::NotImplemented)
    }

    /// 签发记录（最近的在前）。
    pub fn issues(&self) -> Vec<IssueRecord> {
        Vec::new()
    }

    /// 验证之后写回注册器。
    pub fn record_check(&self, connection: &str, result: CheckResult) -> Result<(), KeyringError> {
        let _ = (connection, result);
        Ok(())
    }

    /// 换钥匙文件的位置：复制到新位置、更新配置、删掉旧位置。
    pub fn relocate_identity(&self, new_path: &Path) -> Result<(), KeyringError> {
        let _ = new_path;
        Ok(())
    }

    /// 指到钥匙文件现在的位置（不复制）。
    pub fn repoint_identity(&self, path: &Path) -> Result<(), KeyringError> {
        let _ = path;
        Ok(())
    }

    /// 重新开始：清空 Keyring、生成新钥匙文件，回到第一次使用。
    pub fn restart(&self) -> Result<(), KeyringError> {
        Ok(())
    }

    /// 存储/钥匙文件变化通知（页面据此刷新）；每个订阅者收到 ()。
    pub fn subscribe(&self) -> Receiver<()> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut changes) = self.changes.lock() {
            changes.push(sender);
        }
        receiver
    }

    #[allow(dead_code)] // 骨架期；实现里每个写操作都会调用。
    fn notify(&self) {
        if let Ok(mut changes) = self.changes.lock() {
            changes.retain(|sender| sender.send(()).is_ok());
        }
    }
}

/// 默认钥匙文件位置：数据目录之外的用户配置目录（测试/DoD 显式传路径）。
fn default_identity_path(data_dir: &Path) -> PathBuf {
    let parent = data_dir.parent().unwrap_or(data_dir);
    parent.join(".aios-keyring").join("identity.txt")
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
