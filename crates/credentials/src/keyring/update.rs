//! Keyring 的写路径：连接增删改、签发与验证记录、钥匙文件生命周期、cookie 签名钥匙。
//!
//! 每个入口先拿跨进程写锁，再读整份存储、改、原子写回、通知订阅者；校验失败不落盘。

use std::fs;
use std::path::Path;

use super::storage::{StoredConnection, create_identity, create_identity_at};
use super::{
    CheckResult, CheckState, ConnectionInfo, ConnectionUpdate, IssueRecord, IssuedSecret, Keyring,
    KeyringError, ModelRow, NewConnection, now_seconds,
};

impl Keyring {
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
                price_input: None,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
            }]
        });
        let model = models
            .first()
            .map(|row| row.id.clone())
            .unwrap_or_else(|| connection.model.clone());
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
            models: Some(models),
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
            if let Some(first) = models.first() {
                stored.info.model = first.id.clone();
            }
            stored.info.models = Some(models);
        } else if let Some(row) = stored
            .info
            .models
            .as_mut()
            .and_then(|models| models.first_mut())
        {
            row.id = update.model;
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
