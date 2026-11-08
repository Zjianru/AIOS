//! Keyring 的落盘：age 加解密、配置文件与钥匙文件、跨进程写锁。
//!
//! 这里只回答「字节怎么安全地读写」，不决定设置页看到什么、也不做连接的业务校验。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use age::secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};

use super::{
    CheckState, ConnectionInfo, IssueRecord, Keyring, KeyringError, normalize_info, now_seconds,
};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Store {
    #[serde(default)]
    pub(super) connections: Vec<StoredConnection>,
    /// T008.F11：cookie 签名钥匙（hex 编码；age 密文落盘，不落明文）。
    #[serde(default)]
    pub(super) cookie_secret: Option<String>,
    /// 工具凭证（D73）：这一期只有 Exa 的 key；age 密文落盘，不出现在任何视图里。
    #[serde(default)]
    pub(super) exa: Option<StoredToolKey>,
    #[serde(default)]
    pub(super) issues: Vec<IssueRecord>,
    #[serde(default)]
    pub(super) next_issue: u64,
}

/// 一种工具凭证的存储形状：正文 + 保存时间 + 最近一次验证结果。
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StoredToolKey {
    pub(super) api_key: String,
    pub(super) saved_at: u64,
    #[serde(default)]
    pub(super) check: CheckState,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StoredConnection {
    pub(super) info: ConnectionInfo,
    pub(super) api_key: String,
}

impl Store {
    /// 记一条签发记录（只追加，只留最近 200 条）；不记正文。
    pub(super) fn record_issue(
        &mut self,
        connection: &str,
        purpose: &str,
        run: Option<&str>,
        turn: Option<u64>,
    ) {
        self.next_issue = self.next_issue.saturating_add(1);
        let id = self.next_issue;
        self.issues.push(IssueRecord {
            id,
            at: now_seconds(),
            connection: connection.to_owned(),
            purpose: purpose.to_owned(),
            run: run.map(str::to_owned),
            turn,
        });
        if self.issues.len() > 200 {
            let excess = self.issues.len() - 200;
            self.issues.drain(0..excess);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Config {
    pub(super) version: u32,
    pub(super) identity_path: PathBuf,
    pub(super) since: u64,
}

impl Keyring {
    /// 进程内/跨进程文件锁：读改写整段持有；drop 释放。
    pub(super) fn lock(&self) -> Result<File, KeyringError> {
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

    pub(super) fn load_identity(&self, path: &Path) -> Result<String, KeyringError> {
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

    pub(super) fn load_store(&self) -> Result<Store, KeyringError> {
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

    pub(super) fn save_store(&self, store: &Store) -> Result<(), KeyringError> {
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

    pub(super) fn write_config_for(
        &self,
        identity_path: &Path,
        since: u64,
    ) -> Result<(), KeyringError> {
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

pub(super) fn read_config(path: &Path) -> Result<Config, KeyringError> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|error| KeyringError::Corrupt(format!("配置格式不对：{error}")))
}

pub(super) fn write_config(path: &Path, config: &Config) -> Result<(), KeyringError> {
    let json = serde_json::to_vec_pretty(config)
        .map_err(|error| KeyringError::Corrupt(error.to_string()))?;
    write_private_file(path, &json)
}

pub(super) fn default_identity_path(data_dir: &Path) -> PathBuf {
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

pub(super) fn create_identity(path: &Path) -> Result<(), KeyringError> {
    let identity = age::x25519::Identity::generate();
    let text = identity.to_string();
    create_identity_at(path, text.expose_secret())
}

pub(super) fn create_identity_at(path: &Path, identity_text: &str) -> Result<(), KeyringError> {
    write_private_file(path, identity_text.as_bytes())
}

/// 私有文件的原子写：目录 0700、文件 0600，临时文件 + fsync + rename + 目录 fsync
/// 都在 credentials::atomic 一处（和根状态/设置/汇率缓存同一模块）。
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), KeyringError> {
    crate::atomic::write(path, bytes, crate::atomic::TargetKind::Private)?;
    Ok(())
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
