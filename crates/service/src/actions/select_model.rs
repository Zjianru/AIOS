//! 换模型（属于 aios-settings）：走会话的模型选择，同时更新新会话的上次选择。
//!
//! 事实是模型和推理强度的之前、之后；没变的那一行不显示。模型不存在、已经是要改成的
//! 样子时不提议。点确认前重算一遍事实：设置页在中间改过，就会对不上，记 refused 不执行。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

/// 设置域里的换模型动作。
pub(crate) struct SelectModelAction {
    data_dir: PathBuf,
    identity: Option<PathBuf>,
}

struct Parts {
    connection: Option<String>,
    model: String,
    effort: Option<String>,
}

impl SelectModelAction {
    pub(crate) fn new(data_dir: PathBuf, identity: Option<PathBuf>) -> Self {
        Self { data_dir, identity }
    }

    fn parts(&self, raw: &Value) -> Result<Parts, String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
        let model = object
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| "details.model 必须是非空字符串".to_owned())?;
        let text = |key: &str| -> Result<Option<String>, String> {
            match object.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) if !value.trim().is_empty() => {
                    Ok(Some(value.trim().to_owned()))
                }
                Some(_) => Err(format!("details.{key} 必须是非空字符串（可以省略）")),
            }
        };
        Ok(Parts {
            connection: text("connection")?,
            model: model.to_owned(),
            effort: text("effort")?,
        })
    }

    fn keyring(&self) -> Result<credentials::Keyring, String> {
        credentials::Keyring::open(&self.data_dir, self.identity.as_deref())
            .map_err(|error| format!("读不出 Keyring：{error}"))
    }

    /// 这条对话现在用的连接/模型/档：会话日志最后一条，没有就用 settings.json 的上次选择。
    fn current(&self, env: &ActionEnv<'_>) -> Result<session::ModelSelection, String> {
        session::last_model_selection(env.events)
            .or_else(|| crate::settings::load(&self.data_dir).last_selection)
            .ok_or_else(|| "这条对话还没有选过模型".to_owned())
    }

    fn display_name(
        &self,
        models: &[model::catalog::ModelInfo],
        model_id: &str,
    ) -> String {
        models
            .iter()
            .find(|info| info.id == model_id)
            .map(|info| info.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| model_id.to_owned())
    }
}

impl ActionKind for SelectModelAction {
    fn name(&self) -> &str {
        "select_model"
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let parts = self.parts(raw)?;
        let mut normalized = serde_json::Map::new();
        normalized.insert("model".to_owned(), json!(parts.model));
        if let Some(connection) = parts.connection {
            normalized.insert("connection".to_owned(), json!(connection));
        }
        if let Some(effort) = parts.effort {
            normalized.insert("effort".to_owned(), json!(effort));
        }
        Ok(Value::Object(normalized))
    }

    fn describe(
        &self,
        details: &Value,
        env: &ActionEnv<'_>,
    ) -> Result<Vec<CardFact>, String> {
        let _ = (details, env);
        return Err("换模型还没有实现".to_owned());
        let parts = self.parts(details)?;
        let current = self.current(env)?;
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection_id = parts
            .connection
            .as_deref()
            .unwrap_or(current.connection.as_str());
        let connection = view
            .connections
            .iter()
            .find(|connection| connection.id == connection_id)
            .ok_or_else(|| format!("找不到连接 {connection_id}"))?;
        let models = crate::models::selectable_models(&self.data_dir, connection);
        if !models.iter().any(|info| info.id == parts.model) {
            return Err(format!("这个连接里没有模型 {}", parts.model));
        }
        let before_name = if current.connection == connection_id {
            self.display_name(&models, &current.model)
        } else {
            view.connections
                .iter()
                .find(|connection| connection.id == current.connection)
                .map(|connection| {
                    self.display_name(
                        &crate::models::selectable_models(&self.data_dir, connection),
                        &current.model,
                    )
                })
                .unwrap_or_else(|| current.model.clone())
        };
        let after_name = self.display_name(&models, &parts.model);
        let effort_after = parts
            .effort
            .as_deref()
            .unwrap_or(current.effort_wanted.as_str());
        let mut facts = Vec::new();
        if parts.model != current.model || current.connection != connection_id {
            facts.push(CardFact {
                name: "模型".to_owned(),
                value: format!("{before_name} → {after_name}"),
            });
        }
        if effort_after != current.effort_wanted {
            facts.push(CardFact {
                name: "推理强度".to_owned(),
                value: format!("{} → {effort_after}", current.effort_wanted),
            });
        }
        if facts.is_empty() {
            return Err(format!(
                "这条对话已经用 {after_name}（思考 {}），不用再选",
                current.effort_wanted
            ));
        }
        Ok(facts)
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        let _ = (details, host);
        return Box::pin(async { Err("换模型还没有实现".to_owned()) });
        Box::pin(async move {
            let parts = self.parts(details)?;
            let response = host
                .session_model_select(crate::protocol::SessionModelSelectRequest {
                    connection: parts.connection,
                    model: Some(parts.model),
                    effort_wanted: parts.effort,
                })
                .await
                .map_err(|error| error.to_string())?;
            let name = self
                .keyring()
                .ok()
                .and_then(|keyring| {
                    let view = keyring.view();
                    let connection = view
                        .connections
                        .iter()
                        .find(|connection| connection.id == response.connection)?;
                    Some(self.display_name(
                        &crate::models::selectable_models(&self.data_dir, connection),
                        &response.model,
                    ))
                })
                .unwrap_or_else(|| response.model.clone());
            Ok(ActionOutcome {
                summary: format!(
                    "下一轮用 {name}（思考 {}）",
                    response.effort_actual
                ),
                data: json!({
                    "connection": response.connection,
                    "model": response.model,
                    "effortWanted": response.effort_wanted,
                    "effortActual": response.effort_actual,
                }),
            })
        })
    }
}
