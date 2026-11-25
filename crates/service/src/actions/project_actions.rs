//! 项目更新确认卡（属于 aios-tasks）：名字/目标/默认目录的之前 → 之后。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

pub(crate) struct ProjectUpdateAction {
    data_dir: PathBuf,
}

impl ProjectUpdateAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn field(raw: &Value, key: &str) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_owned)
            .ok_or_else(|| format!("details.{key} 必须是字符串"))
    }

    fn id(raw: &Value) -> Result<String, String> {
        Self::field(raw, "id")
    }

    fn dir(raw: &Value) -> Result<Option<String>, String> {
        match raw.as_object().and_then(|object| object.get("dir")) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(dir)) if dir.trim().is_empty() => Ok(None),
            Some(Value::String(dir)) => Ok(Some(dir.trim().to_owned())),
            Some(_) => Err("details.dir 必须是字符串或 null".to_owned()),
        }
    }

    fn store(&self) -> Result<tasks::ProjectStore, String> {
        tasks::ProjectStore::open(self.data_dir.join(tasks::PROJECTS_DB_FILE))
            .map_err(|error| format!("项目库读不出来：{error}"))
    }

    fn current(&self, id: &str) -> Result<tasks::Project, String> {
        self.store()?
            .get(id)
            .map_err(|error| format!("项目读不出来：{error}"))?
            .ok_or_else(|| format!("找不到这个项目：{id}"))
    }
}

impl ActionKind for ProjectUpdateAction {
    fn name(&self) -> &str {
        "project_update"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let id = Self::id(raw)?;
        let current = self.current(&id)?;
        let name = raw
            .as_object()
            .and_then(|object| object.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(current.name.as_str())
            .to_owned();
        let goal = raw
            .as_object()
            .and_then(|object| object.get("goal"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|goal| !goal.is_empty())
            .unwrap_or(current.goal.as_str())
            .to_owned();
        let dir = Self::dir(raw)?.or_else(|| {
            current
                .dir
                .as_ref()
                .map(|dir| dir.to_string_lossy().into_owned())
        });
        let store = self.store()?;
        crate::projects::prepare_project(&store, &name, &goal, dir.as_deref(), Some(&id))
            .map_err(|error| error.to_string())?;
        Ok(json!({ "id": id, "name": name, "goal": goal, "dir": dir }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let current = self.current(&Self::id(details)?)?;
        let old_dir = current
            .dir
            .as_ref()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_else(|| "无".to_owned());
        let new_dir = details.get("dir").and_then(Value::as_str).unwrap_or("无");
        Ok(vec![
            CardFact {
                name: "名字".to_owned(),
                value: format!(
                    "{} → {}",
                    current.name,
                    details
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ),
            },
            CardFact {
                name: "目标".to_owned(),
                value: format!(
                    "{} → {}",
                    current.goal,
                    details
                        .get("goal")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ),
            },
            CardFact {
                name: "默认目录".to_owned(),
                value: format!("{old_dir} → {new_dir}"),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let store = host.projects().ok_or_else(|| "项目库没开".to_owned())?;
            let response = crate::projects::update(
                store,
                crate::protocol::ProjectUpdateRequest {
                    id: Self::id(details)?,
                    name: Self::field(details, "name")?,
                    goal: Self::field(details, "goal")?,
                    dir: Self::dir(details)?,
                },
            )
            .map_err(|error| error.to_string())?;
            host.notify_projects_changed();
            Ok(ActionOutcome {
                summary: format!("项目「{}」改好了", response.project.name),
                data: json!({ "project_id": response.project.id }),
            })
        })
    }
}
