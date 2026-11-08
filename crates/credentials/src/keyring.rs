//! Keyring 的公开类型与入口：构造、钥匙文件位置、变更通知。
//!
//! 存储/加解密在 storage，只读视图在 view，写路径在 update；
//! 三条路径都重新从文件读，不缓存，描述与记录里没有 key 正文。

mod storage;
mod update;
mod view;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::atomic::create_private_dir;
use storage::{Config, create_identity, default_identity_path, read_config, write_config};

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

/// 连接里的一行模型（价格是用户填的美元/百万 token；None = 用目录标价）。
///
/// 线上和存储都用 camelCase，和目录的 `ModelInfo` 是同一套字段名，页面读一处就够。
/// 旧存储的 `context_window`/`max_tokens` 和旧连接的 `price` 用 alias 照读，老数据不用迁移。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "context_window"
    )]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "max_tokens")]
    pub max_tokens: Option<u32>,
    /// T012 F1：用户填的四类单价（美元/百万 token；没填的格用目录标价）。旧连接只有
    /// 单数 `price`，读入时当作输入价和输出价（alias）；缓存两格可以不填。
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "price")]
    pub price_input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_cache_write: Option<f64>,
}

/// T012.4：编辑连接时一次传的新形状字段（endpoint/model 必填，其余可选）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConnectionUpdate {
    pub endpoint: String,
    pub model: String,
    pub preset: Option<String>,
    pub models: Option<Vec<ModelRow>>,
    pub auth_header: Option<String>,
}

/// 新建/覆盖一个连接时传入的内容（key 只在这一步进来）。
///
/// models = None 时用 model 一行；Some([]) 是「跟着厂商走」，不再补 [model]。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewConnection {
    pub id: String,
    pub name: String,
    pub model: String,
    pub endpoint: String,
    pub api_key: String,
    pub preset: Option<String>,
    pub models: Option<Vec<ModelRow>>,
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
    /// 模型行。None = 旧存储没有这个键（读的时候补成 [model]）；
    /// Some([]) = 显式「跟着厂商的列表走」，不能再补成 [model]。
    #[serde(default)]
    pub models: Option<Vec<ModelRow>>,
    /// T012.4：学到的鉴权头（x-api-key / authorization）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<String>,
}

impl ConnectionInfo {
    /// 连接里显式勾的第一行；显式空表（跟着厂商走）返回旧的 model 字段，
    /// 跟厂商走时可选的模型由 service 结合存下的厂商列表/内置目录解析。
    pub fn primary_model(&self) -> &str {
        self.models
            .as_deref()
            .and_then(|models| models.first())
            .map(|row| row.id.as_str())
            .unwrap_or(self.model.as_str())
    }
}

fn default_key_saved() -> bool {
    true
}

/// T012.4：旧存储只有 model——读的时候当成 models=[model]、preset=deepseek。
fn normalize_info(info: &mut ConnectionInfo) {
    // 只在没有这个键时补 [model]；显式 `[]` 是「跟着厂商走」，不能补。
    if info.models.is_none() && !info.model.is_empty() {
        info.models.get_or_insert_with(Vec::new).push(ModelRow {
            id: info.model.clone(),
            name: None,
            context_window: None,
            max_tokens: None,
            price_input: None,
            price_output: None,
            price_cache_read: None,
            price_cache_write: None,
        });
    }
    if info.preset.is_empty() {
        // spec：旧存储只有 model 的，一律按 deepseek 读；新形状保存时 preset 已显式给出。
        info.preset = "deepseek".to_owned();
    }
    if info.model.is_empty()
        && let Some(first) = info.models.as_deref().and_then(|models| models.first())
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
            models: None,
            auth_header: None,
        };
        normalize_info(&mut info);
        assert_eq!(info.models.as_deref().map(<[ModelRow]>::len), Some(1));
        assert_eq!(
            info.models.as_deref().and_then(|models| models.first()),
            Some(&ModelRow {
                id: "deepseek-flash".to_owned(),
                name: None,
                context_window: None,
                max_tokens: None,
                price_input: None,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
            })
        );
        assert_eq!(info.preset, "deepseek");
        assert_eq!(info.primary_model(), "deepseek-flash");
        // 旧存储非 deepseek 地址也一律按 spec 读成 deepseek。
        let mut relay = info.clone();
        relay.endpoint = "https://relay.example.com/anthropic".to_owned();
        relay.models = Some(Vec::new());
        relay.preset = String::new();
        normalize_info(&mut relay);
        assert_eq!(relay.preset, "deepseek");
    }

    #[test]
    fn old_json_without_models_key_normalizes_to_model_row() {
        let old = serde_json::json!({
            "id": "deepseek",
            "name": "DeepSeek",
            "model": "deepseek-flash",
            "endpoint": "https://api.deepseek.com/anthropic",
            "saved_at": 1,
            "check": {"kind": "never"},
            "key_saved": true,
            "preset": "deepseek"
        });
        let mut info: ConnectionInfo = serde_json::from_value(old).expect("旧存储");
        assert!(info.models.is_none(), "没有 models 键读成缺失");
        normalize_info(&mut info);
        assert_eq!(
            info.models
                .as_deref()
                .and_then(|models| models.first())
                .map(|row| row.id.as_str()),
            Some("deepseek-flash"),
            "旧存储按 [model] 读"
        );
    }

    #[test]
    fn explicit_empty_models_survive_save_and_reopen() {
        let data = tempfile::tempdir().expect("数据目录");
        let identity = data.path().join("identity.txt");
        let keyring = Keyring::open(data.path(), Some(&identity)).expect("keyring");
        let info = keyring
            .save_connection(NewConnection {
                id: "follow".to_owned(),
                name: "Follow".to_owned(),
                model: "ignored".to_owned(),
                endpoint: "https://api.example.com/v1".to_owned(),
                api_key: "test-key".to_owned(),
                preset: Some("custom".to_owned()),
                models: Some(Vec::new()),
                auth_header: None,
            })
            .expect("save");
        assert_eq!(info.models, Some(Vec::new()), "显式空表保存后仍是空表");
        let reopened = Keyring::open(data.path(), Some(&identity)).expect("reopen");
        let view = reopened.view();
        assert_eq!(
            view.connections[0].models,
            Some(Vec::new()),
            "读回仍是空表，不被补成 [model]"
        );
    }

    #[test]
    fn legacy_empty_models_array_follows_the_vendor() {
        // 旧页面把模型行删光也能保存，存下来就是 models: []。这一版把它读成「跟着厂商走」，
        // 不再补回 [model]；旧的无键存储仍按 [model] 读（见 old_json_without_models_key…）。
        let legacy: ConnectionInfo = serde_json::from_value(serde_json::json!({
            "id": "deepseek",
            "name": "DeepSeek",
            "model": "deepseek-flash",
            "endpoint": "https://api.deepseek.com/anthropic",
            "saved_at": 1,
            "check": {"kind": "never"},
            "key_saved": true,
            "preset": "deepseek",
            "models": []
        }))
        .expect("旧存储");
        assert_eq!(legacy.models, Some(Vec::new()), "显式空数组不补 [model]");
        let mut info = legacy;
        normalize_info(&mut info);
        assert_eq!(info.models, Some(Vec::new()), "normalize 也不补");
        assert_eq!(
            info.primary_model(),
            "deepseek-flash",
            "没有存下的厂商列表时退回旧的 model 字段"
        );
    }

    #[test]
    fn model_row_wire_is_camel_case_and_reads_old_snake() {
        let row = ModelRow {
            id: "m".to_owned(),
            name: Some("M".to_owned()),
            context_window: Some(128_000),
            max_tokens: Some(4096),
            price_input: Some(1.0),
            price_output: Some(2.0),
            price_cache_read: None,
            price_cache_write: None,
        };
        let json = serde_json::to_value(&row).expect("json");
        assert_eq!(json["contextWindow"], 128_000);
        assert_eq!(json["maxTokens"], 4096);
        assert!(json.get("context_window").is_none(), "新形状不再写 snake");
        assert!(json.get("max_tokens").is_none(), "新形状不再写 snake");
        let old: ModelRow = serde_json::from_value(serde_json::json!({
            "id": "m",
            "context_window": 128000,
            "max_tokens": 4096,
            "price": 1.5
        }))
        .expect("旧 shape");
        assert_eq!(old.context_window, Some(128_000), "旧 context_window 能读");
        assert_eq!(old.max_tokens, Some(4096), "旧 max_tokens 能读");
        assert_eq!(old.price_input, Some(1.5), "旧 price 别名照读");
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
                    price_input: None,
                    price_output: None,
                    price_cache_read: None,
                    price_cache_write: None,
                }]),
                auth_header: None,
            })
            .expect("save");
        assert_eq!(info.preset, "minimax-cn");
        assert_eq!(info.models.as_deref().map(<[ModelRow]>::len), Some(1));
        let reopened = Keyring::open(data.path(), Some(&identity)).expect("reopen");
        let view = reopened.view();
        let saved = &view.connections[0];
        assert_eq!(saved.preset, "minimax-cn");
        assert_eq!(
            saved
                .models
                .as_deref()
                .and_then(|models| models.first())
                .map(|row| row.id.as_str()),
            Some("MiniMax-M3.1-Flash")
        );
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
