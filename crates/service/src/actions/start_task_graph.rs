//! 确认任务图（属于 aios-tasks）：用户在确认卡上点「确认」后，记下确认并把没有上游的子任务放进 launch。
//!
//! 卡上每项一行事实（标题、等谁、写入范围），由程序从任务库读；图已经确认过就回「已经变了」。

use std::path::PathBuf;

use serde_json::{Value, json};
use tasks::TaskId;

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

/// 确认一张任务图开跑。
pub(crate) struct StartTaskGraphAction {
    data_dir: PathBuf,
}

impl StartTaskGraphAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn task_id(&self, raw: &Value) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get("task_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|task_id| !task_id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.task_id 必须是非空的任务 id".to_owned())
    }

    fn task(&self, task_id: &str) -> Result<tasks::Task, String> {
        let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
            .map_err(|error| format!("任务库读不出来：{error}"))?;
        store
            .task(&TaskId::from(task_id))
            .map_err(|error| format!("任务读不出来：{error}"))?
            .ok_or_else(|| format!("找不到这个任务：{task_id}"))
    }

    /// 卡上的事实：每个子任务一行——标题（名称）、等谁 + 写入范围（值）。
    fn facts(&self, root: &tasks::Task) -> Result<Vec<CardFact>, String> {
        let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
            .map_err(|error| format!("任务库读不出来：{error}"))?;
        let children = store
            .children(&root.id)
            .map_err(|error| format!("子任务读不出来：{error}"))?;
        let mut facts = Vec::with_capacity(children.len());
        for (index, child) in children.iter().enumerate() {
            let waits = if child.needs.is_empty() {
                "无".to_owned()
            } else {
                child
                    .needs
                    .iter()
                    .map(|need| {
                        store
                            .task(&need.from)
                            .ok()
                            .flatten()
                            .map(|task| task.title.clone())
                            .unwrap_or_else(|| format!("任务 {}", need.from))
                    })
                    .collect::<Vec<_>>()
                    .join("、")
            };
            // T053：里程碑那行照实写只读 + 结果由用户判。
            let scope = if child.is_gate() {
                "只读；里程碑，结果由你判".to_owned()
            } else {
                match &child.write_scope {
                    Some(path) => path
                        .strip_prefix(&child.cwd)
                        .unwrap_or(path)
                        .display()
                        .to_string(),
                    None => "只读".to_owned(),
                }
            };
            facts.push(CardFact {
                name: format!("{}. {}", index + 1, child.title),
                value: format!("等：{waits}；写入范围：{scope}"),
            });
        }
        Ok(facts)
    }
}

impl ActionKind for StartTaskGraphAction {
    fn name(&self) -> &str {
        "start_task_graph"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({ "task_id": self.task_id(raw)? }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let root = self.task(&self.task_id(details)?)?;
        if !root.is_graph_root() {
            return Err("这个任务不是任务图的总任务".to_owned());
        }
        let confirmed = root
            .arrangement
            .as_ref()
            .and_then(|arrangement| arrangement.confirmed.as_ref())
            .is_some();
        if confirmed {
            return Err("已经变了".to_owned());
        }
        self.facts(&root)
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let task_id = self.task_id(details)?;
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务执行者没起来，确认不了任务图".to_owned())?;
            let launched = runtime.confirm_graph(&TaskId::from(task_id.clone()))?;
            Ok(ActionOutcome {
                summary: format!("已确认：{launched} 项先开跑，其余等上游结果"),
                data: json!({ "task_id": task_id, "launched": launched }),
            })
        })
    }
}
