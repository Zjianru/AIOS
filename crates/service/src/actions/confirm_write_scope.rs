//! 确认写入范围（属于 aios-tasks）：用户没在消息里说过的路径，先出「写到哪」卡；确认后走同一条派发路径。
//!
//! 卡第一行是绝对路径；带 task_id 的是改已有任务的范围，不是派新任务。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

pub(crate) struct ConfirmWriteScopeAction {
    data_dir: PathBuf,
}

impl ConfirmWriteScopeAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn raw_scope(raw: &Value) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get("write_scope"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.write_scope 必须是非空字符串".to_owned())
    }

    fn task_id(raw: &Value) -> Option<String> {
        raw.as_object()
            .and_then(|object| object.get("task_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    }

    fn absolute(&self, raw: &Value) -> Result<PathBuf, String> {
        let scope = Self::raw_scope(raw)?;
        crate::write_scope::expand(&scope)
    }

    fn task(&self, raw: &Value) -> Result<Value, String> {
        raw.as_object()
            .and_then(|object| object.get("task"))
            .filter(|task| task.is_object())
            .cloned()
            .ok_or_else(|| "details.task 必须是 task_accept 的完整参数对象".to_owned())
    }

    fn task_title(&self, raw: &Value) -> Option<String> {
        raw.get("task")
            .and_then(|task| task.get("title"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
}

impl ActionKind for ConfirmWriteScopeAction {
    fn name(&self) -> &str {
        "confirm_write_scope"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let scope = Self::raw_scope(raw)?;
        crate::write_scope::expand(&scope)?;
        if let Some(task_id) = Self::task_id(raw) {
            return Ok(json!({ "write_scope": scope, "task_id": task_id }));
        }
        let task = self.task(raw)?;
        // 只保留派发要用的字段；write_scope 由这张卡自己的参数决定，避免两份不一致。
        let mut cleaned = task;
        if let Some(object) = cleaned.as_object_mut() {
            object.remove("write_scope");
        }
        Ok(json!({ "write_scope": scope, "task": cleaned }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let absolute = self.absolute(details)?;
        let mut facts = vec![CardFact {
            name: "写到哪".to_owned(),
            value: absolute.display().to_string(),
        }];
        if let Some(task_id) = Self::task_id(details) {
            let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
                .map_err(|error| format!("任务库读不出来：{error}"))?;
            let task = store
                .task(&tasks::TaskId::from(task_id.as_str()))
                .map_err(|error| format!("任务读不出来：{error}"))?
                .ok_or_else(|| format!("找不到这个任务：{task_id}"))?;
            facts.push(CardFact {
                name: "任务".to_owned(),
                value: task.title.clone(),
            });
            facts.push(CardFact {
                name: "范围".to_owned(),
                value: format!(
                    "{} → {}",
                    task.write_scope
                        .as_ref()
                        .map(|scope| scope.display().to_string())
                        .unwrap_or_else(|| "只读".to_owned()),
                    absolute.display()
                ),
            });
        } else {
            if let Some(title) = self.task_title(details) {
                facts.push(CardFact {
                    name: "标题".to_owned(),
                    value: title,
                });
            }
            if let Some(requirement) = details
                .get("task")
                .and_then(|task| task.get("requirement"))
                .and_then(Value::as_str)
            {
                facts.push(CardFact {
                    name: "要求".to_owned(),
                    value: requirement.to_owned(),
                });
            }
        }
        Ok(facts)
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务库没开，不能派任务或改写入范围".to_owned())?;
            let absolute = self.absolute(details)?;
            let scope = absolute.to_string_lossy().into_owned();
            if let Some(task_id) = Self::task_id(details) {
                let summary = runtime
                    .update_scope_from_card(&task_id, &scope)
                    .await
                    .map_err(|error| error.message)?;
                return Ok(ActionOutcome {
                    summary,
                    data: json!({ "taskId": task_id, "writeScope": scope }),
                });
            }
            let session_id = host
                .info()
                .session_id
                .ok_or_else(|| "根会话没开，不能派任务".to_owned())?;
            let task = self.task(details)?;
            let mut dispatch = task
                .as_object()
                .cloned()
                .ok_or_else(|| "details.task 必须是对象".to_owned())?;
            dispatch.insert("write_scope".to_owned(), Value::String(scope.clone()));
            let context = tools::ToolContext::with_session(
                host.cwd(),
                Arc::new(tools::ToolSessionState::new(&self.data_dir, &session_id)),
            );
            let call_id = format!("confirm-write-scope:{scope}");
            let (task, _created) = runtime
                .accept_from_card(&context, &call_id, &Value::Object(dispatch))
                .map_err(|error| error.message)?;
            Ok(ActionOutcome {
                summary: format!("已派出任务 {}（写到 {}）", task.id, absolute.display()),
                data: json!({ "taskId": task.id, "writeScope": scope }),
            })
        })
    }
}
