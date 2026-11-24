//! 改任务要求（属于 aios-tasks）：Kind 按用户要求出确认卡，用户确认后同一个任务开新的
//! Attempt。只改要求与 DoD，不改写集/bash（扩写集仍走 task_update）。

use std::path::PathBuf;

use serde_json::{Value, json};
use tasks::{EffectKind, TaskId};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

/// 改一个任务的要求与 DoD。
pub(crate) struct UpdateTaskRequirementAction {
    data_dir: PathBuf,
}

impl UpdateTaskRequirementAction {
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

    fn quote(&self, raw: &Value) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get("quote"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|quote| !quote.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.quote 必须是非空的用户原话".to_owned())
    }

    fn requirement(&self, raw: &Value) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get("requirement"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|requirement| !requirement.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.requirement 必须是非空的要求".to_owned())
    }

    fn dod(&self, raw: &Value) -> Result<Option<String>, String> {
        match raw.as_object().and_then(|object| object.get("dod")) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(dod)) => {
                let dod = dod.trim();
                Ok((!dod.is_empty()).then(|| dod.to_owned()))
            }
            Some(_) => Err("details.dod 必须是字符串".to_owned()),
        }
    }

    fn constraints(&self, raw: &Value) -> Result<Option<Vec<String>>, String> {
        match raw.as_object().and_then(|object| object.get("constraints")) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => {
                let mut constraints = Vec::with_capacity(items.len());
                for item in items {
                    let Some(text) = item.as_str() else {
                        return Err("details.constraints 里的项必须是字符串".to_owned());
                    };
                    let text = text.trim();
                    if text.is_empty() {
                        return Err("details.constraints 不能含空字符串".to_owned());
                    }
                    constraints.push(text.to_owned());
                }
                Ok(Some(constraints))
            }
            Some(_) => Err("details.constraints 必须是字符串数组".to_owned()),
        }
    }

    fn task(&self, task_id: &str) -> Result<tasks::Task, String> {
        let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
            .map_err(|error| format!("任务库读不出来：{error}"))?;
        store
            .task(&TaskId::from(task_id))
            .map_err(|error| format!("任务读不出来：{error}"))?
            .ok_or_else(|| format!("找不到这个任务：{task_id}"))
    }

    /// 当前任务能不能改要求；能改时返回当前事实用来出卡。
    fn describe_task(
        &self,
        task_id: &str,
        quote: &str,
        requirement: &str,
        dod: Option<&str>,
        constraints: Option<&[String]>,
    ) -> Result<Vec<CardFact>, String> {
        let task = self.task(task_id)?;
        crate::ledger::verify_task_quote(&self.data_dir, task.conversation.as_str(), quote)?;
        if let Some(constraints) = constraints {
            crate::ledger::verify_user_words(
                &self.data_dir,
                task.conversation.as_str(),
                constraints,
            )?;
        }
        let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
            .map_err(|error| format!("任务库读不出来：{error}"))?;
        if store
            .effect_pending(EffectKind::Launch, task.id.as_str())
            .map_err(|error| format!("任务状态读不出来：{error}"))?
        {
            return Err("这个任务还在排队；等它开跑或停下再改要求".to_owned());
        }
        if task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none())
        {
            return Err("这个任务这一轮还在跑；等它结束再改要求".to_owned());
        }
        let main_unchanged = requirement == task.requirement.trim();
        let dod = dod.map(str::trim).filter(|dod| !dod.is_empty());
        let dod_unchanged = dod.is_none_or(|dod| dod == task.dod);
        if main_unchanged && dod_unchanged {
            return Err("要求和 DoD 都没变，不用改".to_owned());
        }
        let current = task.requirement_version();
        let mut understanding = format!("{} → {requirement}", task.requirement);
        if let Some(dod) = dod {
            understanding.push_str(&format!(
                "
DoD：{} → {dod}",
                task.dod
            ));
        }
        Ok(vec![
            CardFact {
                name: "你这次说的".to_owned(),
                value: quote.to_owned(),
            },
            CardFact {
                name: "要求（Kind 的理解）".to_owned(),
                value: understanding,
            },
            CardFact {
                name: "版本".to_owned(),
                value: format!("第 {current} 版 → 第 {} 版，同一个任务接着做", current + 1),
            },
        ])
    }
}

impl ActionKind for UpdateTaskRequirementAction {
    fn name(&self) -> &str {
        "update_task_requirement"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({
            "task_id": self.task_id(raw)?,
            "quote": self.quote(raw)?,
            "requirement": self.requirement(raw)?,
            "dod": self.dod(raw)?,
            "constraints": self.constraints(raw)?,
        }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let task_id = self.task_id(details)?;
        let quote = self.quote(details)?;
        let requirement = self.requirement(details)?;
        let dod = self.dod(details)?;
        let constraints = self.constraints(details)?;
        self.describe_task(
            &task_id,
            &quote,
            &requirement,
            dod.as_deref(),
            constraints.as_deref(),
        )
    }

    fn reconcile(
        &self,
        details: &Value,
        facts: &[CardFact],
        _env: &ActionEnv<'_>,
    ) -> Result<(), String> {
        let task_id = self.task_id(details)?;
        let quote = self.quote(details)?;
        let requirement = self.requirement(details)?;
        let dod = self.dod(details)?;
        let constraints = self.constraints(details)?;
        match self.describe_task(
            &task_id,
            &quote,
            &requirement,
            dod.as_deref(),
            constraints.as_deref(),
        ) {
            Ok(current) if current == facts => Ok(()),
            _ => Err("这个任务的要求已经改过了，这张卡是按改之前写的".to_owned()),
        }
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let task_id = self.task_id(details)?;
            let quote = self.quote(details)?;
            let requirement = self.requirement(details)?;
            let dod = self.dod(details)?;
            let constraints = self.constraints(details)?;
            // 先查再写（顺序/原因照 task_update）：任务在排队、这一轮还在跑、或从没开跑
            // 都不碰任务库，避免「版本先加一版、会话却开不出来」。
            let task = self.task(&task_id)?;
            let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
                .map_err(|error| format!("任务库读不出来：{error}"))?;
            if store
                .effect_pending(EffectKind::Launch, task.id.as_str())
                .map_err(|error| format!("任务状态读不出来：{error}"))?
            {
                return Err("这个任务还在排队；等它开跑或停下再改要求".to_owned());
            }
            if task
                .latest_attempt()
                .is_some_and(|attempt| attempt.ended_at.is_none())
            {
                return Err("这个任务这一轮还在跑；等它结束再改要求".to_owned());
            }
            if task.latest_attempt().is_none() {
                return Err("这个任务还没开跑，没有可接着做的会话；请重新派任务".to_owned());
            }
            let quote_source = crate::ledger::verify_task_quote(
                &self.data_dir,
                task.conversation.as_str(),
                &quote,
            )?;
            if let Some(constraints) = constraints.as_deref() {
                crate::ledger::verify_user_words(
                    &self.data_dir,
                    task.conversation.as_str(),
                    constraints,
                )?;
            }
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务执行者没起来，改不了要求".to_owned())?;
            let summary = runtime
                .update_requirement(
                    &task_id,
                    requirement.clone(),
                    dod,
                    constraints,
                    Some(quote),
                    Some(quote_source),
                    None,
                )
                .await?;
            Ok(ActionOutcome {
                summary,
                data: json!({ "task_id": task_id, "requirement": requirement }),
            })
        })
    }
}
