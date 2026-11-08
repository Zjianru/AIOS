//! 屏蔽、解除屏蔽一个模型（属于 aios-settings）：走设置里的屏蔽规则表。
//!
//! 事实是这个模型的状态之前、之后。模型不存在、已经屏蔽、解除时没有单独规则时不提议；
//! 点确认前重算一遍事实，设置页在中间改过就对不上，记 refused 不执行。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

/// 做哪个方向：屏蔽，还是解除。
#[derive(Clone, Copy)]
enum Direction {
    Mask,
    Unmask,
}

/// 设置域里的屏蔽动作。
pub(crate) struct MaskAction {
    data_dir: PathBuf,
    identity: Option<PathBuf>,
    direction: Direction,
}

struct Parts {
    connection: String,
    model: String,
}

impl MaskAction {
    pub(crate) fn mask(data_dir: PathBuf, identity: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            identity,
            direction: Direction::Mask,
        }
    }

    pub(crate) fn unmask(data_dir: PathBuf, identity: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            identity,
            direction: Direction::Unmask,
        }
    }

    fn parts(&self, raw: &Value) -> Result<Parts, String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
        let text = |key: &str| -> Result<String, String> {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("details.{key} 必须是非空字符串"))
        };
        Ok(Parts {
            connection: text("connection")?,
            model: text("model")?,
        })
    }

    fn keyring(&self) -> Result<credentials::Keyring, String> {
        credentials::Keyring::open(&self.data_dir, self.identity.as_deref())
            .map_err(|error| format!("读不出 Keyring：{error}"))
    }

    /// 现在的屏蔽表；文件读不出来时不当成空表。
    fn rules(&self) -> Result<Vec<String>, String> {
        let table = crate::disabled::table(&self.data_dir);
        if table.unknown {
            return Err("settings.json 读不出来，不知道现在的屏蔽表".to_owned());
        }
        Ok(table.models)
    }
}

/// 这个模型现在的状态：可用、已屏蔽（规则「…」）；命中自己那条精确规则时只说已屏蔽。
fn mask_state(rules: &[String], connection: &str, model: &str, exact: &str) -> String {
    match crate::disabled::match_disabled(rules, connection, model) {
        None => "可用".to_owned(),
        Some(rule) if rule.trim() == exact => "已屏蔽".to_owned(),
        Some(rule) => format!("已屏蔽（规则「{rule}」）"),
    }
}

impl ActionKind for MaskAction {
    fn name(&self) -> &str {
        match self.direction {
            Direction::Mask => "mask_model",
            Direction::Unmask => "unmask_model",
        }
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let parts = self.parts(raw)?;
        Ok(json!({
            "connection": parts.connection,
            "model": parts.model,
        }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let parts = self.parts(details)?;
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .iter()
            .find(|connection| connection.id == parts.connection)
            .ok_or_else(|| format!("找不到连接 {}", parts.connection))?;
        let models = crate::models::selectable_models(&self.data_dir, connection);
        if !models.iter().any(|info| info.id == parts.model) {
            return Err(format!("这个连接里没有模型 {}", parts.model));
        }
        let rules = self.rules()?;
        let exact = format!("{}/{}", parts.connection, parts.model);
        let before = mask_state(&rules, &parts.connection, &parts.model, &exact);
        let fact = |after: &str| CardFact {
            name: "屏蔽状态".to_owned(),
            value: format!("{before} → {after}"),
        };
        match self.direction {
            Direction::Mask => {
                if let Some(rule) =
                    crate::disabled::match_disabled(&rules, &parts.connection, &parts.model)
                {
                    return Err(format!("这个模型已经屏蔽了（规则「{rule}」）"));
                }
                Ok(vec![fact("已屏蔽")])
            }
            Direction::Unmask => {
                if !rules.iter().any(|rule| rule.trim() == exact) {
                    return Err("没有单独屏蔽这个模型的规则".to_owned());
                }
                let remaining: Vec<String> = rules
                    .iter()
                    .filter(|rule| rule.trim() != exact)
                    .cloned()
                    .collect();
                if let Some(rule) =
                    crate::disabled::match_disabled(&remaining, &parts.connection, &parts.model)
                {
                    return Err(format!(
                        "解除这一条以后仍被规则「{rule}」命中，去设置页改那条规则"
                    ));
                }
                Ok(vec![fact("可用")])
            }
        }
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let parts = self.parts(details)?;
            let exact = format!("{}/{}", parts.connection, parts.model);
            let settings = crate::settings::load_for_write(&self.data_dir)
                .map_err(|reason| format!("读不出设置：{reason}"))?;
            let mut models = settings.disabled_models;
            let summary = match self.direction {
                Direction::Mask => {
                    models.push(exact.clone());
                    format!("已屏蔽「{}」", parts.model)
                }
                Direction::Unmask => {
                    models.retain(|rule| rule.trim() != exact);
                    format!("已解除屏蔽「{}」", parts.model)
                }
            };
            let response = host
                .disabled_models_set(crate::protocol::DisabledModelsSetRequest { models })
                .await
                .map_err(|error| error.to_string())?;
            Ok(ActionOutcome {
                summary,
                data: json!({
                    "connection": parts.connection,
                    "model": parts.model,
                    "rule": exact,
                    "disabledModels": response.models,
                }),
            })
        })
    }
}
