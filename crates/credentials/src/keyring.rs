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

/// T012.4：连接里的一行模型（价格是用户填的美元/百万 token；None = 用目录标价）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRow {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
}

/// T012.4：编辑连接时一次传的新形状字段（endpoint/model 必填，其余可选）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConnectionUpdate {
    pub endpoint: String,
    pub model: String,
    pub preset: Option<String>,
    pub models: Option<Vec<ModelRow>>,
    pub price_rate: Option<f64>,
    pub auth_header: Option<String>,
}

/// 新建/覆盖一个连接时传入的内容（key 只在这一步进来；models 为空时用 model 一行）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewConnection {
    pub id: String,
    pub name: String,
    pub model: String,
    pub endpoint: String,
    pub api_key: String,
    pub preset: Option<String>,
    pub models: Option<Vec<ModelRow>>,
    pub price_rate: Option<f64>,
    pub auth_header: Option<String>,
}

/// 连接的描述，不含 key 正文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub name: String,
    /// T012.4 兼容字段：旧存储只有 model；新形状的模型见 models。
    pub model: String,
    pub endpoint: String,
    /// Unix 秒；界面自己格式化。
    pub saved_at: u64,
    pub check: CheckState,
    /// Keyring 里有没有这把 key（撤销后连接还在，但不能签发）。
    #[serde(default = "default_key_saved")]
    pub key_saved: bool,
    /// T012.4：预设 id（deepseek/minimax/minimax-cn/custom）。
    #[serde(default)]
    pub preset: String,
    /// T012.4：模型行；旧存储读出来是 [model]。
    #[serde(default)]
    pub models: Vec<ModelRow>,
    /// T012.4：生效价格倍率（0 = 免费）。
    #[serde(default = "default_price_rate")]
    pub price_rate: f64,
    /// T012.4：学到的鉴权头（x-api-key / authorization）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<String>,
}

fn default_price_rate() -> f64 {
    1.0
}

impl ConnectionInfo {
    /// T012.4：新形状取 models 第一行；旧存储已归一化到 models。
    pub fn primary_model(&self) -> &str {
        self.models
            .first()
            .map(|row| row.id.as_str())
            .unwrap_or(self.model.as_str())
    }
}

fn default_key_saved() -> bool {
    true
}

/// T012.4：旧存储只有 model——读的时候当成 models=[model]、preset=deepseek。
fn normalize_info(info: &mut ConnectionInfo) {
    if info.models.is_empty() && !info.model.is_empty() {
        info.models.push(ModelRow {
            id: info.model.clone(),
            name: None,
            context_window: None,
            max_tokens: None,
            price: None,
        });
    }
    if info.preset.is_empty() {
        // spec：旧存储只有 model 的，一律按 deepseek 读；新形状保存时 preset 已显式给出。
        info.preset = "deepseek".to_owned();
    }
    if info.model.is_empty()
        && let Some(first) = info.models.first()
    {
        info.model = first.id.clone();
    }
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// T008.F11：cookie 签名钥匙（hex 编码；age 密文落盘，不落明文）。
    #[serde(default)]
    cookie_secret: Option<String>,
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
        let models = connection.models.unwrap_or_else(|| {
            vec![ModelRow {
                id: connection.model.clone(),
                name: None,
                context_window: None,
                max_tokens: None,
                price: None,
            }]
        });
        let model = models
            .first()
            .map(|row| row.id.clone())
            .unwrap_or(connection.model);
        let preset = connection
            .preset
            .or_else(|| {
                crate::presets::preset_for_endpoint(&connection.endpoint)
                    .map(|preset| preset.id.to_owned())
            })
            .unwrap_or_else(|| "custom".to_owned());
        let info = ConnectionInfo {
            id: connection.id.clone(),
            name: connection.name,
            model,
            endpoint: connection.endpoint,
            saved_at: now_seconds(),
            check: CheckState::Never,
            key_saved: true,
            preset,
            models,
            price_rate: connection.price_rate.unwrap_or(1.0),
            auth_header: connection.auth_header,
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

    /// F1：当前持有的全部 key 正文（只在进程内交给监控脱敏；不落盘、不进页面/日志）。
    pub fn secrets(&self) -> Result<Vec<String>, KeyringError> {
        let _lock = self.lock()?;
        let store = self.load_store()?;
        Ok(store
            .connections
            .into_iter()
            .map(|stored| stored.api_key)
            .filter(|key| !key.is_empty())
            .collect())
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

    /// T008.F11：cookie 签名钥匙——Keyring 里的一条持久记录；没有就新建并写入。
    ///
    /// 只在启动时读一次、用时留在内存；age 密文落盘（不落明文）。
    pub fn cookie_signing_secret(&self) -> Result<[u8; 32], KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        if let Some(hex) = store.cookie_secret.as_deref() {
            return decode_cookie_secret(hex);
        }
        let secret = random_secret();
        store.cookie_secret = Some(encode_cookie_secret(&secret));
        self.save_store(&store)?;
        Ok(secret)
    }

    /// T008.F11：删除 cookie 签名钥匙记录（下次启动重新生成，旧页面会话全部失效）。
    pub fn delete_cookie_signing_secret(&self) -> Result<bool, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let existed = store.cookie_secret.take().is_some();
        if existed {
            self.save_store(&store)?;
            self.notify();
        }
        Ok(existed)
    }

    /// T008.3 增补：只改 endpoint/model，保留已存 key。
    pub fn update_connection(
        &self,
        id: &str,
        endpoint: String,
        model: String,
    ) -> Result<ConnectionInfo, KeyringError> {
        self.update_connection_details(
            id,
            ConnectionUpdate {
                endpoint,
                model,
                ..ConnectionUpdate::default()
            },
        )
    }

    /// T012.4：编辑连接时同时更新新形状字段。
    pub fn update_connection_details(
        &self,
        id: &str,
        update: ConnectionUpdate,
    ) -> Result<ConnectionInfo, KeyringError> {
        if update.endpoint.trim().is_empty() || update.model.trim().is_empty() {
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
        stored.info.endpoint = update.endpoint;
        stored.info.model = update.model.clone();
        if let Some(preset) = update.preset {
            stored.info.preset = preset;
        }
        if let Some(models) = update.models {
            stored.info.models = models;
            if let Some(first) = stored.info.models.first() {
                stored.info.model = first.id.clone();
            }
        } else if let Some(row) = stored.info.models.first_mut() {
            row.id = update.model;
        }
        if let Some(price_rate) = update.price_rate {
            stored.info.price_rate = price_rate;
        }
        if update.auth_header.is_some() {
            stored.info.auth_header = update.auth_header;
        }
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
        let mut store: Store = serde_json::from_slice(&plaintext)
            .map_err(|error| KeyringError::Corrupt(format!("存储格式不对：{error}")))?;
        for stored in &mut store.connections {
            normalize_info(&mut stored.info);
        }
        Ok(store)
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
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    default_identity_path_for(data_dir, xdg.as_deref(), home.as_deref())
}

/// T008.F7：默认放用户配置目录（XDG_CONFIG_HOME 或 HOME/.config），不在数据目录里。
fn default_identity_path_for(
    data_dir: &Path,
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
) -> PathBuf {
    let root = xdg_config_home
        .map(Path::to_path_buf)
        .or_else(|| home.map(|home| home.join(".config")))
        .unwrap_or_else(|| data_dir.parent().unwrap_or(data_dir).join(".config"));
    root.join("aios").join("keyring").join("identity.txt")
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

/// T008.F11：cookie 签名钥匙的 hex 编码（存进 age 密文里的 Store）。
fn encode_cookie_secret(secret: &[u8; 32]) -> String {
    secret.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// T008.F11：解码 hex 形式的 cookie 签名钥匙。
fn decode_cookie_secret(hex: &str) -> Result<[u8; 32], KeyringError> {
    if hex.len() != 64 {
        return Err(KeyringError::Corrupt("cookie 签名钥匙长度不对".to_owned()));
    }
    let mut secret = [0u8; 32];
    for (index, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk)
            .map_err(|_| KeyringError::Corrupt("cookie 签名钥匙不是 hex".to_owned()))?;
        secret[index] = u8::from_str_radix(text, 16)
            .map_err(|_| KeyringError::Corrupt("cookie 签名钥匙不是 hex".to_owned()))?;
    }
    Ok(secret)
}

/// T008.F11：cookie 签名钥匙的随机 32 字节（ring 系统随机数）。
fn random_secret() -> [u8; 32] {
    use ring::rand::SecureRandom;

    let mut secret = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut secret)
        .expect("ring 系统随机数不可用");
    secret
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

#[cfg(test)]
mod default_path_tests {
    use super::*;

    #[test]
    fn default_identity_is_in_user_config_not_data_dir() {
        let data_dir = Path::new("/tmp/aios-data");
        let xdg = Path::new("/tmp/xdg-config");
        let path = default_identity_path_for(data_dir, Some(xdg), None);
        assert_eq!(path, xdg.join("aios").join("keyring").join("identity.txt"));
        assert!(!path.starts_with(data_dir), "默认钥匙不能在数据目录里");
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    #[test]
    fn old_model_only_connection_normalizes_to_models_and_deepseek_preset() {
        let mut info = ConnectionInfo {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            saved_at: 1,
            check: CheckState::Never,
            key_saved: true,
            preset: String::new(),
            models: Vec::new(),
            price_rate: 0.0,
            auth_header: None,
        };
        normalize_info(&mut info);
        assert_eq!(info.models.len(), 1);
        assert_eq!(info.models[0].id, "deepseek-flash");
        assert_eq!(info.preset, "deepseek");
        assert_eq!(info.primary_model(), "deepseek-flash");
        // 旧存储非 deepseek 地址也一律按 spec 读成 deepseek。
        let mut relay = info.clone();
        relay.endpoint = "https://relay.example.com/anthropic".to_owned();
        relay.models.clear();
        relay.preset = String::new();
        normalize_info(&mut relay);
        assert_eq!(relay.preset, "deepseek");
    }

    #[test]
    fn new_connection_saves_new_shape_and_keeps_key() {
        let data = tempfile::tempdir().expect("数据目录");
        let identity = data.path().join("identity.txt");
        let keyring = Keyring::open(data.path(), Some(&identity)).expect("keyring");
        let info = keyring
            .save_connection(NewConnection {
                id: "minimax".to_owned(),
                name: "MiniMax".to_owned(),
                model: "MiniMax-M3.1-Flash".to_owned(),
                endpoint: "https://api.minimaxi.com/anthropic".to_owned(),
                api_key: "test-key".to_owned(),
                preset: Some("minimax-cn".to_owned()),
                models: Some(vec![ModelRow {
                    id: "MiniMax-M3.1-Flash".to_owned(),
                    name: None,
                    context_window: Some(1_000_000),
                    max_tokens: Some(524_288),
                    price: None,
                }]),
                price_rate: Some(0.0),
                auth_header: None,
            })
            .expect("save");
        assert_eq!(info.preset, "minimax-cn");
        assert_eq!(info.models.len(), 1);
        assert_eq!(info.price_rate, 0.0);
        let reopened = Keyring::open(data.path(), Some(&identity)).expect("reopen");
        let view = reopened.view();
        let saved = &view.connections[0];
        assert_eq!(saved.preset, "minimax-cn");
        assert_eq!(saved.models[0].id, "MiniMax-M3.1-Flash");
        assert!(saved.key_saved);
        let issued = reopened
            .issue("minimax", "test", None, None)
            .expect("issue");
        assert_eq!(issued.into_secret(), "test-key");
    }

    #[test]
    fn presets_cover_the_three_connections() {
        let ids: Vec<&str> = crate::presets::PRESETS
            .iter()
            .map(|preset| preset.id)
            .collect();
        assert_eq!(ids, vec!["deepseek", "minimax", "minimax-cn"]);
        assert_eq!(
            crate::presets::preset("minimax-cn").unwrap().endpoint,
            "https://api.minimaxi.com/anthropic"
        );
    }
}
