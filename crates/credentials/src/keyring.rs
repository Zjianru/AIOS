use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use age::secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

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
}

/// 一次签发的凭证值。只在本次出站请求里用；Debug 不打印正文。
pub struct IssuedSecret {
    pub connection: String,
    pub secret: String,
}

impl IssuedSecret {
    /// 显式清零本次签发值；Drop 也会调用。
    pub fn zeroize(&mut self) {
        self.secret.zeroize();
    }

    /// 取出本次签发值（调用方用完后应立即交给模型请求并丢弃）。
    pub fn into_secret(self) -> String {
        let mut this = std::mem::ManuallyDrop::new(self);
        let secret = std::mem::take(&mut this.secret);
        // 余下字段正常 drop（connection 不残留；String 已置空，不泄漏）。
        drop(std::mem::take(&mut this.connection));
        secret
    }
}

#[cfg(test)]
static ISSUED_DROP_CALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

impl Drop for IssuedSecret {
    fn drop(&mut self) {
        self.zeroize();
        #[cfg(test)]
        ISSUED_DROP_CALLED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
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
    /// Keyring 里有没有这把 key（撤销后连接还在，但不能签发）。
    #[serde(default = "default_key_saved")]
    pub key_saved: bool,
}

fn default_key_saved() -> bool {
    true
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
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

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    connections: Vec<StoredConnection>,
    #[serde(default)]
    issues: Vec<IssueRecord>,
    #[serde(default)]
    next_issue: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredConnection {
    info: ConnectionInfo,
    api_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    version: u32,
    identity_path: PathBuf,
    since: u64,
}

/// Keyring 本体。一个数据目录一份存储，一把钥匙文件。
pub struct Keyring {
    data_dir: PathBuf,
    since: u64,
    changes: Mutex<Vec<Sender<()>>>,
}

impl Keyring {
    /// 打开（或初始化）数据目录的 Keyring。
    ///
    /// 已有配置时以配置里的钥匙文件位置为准；没有配置时用传入位置（None 用默认用户配置目录）。
    /// 钥匙文件不在时不在这里失败：描述会报读不出，签发失败（页面有两条出路）。
    pub fn open(
        data_dir: impl Into<PathBuf>,
        identity_path: Option<&Path>,
    ) -> Result<Self, KeyringError> {
        let data_dir = data_dir.into();
        create_private_dir(&data_dir)?;
        let config_path = data_dir.join("config.json");
        let since = if config_path.exists() {
            read_config(&config_path)?.since
        } else {
            let identity_path = match identity_path {
                Some(path) => path.to_path_buf(),
                None => default_identity_path(&data_dir),
            };
            let since = now_seconds();
            if !identity_path.exists() {
                create_identity(&identity_path)?;
            }
            write_config(
                &config_path,
                &Config {
                    version: 1,
                    identity_path,
                    since,
                },
            )?;
            since
        };
        Ok(Self {
            data_dir,
            since,
            changes: Mutex::new(Vec::new()),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// 当前钥匙文件位置：每次从配置重读（进程内多个 Keyring 实例共享同一份配置）。
    pub fn identity_path(&self) -> PathBuf {
        read_config(&self.data_dir.join("config.json"))
            .map(|config| config.identity_path)
            .unwrap_or_else(|_| default_identity_path(&self.data_dir))
    }

    /// 进程内/跨进程文件锁：读改写整段持有；drop 释放。
    fn lock(&self) -> Result<File, KeyringError> {
        fs::create_dir_all(&self.data_dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.data_dir.join("keyring.lock"))?;
        file.lock()?;
        Ok(file)
    }

    /// 存/覆盖一个连接；saved_at 更新，验证状态重置为 Never。
    pub fn save_connection(
        &self,
        connection: NewConnection,
    ) -> Result<ConnectionInfo, KeyringError> {
        if connection.id.trim().is_empty()
            || connection.name.trim().is_empty()
            || connection.model.trim().is_empty()
            || connection.endpoint.trim().is_empty()
        {
            return Err(KeyringError::InvalidConnection(
                "id/name/model/endpoint 不能为空".to_owned(),
            ));
        }
        if connection.api_key.trim().is_empty() {
            return Err(KeyringError::InvalidConnection("key 不能为空".to_owned()));
        }
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let info = ConnectionInfo {
            id: connection.id.clone(),
            name: connection.name,
            model: connection.model,
            endpoint: connection.endpoint,
            saved_at: now_seconds(),
            check: CheckState::Never,
            key_saved: true,
        };
        let stored = StoredConnection {
            info: info.clone(),
            api_key: connection.api_key,
        };
        if let Some(existing) = store
            .connections
            .iter_mut()
            .find(|item| item.info.id == info.id)
        {
            *existing = stored;
        } else {
            store.connections.push(stored);
        }
        self.save_store(&store)?;
        self.notify();
        Ok(info)
    }

    /// 删一个连接；返回是否真的存在过。
    pub fn delete_connection(&self, id: &str) -> Result<bool, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let before = store.connections.len();
        store.connections.retain(|item| item.info.id != id);
        let deleted = before != store.connections.len();
        if deleted {
            self.save_store(&store)?;
            self.notify();
        }
        Ok(deleted)
    }

    /// 设置页/CLI 的描述视图；不含任何 key 正文。
    pub fn view(&self) -> KeyringView {
        let identity_path = self.identity_path();
        let (connections, issues, issued_today, problem) = match self.load_store() {
            Ok(store) => {
                let now = now_seconds();
                let today_start = now.saturating_sub(now % 86_400);
                let issued_today = store
                    .issues
                    .iter()
                    .filter(|issue| issue.at >= today_start)
                    .count() as u64;
                (
                    store
                        .connections
                        .into_iter()
                        .map(|item| item.info)
                        .collect(),
                    store.issues,
                    issued_today,
                    None,
                )
            }
            Err(error) => (Vec::new(), Vec::new(), 0, Some(error.to_string())),
        };
        let first_use = problem.is_none() && connections.is_empty();
        let mut issues = issues;
        issues.reverse();
        issues.truncate(20);
        KeyringView {
            connections,
            key_file: KeyFileInfo {
                path: identity_path,
                since: self.since,
                problem,
            },
            issued_today,
            issues,
            first_use,
        }
    }

    /// 申请一次签发：找到连接、记一条记录、返回这一次要用的值。
    pub fn issue(
        &self,
        connection: &str,
        purpose: &str,
        run: Option<&str>,
        turn: Option<u64>,
    ) -> Result<IssuedSecret, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let stored = store
            .connections
            .iter()
            .find(|item| item.info.id == connection)
            .ok_or_else(|| KeyringError::MissingConnection(connection.to_owned()))?;
        if stored.api_key.trim().is_empty() {
            return Err(KeyringError::MissingConnection(format!(
                "{connection}（key 已撤销）"
            )));
        }
        let secret = stored.api_key.clone();
        store.next_issue = store.next_issue.saturating_add(1);
        let id = store.next_issue;
        store.issues.push(IssueRecord {
            id,
            at: now_seconds(),
            connection: connection.to_owned(),
            purpose: purpose.to_owned(),
            run: run.map(str::to_owned),
            turn,
        });
        if store.issues.len() > 200 {
            let excess = store.issues.len() - 200;
            store.issues.drain(0..excess);
        }
        self.save_store(&store)?;
        self.notify();
        Ok(IssuedSecret {
            connection: connection.to_owned(),
            secret,
        })
    }

    /// 签发记录（最近的在前）。
    pub fn issues(&self) -> Vec<IssueRecord> {
        match self.load_store() {
            Ok(mut store) => {
                store.issues.reverse();
                store.issues
            }
            Err(_) => Vec::new(),
        }
    }

    /// 验证之后写回注册器。
    pub fn record_check(&self, connection: &str, result: CheckResult) -> Result<(), KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let stored = store
            .connections
            .iter_mut()
            .find(|item| item.info.id == connection)
            .ok_or_else(|| KeyringError::MissingConnection(connection.to_owned()))?;
        stored.info.check = match result {
            CheckResult::Ok { at } => CheckState::Ok { at },
            CheckResult::Fail { at, reason } => CheckState::Fail { at, reason },
        };
        self.save_store(&store)?;
        self.notify();
        Ok(())
    }

    /// 换钥匙文件的位置：复制到新位置、更新配置、删掉旧位置。
    pub fn relocate_identity(&self, new_path: &Path) -> Result<(), KeyringError> {
        let _lock = self.lock()?;
        let old_path = self.identity_path();
        let text = fs::read_to_string(&old_path)?;
        create_identity_at(new_path, &text)?;
        self.write_config_for(new_path, self.since)?;
        if old_path != new_path {
            fs::remove_file(&old_path)?;
        }
        self.notify();
        Ok(())
    }

    /// 指到钥匙文件现在的位置（不复制）。
    pub fn repoint_identity(&self, path: &Path) -> Result<(), KeyringError> {
        let _lock = self.lock()?;
        self.load_identity(path)?;
        self.write_config_for(path, self.since)?;
        self.notify();
        Ok(())
    }

    /// 重新开始：清空 Keyring、生成新钥匙文件，回到第一次使用。
    pub fn restart(&self) -> Result<(), KeyringError> {
        let _lock = self.lock()?;
        let identity_path = self.identity_path();
        create_identity(&identity_path)?;
        let store_path = self.data_dir.join("keyring.age");
        if store_path.exists() {
            fs::remove_file(&store_path)?;
        }
        self.write_config_for(&identity_path, now_seconds())?;
        self.notify();
        Ok(())
    }

    /// T008.3 增补：只改 endpoint/model，保留已存 key。
    pub fn update_connection(
        &self,
        id: &str,
        endpoint: String,
        model: String,
    ) -> Result<ConnectionInfo, KeyringError> {
        if endpoint.trim().is_empty() || model.trim().is_empty() {
            return Err(KeyringError::InvalidConnection(
                "endpoint/model 不能为空".to_owned(),
            ));
        }
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let stored = store
            .connections
            .iter_mut()
            .find(|item| item.info.id == id)
            .ok_or_else(|| KeyringError::MissingConnection(id.to_owned()))?;
        stored.info.endpoint = endpoint;
        stored.info.model = model;
        let info = stored.info.clone();
        self.save_store(&store)?;
        self.notify();
        Ok(info)
    }

    /// T008.3 增补：撤销 key（连接保留，之后的签发失败）。
    pub fn revoke_key(&self, id: &str) -> Result<ConnectionInfo, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let stored = store
            .connections
            .iter_mut()
            .find(|item| item.info.id == id)
            .ok_or_else(|| KeyringError::MissingConnection(id.to_owned()))?;
        stored.api_key.clear();
        stored.info.key_saved = false;
        stored.info.check = CheckState::Never;
        let info = stored.info.clone();
        self.save_store(&store)?;
        self.notify();
        Ok(info)
    }

    /// 存储/钥匙文件变化通知（页面据此刷新）。
    pub fn subscribe(&self) -> Receiver<()> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut changes) = self.changes.lock() {
            changes.push(sender);
        }
        receiver
    }

    fn notify(&self) {
        if let Ok(mut changes) = self.changes.lock() {
            changes.retain(|sender| sender.send(()).is_ok());
        }
    }

    fn load_identity(&self, path: &Path) -> Result<String, KeyringError> {
        if !path.exists() {
            return Err(KeyringError::KeyFileMissing(path.to_path_buf()));
        }
        let text = fs::read_to_string(path)?;
        let identity: age::x25519::Identity = text
            .trim()
            .parse()
            .map_err(|error| KeyringError::Crypto(format!("钥匙文件解不开：{error}")))?;
        Ok(identity.to_string().expose_secret().to_owned())
    }

    fn load_store(&self) -> Result<Store, KeyringError> {
        let identity_path = self.identity_path();
        let identity_text = self.load_identity(&identity_path)?;
        let identity: age::x25519::Identity = identity_text
            .parse()
            .map_err(|error| KeyringError::Crypto(format!("钥匙文件解不开：{error}")))?;
        let store_path = self.data_dir.join("keyring.age");
        if !store_path.exists() {
            return Ok(Store::default());
        }
        let ciphertext = fs::read(&store_path)?;
        let decryptor = age::Decryptor::new(&ciphertext[..])
            .map_err(|error| KeyringError::Crypto(format!("存储解不开：{error}")))?;
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|error| KeyringError::Crypto(format!("存储解不开：{error}")))?;
        let mut plaintext = Vec::new();
        reader.read_to_end(&mut plaintext)?;
        serde_json::from_slice(&plaintext)
            .map_err(|error| KeyringError::Corrupt(format!("存储格式不对：{error}")))
    }

    fn save_store(&self, store: &Store) -> Result<(), KeyringError> {
        let identity_path = self.identity_path();
        let identity_text = self.load_identity(&identity_path)?;
        let identity: age::x25519::Identity = identity_text
            .parse()
            .map_err(|error| KeyringError::Crypto(format!("钥匙文件解不开：{error}")))?;
        let recipient = identity.to_public();
        let encryptor =
            age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .map_err(|error| KeyringError::Crypto(format!("加密失败：{error}")))?;
        let mut writer = encryptor
            .wrap_output(Vec::new())
            .map_err(|error| KeyringError::Crypto(format!("加密失败：{error}")))?;
        let plaintext =
            serde_json::to_vec(store).map_err(|error| KeyringError::Corrupt(error.to_string()))?;
        writer.write_all(&plaintext)?;
        let ciphertext = writer
            .finish()
            .map_err(|error| KeyringError::Crypto(format!("加密失败：{error}")))?;
        write_private_file(&self.data_dir.join("keyring.age"), &ciphertext)
    }

    fn write_config_for(&self, identity_path: &Path, since: u64) -> Result<(), KeyringError> {
        write_config(
            &self.data_dir.join("config.json"),
            &Config {
                version: 1,
                identity_path: identity_path.to_path_buf(),
                since,
            },
        )
    }
}

fn read_config(path: &Path) -> Result<Config, KeyringError> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|error| KeyringError::Corrupt(format!("配置格式不对：{error}")))
}

fn write_config(path: &Path, config: &Config) -> Result<(), KeyringError> {
    let json = serde_json::to_vec_pretty(config)
        .map_err(|error| KeyringError::Corrupt(error.to_string()))?;
    write_private_file(path, &json)
}

fn default_identity_path(data_dir: &Path) -> PathBuf {
    let parent = data_dir.parent().unwrap_or(data_dir);
    parent.join(".aios-keyring").join("identity.txt")
}

fn create_identity(path: &Path) -> Result<(), KeyringError> {
    let identity = age::x25519::Identity::generate();
    let text = identity.to_string();
    create_identity_at(path, text.expose_secret())
}

fn create_identity_at(path: &Path, identity_text: &str) -> Result<(), KeyringError> {
    write_private_file(path, identity_text.as_bytes())
}

fn create_private_dir(path: &Path) -> Result<(), KeyringError> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), KeyringError> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    let result = (|| -> Result<(), KeyringError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600))?;
        fs::rename(&temp_path, path)?;
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod issued_secret_drop_tests {
    use super::IssuedSecret;
    use std::sync::atomic::Ordering;

    #[test]
    fn drop_calls_zeroize() {
        super::ISSUED_DROP_CALLED.store(false, Ordering::Relaxed);
        let issued = IssuedSecret {
            connection: "deepseek".to_owned(),
            secret: "sk-test-drop".to_owned(),
        };
        drop(issued);
        assert!(
            super::ISSUED_DROP_CALLED.load(Ordering::Relaxed),
            "IssuedSecret Drop 必须调用 zeroize"
        );
    }
}
