//! 设置/连接/Keyring/Exa 的确认卡动作（属于 aios-settings）。
//!
//! 连接和 Exa 的 key 值永远不进 details；Keyring 换位置/重开只接受用户原话里的 / 或 ~/ 路径。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::protocol::{
    ConnectionDeleteRequest, ConnectionRevokeRequest, ConnectionUpdateRequest,
    KeyringRelocateRequest, KeyringReopenRequest, SettingsUpdateRequest, VisionModelRefView,
};
use crate::root_lifecycle::RootHost;

fn text_field(raw: &Value, key: &str) -> Result<String, String> {
    raw.as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("details.{key} 必须是非空字符串"))
}

fn reject_key_fields(raw: &Value) -> Result<(), String> {
    if let Some(object) = raw.as_object() {
        for key in ["key", "api_key", "apiKey", "secret"] {
            if object.contains_key(key) {
                return Err("key 值不经过 Kind，也不需要确认卡；去设置页对应连接里填。".to_owned());
            }
        }
    }
    Ok(())
}

pub(crate) struct SettingsUpdateAction {
    data_dir: PathBuf,
}

impl SettingsUpdateAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn current(&self) -> Result<crate::settings::Settings, String> {
        Ok(crate::settings::load(&self.data_dir))
    }

    fn fields(
        raw: &Value,
    ) -> Result<
        (
            Option<u8>,
            Option<bool>,
            Option<Vec<VisionModelRefView>>,
            Option<bool>,
        ),
        String,
    > {
        reject_key_fields(raw)?;
        let object = raw
            .as_object()
            .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
        if object.contains_key("currency") {
            return Err("币种已删除；设置只认 seats、bash、visionModels、screenCapture".to_owned());
        }
        let seats = match object.get("seats") {
            None | Some(Value::Null) => None,
            Some(Value::Number(value)) => Some(
                value
                    .as_u64()
                    .and_then(|seats| u8::try_from(seats).ok())
                    .filter(|seats| {
                        (crate::settings::MIN_SEATS..=crate::settings::MAX_SEATS).contains(seats)
                    })
                    .ok_or_else(|| "details.seats 必须在 1–16 之间".to_owned())?,
            ),
            Some(_) => return Err("details.seats 必须是整数".to_owned()),
        };
        let bash = match object.get("bash") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(value)) => Some(*value),
            Some(_) => return Err("details.bash 必须是布尔值".to_owned()),
        };
        let vision_models = match object.get("visionModels") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                serde_json::from_value::<Vec<VisionModelRefView>>(value.clone()).map_err(|_| {
                    "details.visionModels 必须是 {connection, model} 数组".to_owned()
                })?,
            ),
        };
        let screen_capture = match object.get("screenCapture") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(value)) => Some(*value),
            Some(_) => return Err("details.screenCapture 必须是布尔值".to_owned()),
        };
        if seats.is_none() && bash.is_none() && vision_models.is_none() && screen_capture.is_none()
        {
            return Err(
                "details 至少要带 seats、bash、visionModels 或 screenCapture 一项".to_owned(),
            );
        }
        Ok((seats, bash, vision_models, screen_capture))
    }
}

impl ActionKind for SettingsUpdateAction {
    fn name(&self) -> &str {
        "settings_update"
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let (seats, bash, vision_models, screen_capture) = Self::fields(raw)?;
        Ok(json!({
            "seats": seats,
            "bash": bash,
            "visionModels": vision_models,
            "screenCapture": screen_capture,
        }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let (seats, bash, vision_models, screen_capture) = Self::fields(details)?;
        let current = self.current()?;
        let mut facts = Vec::new();
        if let Some(seats) = seats {
            facts.push(CardFact {
                name: "席位".to_owned(),
                value: format!("{} → {seats}", current.exec.seats),
            });
        }
        if let Some(bash) = bash {
            facts.push(CardFact {
                name: "bash".to_owned(),
                value: format!(
                    "{} → {}",
                    if current.exec.bash { "开" } else { "关" },
                    if bash { "开" } else { "关" }
                ),
            });
        }
        if let Some(models) = vision_models {
            let before = current
                .vision
                .models
                .iter()
                .map(|row| format!("{}/{}", row.connection, row.model))
                .collect::<Vec<_>>()
                .join("、");
            let after = models
                .iter()
                .map(|row| format!("{}/{}", row.connection, row.model))
                .collect::<Vec<_>>()
                .join("、");
            facts.push(CardFact {
                name: "看图模型".to_owned(),
                value: format!(
                    "{} → {}",
                    if before.is_empty() { "没有" } else { &before },
                    if after.is_empty() { "清空" } else { &after }
                ),
            });
        }
        if let Some(screen_capture) = screen_capture {
            facts.push(CardFact {
                name: "让 Kind 截桌面".to_owned(),
                value: format!(
                    "{} → {}",
                    if current.vision.screen_capture {
                        "开"
                    } else {
                        "关"
                    },
                    if screen_capture { "开" } else { "关" }
                ),
            });
        }
        Ok(facts)
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let (seats, bash, vision_models, screen_capture) = Self::fields(details)?;
            host.settings_update(SettingsUpdateRequest {
                paused: None,
                seats,
                bash,
                vision_models,
                screen_capture,
            })
            .await
            .map_err(|error| error.to_string())?;
            Ok(ActionOutcome {
                summary: "设置改好了".to_owned(),
                data: json!({}),
            })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionKind {
    Update,
    Delete,
    Revoke,
}

pub(crate) struct ConnectionAction {
    data_dir: PathBuf,
    identity: Option<PathBuf>,
    kind: ConnectionKind,
}

impl ConnectionAction {
    pub(crate) fn new(data_dir: PathBuf, identity: Option<PathBuf>, kind: ConnectionKind) -> Self {
        Self {
            data_dir,
            identity,
            kind,
        }
    }

    fn keyring(&self) -> Result<credentials::Keyring, String> {
        credentials::Keyring::open(&self.data_dir, self.identity.as_deref())
            .map_err(|error| format!("Keyring 读不出来：{error}"))
    }

    fn id(raw: &Value) -> Result<String, String> {
        reject_key_fields(raw)?;
        text_field(raw, "id")
    }

    fn connection(&self, id: &str) -> Result<credentials::ConnectionInfo, String> {
        self.keyring()?
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == id)
            .ok_or_else(|| format!("找不到这个连接：{id}"))
    }
}

impl ActionKind for ConnectionAction {
    fn name(&self) -> &str {
        match self.kind {
            ConnectionKind::Update => "connection_update",
            ConnectionKind::Delete => "connection_delete",
            ConnectionKind::Revoke => "connection_revoke",
        }
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        reject_key_fields(raw)?;
        let id = Self::id(raw)?;
        if self.kind == ConnectionKind::Update {
            Ok(json!({
                "id": id,
                "endpoint": text_field(raw, "endpoint")?,
                "model": text_field(raw, "model")?,
            }))
        } else {
            Ok(json!({ "id": id }))
        }
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let connection = self.connection(&Self::id(details)?)?;
        match self.kind {
            ConnectionKind::Update => Ok(vec![
                CardFact {
                    name: "连接".to_owned(),
                    value: format!("{}「{}」", connection.id, connection.name),
                },
                CardFact {
                    name: "地址".to_owned(),
                    value: format!(
                        "{} → {}",
                        connection.endpoint,
                        text_field(details, "endpoint")?
                    ),
                },
                CardFact {
                    name: "模型".to_owned(),
                    value: format!(
                        "{} → {}",
                        connection.primary_model(),
                        text_field(details, "model")?
                    ),
                },
            ]),
            ConnectionKind::Delete => Ok(vec![
                CardFact {
                    name: "连接".to_owned(),
                    value: format!("{}「{}」", connection.id, connection.name),
                },
                CardFact {
                    name: "动作".to_owned(),
                    value: "删除这个连接".to_owned(),
                },
            ]),
            ConnectionKind::Revoke => Ok(vec![
                CardFact {
                    name: "连接".to_owned(),
                    value: format!("{}「{}」", connection.id, connection.name),
                },
                CardFact {
                    name: "key".to_owned(),
                    value: format!(
                        "{} → 撤销（连接保留）",
                        if connection.key_saved {
                            "已保存"
                        } else {
                            "没有 key"
                        }
                    ),
                },
            ]),
        }
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let id = Self::id(details)?;
            match self.kind {
                ConnectionKind::Update => {
                    let response = host
                        .connection_update(ConnectionUpdateRequest {
                            id,
                            endpoint: text_field(details, "endpoint")?,
                            model: text_field(details, "model")?,
                            preset: None,
                            models: None,
                            auth_header: None,
                        })
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok(ActionOutcome {
                        summary: format!("连接「{}」改好了", response.connection.name),
                        data: json!({ "connection": response.connection }),
                    })
                }
                ConnectionKind::Delete => {
                    let response = host
                        .connection_delete(ConnectionDeleteRequest { id })
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok(ActionOutcome {
                        summary: if response.deleted {
                            "连接删了".to_owned()
                        } else {
                            "这个连接已经不在了".to_owned()
                        },
                        data: json!({}),
                    })
                }
                ConnectionKind::Revoke => {
                    let response = host
                        .connection_revoke(ConnectionRevokeRequest { id })
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok(ActionOutcome {
                        summary: format!(
                            "连接「{}」的 key 撤销了，连接保留",
                            response.connection.name
                        ),
                        data: json!({ "connection": response.connection }),
                    })
                }
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyringKind {
    Relocate,
    Reopen,
    Restart,
}

pub(crate) struct KeyringAction {
    data_dir: PathBuf,
    identity: Option<PathBuf>,
}

impl KeyringAction {
    pub(crate) fn new(data_dir: PathBuf, identity: Option<PathBuf>) -> Self {
        Self { data_dir, identity }
    }

    fn kind(&self, raw: &Value) -> Result<KeyringKind, String> {
        match raw
            .as_object()
            .and_then(|object| object.get("action"))
            .and_then(Value::as_str)
        {
            Some("relocate") => Ok(KeyringKind::Relocate),
            Some("reopen") => Ok(KeyringKind::Reopen),
            Some("restart") => Ok(KeyringKind::Restart),
            Some(other) => Err(format!(
                "details.action 必须是 relocate、reopen 或 restart：{other}"
            )),
            None => Err("details.action 必须是 relocate、reopen 或 restart".to_owned()),
        }
    }

    fn keyring(&self) -> Result<credentials::Keyring, String> {
        credentials::Keyring::open(&self.data_dir, self.identity.as_deref())
            .map_err(|error| format!("Keyring 读不出来：{error}"))
    }

    fn path(raw: &Value) -> Result<Option<String>, String> {
        match raw.as_object().and_then(|object| object.get("path")) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(path)) => Ok(Some(path.clone())),
            Some(_) => Err("details.path 必须是字符串".to_owned()),
        }
    }

    fn checked_path(&self, raw: &Value, env: &ActionEnv<'_>) -> Result<String, String> {
        let path =
            Self::path(raw)?.ok_or_else(|| "details.path 必须给出 / 或 ~/ 路径".to_owned())?;
        if !path.starts_with('/') && !path.starts_with("~/") {
            return Err("路径必须以 / 或 ~/ 开头".to_owned());
        }
        let conversation = env
            .conversation
            .ok_or_else(|| "没有发起会话 id，核不了路径".to_owned())?;
        if crate::ledger::find_user_words(env.data_dir, conversation, &path)?.is_none() {
            return Err(format!(
                "路径『{path}』不在用户原话里，先问用户要换到哪里；核对通过再出卡。"
            ));
        }
        Ok(path)
    }

    fn checked_path_in_act(&self, raw: &Value, host: &RootHost) -> Result<String, String> {
        let path =
            Self::path(raw)?.ok_or_else(|| "details.path 必须给出 / 或 ~/ 路径".to_owned())?;
        if !path.starts_with('/') && !path.starts_with("~/") {
            return Err("路径必须以 / 或 ~/ 开头".to_owned());
        }
        let conversation = host
            .info()
            .session_id
            .ok_or_else(|| "根会话没开，核不了路径".to_owned())?;
        if crate::ledger::find_user_words(&host.options.data_dir, &conversation, &path)?.is_none() {
            return Err(format!(
                "路径『{path}』不在用户原话里，先问用户要换到哪里；核对通过再出卡。"
            ));
        }
        Ok(path)
    }
}

impl ActionKind for KeyringAction {
    fn name(&self) -> &str {
        "keyring_action"
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        reject_key_fields(raw)?;
        let kind = self.kind(raw)?;
        let action = match kind {
            KeyringKind::Relocate => "relocate",
            KeyringKind::Reopen => "reopen",
            KeyringKind::Restart => "restart",
        };
        let path = Self::path(raw)?;
        if kind != KeyringKind::Restart && path.is_none() {
            return Err("details.path 必须给出 / 或 ~/ 路径".to_owned());
        }
        let path = path
            .map(|path| {
                if path.starts_with('/') || path.starts_with("~/") {
                    Ok(path)
                } else {
                    Err("路径必须以 / 或 ~/ 开头".to_owned())
                }
            })
            .transpose()?;
        Ok(json!({ "action": action, "path": path }))
    }

    fn describe(&self, details: &Value, env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let key_file = self
            .keyring()?
            .view()
            .key_file
            .path
            .to_string_lossy()
            .into_owned();
        match self.kind(details)? {
            KeyringKind::Relocate => {
                let path = self.checked_path(details, env)?;
                Ok(vec![
                    CardFact {
                        name: "动作".to_owned(),
                        value: "换 Keyring 位置".to_owned(),
                    },
                    CardFact {
                        name: "位置".to_owned(),
                        value: format!("{key_file} → {path}"),
                    },
                ])
            }
            KeyringKind::Reopen => {
                let path = self.checked_path(details, env)?;
                Ok(vec![
                    CardFact {
                        name: "动作".to_owned(),
                        value: "重新读这个 Keyring".to_owned(),
                    },
                    CardFact {
                        name: "位置".to_owned(),
                        value: format!("{key_file} → {path}"),
                    },
                ])
            }
            KeyringKind::Restart => Ok(vec![
                CardFact {
                    name: "动作".to_owned(),
                    value: "重新读 Keyring".to_owned(),
                },
                CardFact {
                    name: "位置".to_owned(),
                    value: key_file,
                },
            ]),
        }
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            match self.kind(details)? {
                KeyringKind::Relocate => {
                    let path = self.checked_path_in_act(details, host)?;
                    host.keyring_relocate(KeyringRelocateRequest { to: path })
                        .await
                        .map_err(|error| error.to_string())?;
                }
                KeyringKind::Reopen => {
                    let path = self.checked_path_in_act(details, host)?;
                    host.keyring_reopen(KeyringReopenRequest { at: path })
                        .await
                        .map_err(|error| error.to_string())?;
                }
                KeyringKind::Restart => {
                    host.keyring_restart()
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            Ok(ActionOutcome {
                summary: "Keyring 已处理".to_owned(),
                data: json!({}),
            })
        })
    }
}

pub(crate) struct ExaKeyDeleteAction {
    data_dir: PathBuf,
    identity: Option<PathBuf>,
}

impl ExaKeyDeleteAction {
    pub(crate) fn new(data_dir: PathBuf, identity: Option<PathBuf>) -> Self {
        Self { data_dir, identity }
    }

    fn keyring(&self) -> Result<credentials::Keyring, String> {
        credentials::Keyring::open(&self.data_dir, self.identity.as_deref())
            .map_err(|error| format!("Keyring 读不出来：{error}"))
    }
}

impl ActionKind for ExaKeyDeleteAction {
    fn name(&self) -> &str {
        "exa_key_delete"
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        reject_key_fields(raw)?;
        Ok(json!({}))
    }

    fn describe(&self, _details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let view = self
            .keyring()?
            .exa_key_view()
            .map_err(|error| format!("Exa key 读不出来：{error}"))?;
        if !view.key.saved {
            return Err("Exa key 现在没存，不用删".to_owned());
        }
        Ok(vec![
            CardFact {
                name: "动作".to_owned(),
                value: "删除 Exa 的 key".to_owned(),
            },
            CardFact {
                name: "影响".to_owned(),
                value: "联网搜索和网页工具暂时不能用；之后去设置页重填".to_owned(),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        _details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let view = host
                .exa_key_delete()
                .await
                .map_err(|error| error.to_string())?;
            Ok(ActionOutcome {
                summary: "Exa 的 key 删了；要用时去设置页重填".to_owned(),
                data: json!({ "saved": view.key.saved }),
            })
        })
    }
}
