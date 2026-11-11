//! 接续（属于 aios-tasks）：重启打断的任务由用户确认后，在原会话里开一次新 Attempt。
//!
//! 提议只在「最近 Attempt 被打断了」时成立；点确认前重算，任务已经接着做了就回
//! 「已经接着做了」，这一项不再执行。开会话、写首条恢复说明由任务执行者负责。

use std::path::PathBuf;

use serde_json::{Value, json};
use tasks::TaskId;

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

/// 接续一个被打断的任务。
pub(crate) struct ResumeTaskAction {
    data_dir: PathBuf,
}

impl ResumeTaskAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// details 只有一个非空的任务 id。
    fn task_id(&self, raw: &Value) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get("task_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|task_id| !task_id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.task_id 必须是非空的任务 id".to_owned())
    }

    /// 读任务库里的当前任务；读不出来或没有就是拒绝原因。
    fn task(&self, task_id: &str) -> Result<tasks::Task, String> {
        let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
            .map_err(|error| format!("任务库读不出来：{error}"))?;
        store
            .task(&TaskId::from(task_id))
            .map_err(|error| format!("任务读不出来：{error}"))?
            .ok_or_else(|| format!("找不到这个任务：{task_id}"))
    }
}

impl ActionKind for ResumeTaskAction {
    fn name(&self) -> &str {
        "resume_task"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({ "task_id": self.task_id(raw)? }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let task = self.task(&self.task_id(details)?)?;
        if !task.is_interrupted() {
            return Err("这个任务现在不是被打断了，不用接续".to_owned());
        }
        Ok(vec![CardFact {
            name: "状态".to_owned(),
            value: "被打断了 → 新的 Attempt，接着做".to_owned(),
        }])
    }

    fn reconcile(
        &self,
        details: &Value,
        facts: &[CardFact],
        env: &ActionEnv<'_>,
    ) -> Result<(), String> {
        let task = self.task(&self.task_id(details)?)?;
        // 用户先在任务对话里说了话（或别处确认过）：新的 Attempt 已经在跑，这一项不再执行。
        if !task.is_interrupted() && task.was_interrupted() {
            return Err("已经接着做了".to_owned());
        }
        let current = self.describe(details, env)?;
        if current == facts {
            Ok(())
        } else {
            Err("卡片上的之前和之后已经变了".to_owned())
        }
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
                .ok_or_else(|| "任务执行者没起来，接不了".to_owned())?;
            let attempt = runtime.resume_task(&TaskId::from(task_id.clone())).await?;
            Ok(ActionOutcome {
                summary: "已接着做：新的 Attempt 在原会话里开始了".to_owned(),
                data: json!({ "task_id": task_id, "attempt": attempt.0 }),
            })
        })
    }
}
