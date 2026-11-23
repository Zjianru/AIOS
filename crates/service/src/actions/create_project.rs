//! 确认卡建项目（属于 aios-tasks）：卡上只放名字/目标/默认目录，用户点确认才建。
//! 确认前同名项目已经存在时，不重复建，回一句已经建过了。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

pub(crate) struct CreateProjectAction {
    data_dir: PathBuf,
}

impl CreateProjectAction {
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

    fn normalized(&self, raw: &Value) -> Result<tasks::NewProject, String> {
        let name = Self::field(raw, "name")?;
        let goal = Self::field(raw, "goal")?;
        let dir = raw
            .as_object()
            .and_then(|object| object.get("dir"))
            .and_then(Value::as_str);
        crate::projects::normalize_project_fields(&name, &goal, dir)
    }

    fn project(&self, name: &str) -> Result<Option<tasks::Project>, String> {
        let store = tasks::ProjectStore::open(self.data_dir.join(tasks::PROJECTS_DB_FILE))
            .map_err(|error| format!("项目库打不开：{error}"))?;
        let projects = store
            .list()
            .map_err(|error| format!("项目读不出来：{error}"))?;
        Ok(projects.into_iter().find(|project| project.name == name))
    }
}

impl ActionKind for CreateProjectAction {
    fn name(&self) -> &str {
        "create_project"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let project = self.normalized(raw)?;
        Ok(json!({
            "name": project.name,
            "goal": project.goal,
            "dir": project.dir.map(|dir| dir.to_string_lossy().into_owned()),
        }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let project = self.normalized(details)?;
        Ok(vec![
            CardFact {
                name: "名字".to_owned(),
                value: project.name,
            },
            CardFact {
                name: "目标".to_owned(),
                value: project.goal,
            },
            CardFact {
                name: "默认目录".to_owned(),
                value: project
                    .dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "无".to_owned()),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        _host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let project = self.normalized(details)?;
            if let Some(existing) = self.project(&project.name)? {
                return Ok(ActionOutcome {
                    summary: format!(
                        "已经建过了：项目 {}「{}」。默认目录{}，派任务时预填。",
                        existing.id,
                        existing.name,
                        existing
                            .dir
                            .as_ref()
                            .map(|dir| dir.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "无".to_owned())
                    ),
                    data: json!({ "project_id": existing.id, "name": existing.name }),
                });
            }
            let store = tasks::ProjectStore::open(self.data_dir.join(tasks::PROJECTS_DB_FILE))
                .map_err(|error| format!("项目库打不开：{error}"))?;
            let created = crate::projects::new_project(
                &store,
                &project.name,
                &project.goal,
                project
                    .dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .as_deref(),
            )
            .map_err(|error| error.to_string())?;
            Ok(ActionOutcome {
                summary: format!(
                    "已建：项目 {}「{}」。默认目录{}，派任务时预填。",
                    created.id,
                    created.name,
                    created
                        .dir
                        .as_ref()
                        .map(|dir| dir.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "无".to_owned())
                ),
                data: json!({ "project_id": created.id, "name": created.name }),
            })
        })
    }
}
