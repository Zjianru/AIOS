//! 任务页/任务对话里原来只能由页面做的写动作，登记成确认卡动作。
//!
//! 只做参数校验、事实行和执行入口；真正改任务库/项目库/任务对话的仍是页面同一套
//! 函数（TaskRuntime、projects::set_task_project、RootHost::accept_many / gate_judge）。

use std::path::PathBuf;

use serde_json::{Value, json};
use tasks::TaskId;

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::protocol::{
    GateJudgeConclusion, GateJudgeRequest, TaskAcceptManyRequest, TaskSayRequest,
    TaskSetProjectRequest,
};
use crate::root_lifecycle::RootHost;

fn stage_zh(stage: tasks::Stage) -> &'static str {
    match stage {
        tasks::Stage::NeedsConditions => "待补足条件",
        tasks::Stage::Pending => "待执行",
        tasks::Stage::Running => "执行中",
        tasks::Stage::ReadyForReview => "待检查",
        tasks::Stage::RequirementMet => "任务要求已满足",
        tasks::Stage::Closed => "已关闭",
    }
}

fn task_id(raw: &Value) -> Result<String, String> {
    raw.as_object()
        .and_then(|object| object.get("task_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "details.task_id 必须是非空的任务 id".to_owned())
}

fn task_text(raw: &Value) -> Result<String, String> {
    raw.as_object()
        .and_then(|object| object.get("text").or_else(|| object.get("message")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "details.text 必须是非空的用户原话".to_owned())
}

fn task_store(data_dir: &std::path::Path) -> Result<tasks::Store, String> {
    tasks::Store::open(data_dir.join(crate::TASKS_DB_FILE))
        .map_err(|error| format!("任务库读不出来：{error}"))
}

fn task(store: &tasks::Store, id: &str) -> Result<tasks::Task, String> {
    store
        .task(&TaskId::from(id))
        .map_err(|error| format!("任务读不出来：{error}"))?
        .ok_or_else(|| format!("找不到这个任务：{id}"))
}

fn project_store(data_dir: &std::path::Path) -> Result<tasks::ProjectStore, String> {
    tasks::ProjectStore::open(data_dir.join(tasks::PROJECTS_DB_FILE))
        .map_err(|error| format!("项目库读不出来：{error}"))
}

/// 去掉首尾空白；空字符串当没有（个人）。
fn optional_project(raw: &Value) -> Result<Option<String>, String> {
    let Some(value) = raw.as_object().and_then(|object| object.get("project_id")) else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::String(id) => {
            let id = id.trim();
            if id.is_empty() {
                Ok(None)
            } else {
                Ok(Some(id.to_owned()))
            }
        }
        _ => Err("details.project_id 必须是项目 id 或 null".to_owned()),
    }
}

pub(crate) struct TaskInterruptAction {
    data_dir: PathBuf,
}

impl TaskInterruptAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

impl ActionKind for TaskInterruptAction {
    fn name(&self) -> &str {
        "task_interrupt"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({ "task_id": task_id(raw)? }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let store = task_store(&self.data_dir)?;
        let task = task(&store, &task_id(details)?)?;
        let stage = store
            .stage(&task.id)
            .map_err(|error| format!("任务阶段读不出来：{error}"))?;
        let running = task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none());
        Ok(vec![
            CardFact {
                name: "任务".to_owned(),
                value: task.title,
            },
            CardFact {
                name: "状态".to_owned(),
                value: format!(
                    "{}（{}）",
                    stage_zh(stage),
                    if running { "在跑" } else { "没在跑" }
                ),
            },
            CardFact {
                name: "动作".to_owned(),
                value: "停下当前这一轮".to_owned(),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务执行者没起来，停不了".to_owned())?;
            let response = runtime
                .interrupt_task(&task_id(details)?)
                .await
                .map_err(|error| error.to_string())?;
            Ok(ActionOutcome {
                summary: response.message,
                data: json!({ "interrupted": response.interrupted }),
            })
        })
    }
}

pub(crate) struct TaskSayAction {
    data_dir: PathBuf,
}

impl TaskSayAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

impl ActionKind for TaskSayAction {
    fn name(&self) -> &str {
        "task_say"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({
            "task_id": task_id(raw)?,
            "text": task_text(raw)?,
        }))
    }

    fn describe(&self, details: &Value, env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let id = task_id(details)?;
        let text = task_text(details)?;
        let conversation = env
            .conversation
            .ok_or_else(|| "没有发起这次对话的会话，核不了原话".to_owned())?;
        if crate::ledger::find_user_words(env.data_dir, conversation, &text)?.is_none() {
            return Err(format!(
                "task_say.text 要用用户原话：『{text}』不在用户消息里；要改要求请用 update_task_requirement。"
            ));
        }
        let store = task_store(&self.data_dir)?;
        let task = task(&store, &id)?;
        Ok(vec![
            CardFact {
                name: "原话".to_owned(),
                value: text,
            },
            CardFact {
                name: "任务".to_owned(),
                value: task.title,
            },
            CardFact {
                name: "送进".to_owned(),
                value: "任务对话".to_owned(),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务执行者没起来，说不了".to_owned())?;
            let request = TaskSayRequest {
                task_id: task_id(details)?,
                text: task_text(details)?,
                pastes: Vec::new(),
                attachments: Vec::new(),
            };
            let response = runtime
                .say_task(request)
                .await
                .map_err(|error| error.to_string())?;
            Ok(ActionOutcome {
                summary: "已经送进任务对话".to_owned(),
                data: json!({
                    "input_id": response.input_id,
                    "new_attempt": response.new_attempt,
                }),
            })
        })
    }
}

pub(crate) struct TaskSetProjectAction {
    data_dir: PathBuf,
}

impl TaskSetProjectAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

impl ActionKind for TaskSetProjectAction {
    fn name(&self) -> &str {
        "task_set_project"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({
            "task_id": task_id(raw)?,
            "project_id": optional_project(raw)?,
        }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let store = task_store(&self.data_dir)?;
        let task = task(&store, &task_id(details)?)?;
        let projects = project_store(&self.data_dir)?;
        let new_project = optional_project(details)?;
        if let Some(project_id) = new_project.as_deref()
            && projects
                .get(project_id)
                .map_err(|error| format!("项目读不出来：{error}"))?
                .is_none()
        {
            return Err(format!("找不到这个项目：{project_id}"));
        }
        let project_name = |id: Option<&str>| -> Result<String, String> {
            match id {
                None => Ok("个人".to_owned()),
                Some(id) => projects
                    .get(id)
                    .map_err(|error| format!("项目读不出来：{error}"))?
                    .map(|project| project.name)
                    .ok_or_else(|| format!("找不到这个项目：{id}")),
            }
        };
        let before = project_name(task.project_id.as_deref())?;
        let after = project_name(new_project.as_deref())?;
        Ok(vec![
            CardFact {
                name: "任务".to_owned(),
                value: task.title,
            },
            CardFact {
                name: "项目".to_owned(),
                value: format!("{before} → {after}"),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务执行者没起来，改不了归属".to_owned())?;
            let projects = host
                .projects()
                .ok_or_else(|| "项目库没开，改不了归属".to_owned())?;
            let response = crate::projects::set_task_project(
                runtime,
                projects,
                TaskSetProjectRequest {
                    task_id: task_id(details)?,
                    project_id: optional_project(details)?,
                },
            )
            .map_err(|error| error.to_string())?;
            Ok(ActionOutcome {
                summary: format!("任务归属已改，连带 {} 个任务", response.changed.len()),
                data: json!({ "task_id": response.task_id, "project_id": response.project_id, "changed": response.changed }),
            })
        })
    }
}

pub(crate) struct AcceptDeliveriesAction {
    data_dir: PathBuf,
}

impl AcceptDeliveriesAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn task_ids(&self, raw: &Value) -> Result<Vec<String>, String> {
        let Some(ids) = raw.as_object().and_then(|object| {
            object
                .get("task_ids")
                .or_else(|| object.get("deliveries"))
                .and_then(Value::as_array)
        }) else {
            return Err("details.task_ids 必须是非空数组".to_owned());
        };
        let mut out = Vec::new();
        for item in ids {
            let id = match item {
                Value::String(id) => Some(id.as_str()),
                Value::Object(object) => object.get("task_id").and_then(Value::as_str),
                _ => None,
            };
            let Some(id) = id.map(str::trim).filter(|id| !id.is_empty()) else {
                return Err("details.task_ids 里每项都必须是非空任务 id".to_owned());
            };
            if !out.iter().any(|existing: &String| existing == id) {
                out.push(id.to_owned());
            }
        }
        if out.is_empty() {
            return Err("details.task_ids 必须至少有一项".to_owned());
        }
        Ok(out)
    }

    fn current_picks(&self, ids: &[String]) -> Result<Vec<crate::work::WorkPick>, String> {
        let store = task_store(&self.data_dir)?;
        let mut picks = Vec::new();
        for id in ids {
            let report = store
                .releasable_report(&TaskId::from(id.as_str()))
                .map_err(|error| format!("报告读不出来：{error}"))?
                .ok_or_else(|| format!("这个任务还没有可接受的交付：{id}"))?;
            picks.push(crate::work::WorkPick {
                task_id: id.clone(),
                report_id: report.id.as_str().to_owned(),
                version: report.version,
            });
        }
        Ok(picks)
    }

    fn parse_picks(value: &Value) -> Result<Vec<crate::work::WorkPick>, String> {
        let Some(items) = value.as_array() else {
            return Err("details.picks 必须是数组".to_owned());
        };
        let mut picks = Vec::with_capacity(items.len());
        for item in items {
            let Some(object) = item.as_object() else {
                return Err("details.picks 里每项必须是对象".to_owned());
            };
            let task_id = object
                .get("task_id")
                .or_else(|| object.get("taskId"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "details.picks 缺 task_id".to_owned())?;
            let report_id = object
                .get("report_id")
                .or_else(|| object.get("reportId"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "details.picks 缺 report_id".to_owned())?;
            let version = object
                .get("version")
                .and_then(Value::as_u64)
                .and_then(|version| u32::try_from(version).ok())
                .ok_or_else(|| "details.picks 缺 version".to_owned())?;
            picks.push(crate::work::WorkPick {
                task_id: task_id.to_owned(),
                report_id: report_id.to_owned(),
                version,
            });
        }
        if picks.is_empty() {
            return Err("details.picks 必须至少有一项".to_owned());
        }
        Ok(picks)
    }

    fn picks_json(picks: &[crate::work::WorkPick]) -> Value {
        json!(
            picks
                .iter()
                .map(|pick| json!({
                    "task_id": pick.task_id,
                    "report_id": pick.report_id,
                    "version": pick.version,
                }))
                .collect::<Vec<_>>()
        )
    }

    /// details.picks 是提议那一刻钉住的报告；没有时按 task_ids 解析当前报告（旧卡/直接调用）。
    fn picks(&self, details: &Value) -> Result<Vec<crate::work::WorkPick>, String> {
        if let Some(value) = details
            .as_object()
            .and_then(|object| object.get("picks"))
            .filter(|value| !value.is_null())
        {
            return Self::parse_picks(value);
        }
        let ids = self.task_ids(details)?;
        self.current_picks(&ids)
    }

    /// act 只接受提议时钉住的 picks；没有钉住就不执行，避免拿当前报告顶替卡上的报告。
    fn pinned_picks(&self, details: &Value) -> Result<Vec<crate::work::WorkPick>, String> {
        let Some(value) = details
            .as_object()
            .and_then(|object| object.get("picks"))
            .filter(|value| !value.is_null())
        else {
            return Err("卡片上的报告已经变了，或者没有钉住，请重新出卡".to_owned());
        };
        Self::parse_picks(value)
    }

    fn check_still_current(&self, picks: &[crate::work::WorkPick]) -> Result<(), String> {
        let store = task_store(&self.data_dir)?;
        for pick in picks {
            let report = store
                .releasable_report(&TaskId::from(pick.task_id.as_str()))
                .map_err(|error| format!("报告读不出来：{error}"))?
                .ok_or_else(|| "卡片上的报告已经变了，请重新出卡".to_owned())?;
            if report.id.as_str() != pick.report_id || report.version != pick.version {
                return Err("卡片上的报告已经变了，请重新出卡".to_owned());
            }
        }
        Ok(())
    }
}

impl ActionKind for AcceptDeliveriesAction {
    fn name(&self) -> &str {
        "accept_deliveries"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        if raw
            .as_object()
            .and_then(|object| object.get("picks"))
            .filter(|value| !value.is_null())
            .is_some()
        {
            let picks = self.picks(raw)?;
            return Ok(json!({ "picks": Self::picks_json(&picks) }));
        }
        let ids = self.task_ids(raw)?;
        let picks = self.current_picks(&ids)?;
        Ok(json!({ "picks": Self::picks_json(&picks) }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let picks = self.picks(details)?;
        self.check_still_current(&picks)?;
        let store = task_store(&self.data_dir)?;
        let mut first = Vec::new();
        let mut facts = Vec::new();
        for pick in &picks {
            let task = task(&store, &pick.task_id)?;
            first.push(format!(
                "接受「{}」（报告 {}）的交付",
                task.title, pick.report_id
            ));
            facts.push(CardFact {
                name: task.title,
                value: format!("报告 {} · 第 {} 版", pick.report_id, pick.version),
            });
        }
        let mut out = vec![CardFact {
            name: "这是你的判断".to_owned(),
            value: first.join("；"),
        }];
        out.append(&mut facts);
        Ok(out)
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let picks = self.pinned_picks(details)?;
            self.check_still_current(&picks)?;
            let response = host
                .accept_many(&TaskAcceptManyRequest { picks })
                .await
                .map_err(|error| error.to_string())?;
            let refused: Vec<String> = response
                .outcomes
                .iter()
                .filter_map(|outcome| outcome.refused.clone())
                .collect();
            let summary = if refused.is_empty() {
                format!("接受了 {} 份交付", response.outcomes.len())
            } else {
                format!(
                    "接受 {} 份，{} 份没接受：{}",
                    response.outcomes.len() - refused.len(),
                    refused.len(),
                    refused.join("；")
                )
            };
            Ok(ActionOutcome {
                summary,
                data: json!({ "accepted_at": response.accepted_at, "outcomes": response.outcomes }),
            })
        })
    }
}

pub(crate) struct GateJudgeAction {
    data_dir: PathBuf,
}

impl GateJudgeAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn gate_task(details: &Value) -> Result<String, String> {
        details
            .as_object()
            .and_then(|object| {
                object
                    .get("gate_task")
                    .or_else(|| object.get("gate_task_id"))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.gate_task 必须是非空的门任务 id".to_owned())
    }

    fn gate_report(details: &Value) -> Result<String, String> {
        details
            .as_object()
            .and_then(|object| {
                object
                    .get("gate_report")
                    .or_else(|| object.get("report_id"))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.gate_report 必须是非空的报告 id".to_owned())
    }

    fn conclusion(details: &Value) -> Result<GateJudgeConclusion, String> {
        match details
            .as_object()
            .and_then(|object| object.get("conclusion").or_else(|| object.get("verdict")))
            .and_then(Value::as_str)
        {
            Some("pass" | "通过") => Ok(GateJudgeConclusion::Pass),
            Some("needs_fix" | "需修正" | "不通过") => Ok(GateJudgeConclusion::NeedsFix),
            _ => Err("details.conclusion 必须是 pass 或 needs_fix".to_owned()),
        }
    }

    fn note(details: &Value) -> Option<String> {
        details
            .as_object()
            .and_then(|object| object.get("note"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(str::to_owned)
    }
}

impl ActionKind for GateJudgeAction {
    fn name(&self) -> &str {
        "gate_judge"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let conclusion = match Self::conclusion(raw)? {
            GateJudgeConclusion::Pass => "pass",
            GateJudgeConclusion::NeedsFix => "needs_fix",
        };
        Ok(json!({
            "gate_task": Self::gate_task(raw)?,
            "gate_report": Self::gate_report(raw)?,
            "conclusion": conclusion,
            "note": Self::note(raw),
        }))
    }

    fn describe(&self, details: &Value, env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let store = task_store(&self.data_dir)?;
        let gate = task(&store, &Self::gate_task(details)?)?;
        if !gate.is_gate() {
            return Err("这个任务不是门，不能判".to_owned());
        }
        let conclusion = Self::conclusion(details)?;
        if let Some(note) = Self::note(details) {
            let conversation = env
                .conversation
                .ok_or_else(|| "没有发起这次对话的会话，核不了判定理由".to_owned())?;
            if crate::ledger::find_user_words(env.data_dir, conversation, &note)?.is_none() {
                return Err(format!(
                    "gate_judge.note 要用用户原话：『{note}』不在用户消息里。"
                ));
            }
        }
        let pass = conclusion == GateJudgeConclusion::Pass;
        let mut facts = vec![CardFact {
            name: "这是你的判断".to_owned(),
            value: format!(
                "判「{}」{}",
                gate.title,
                if pass {
                    "通过"
                } else {
                    "不通过（需修正）"
                }
            ),
        }];
        facts.push(CardFact {
            name: "门".to_owned(),
            value: gate.title.clone(),
        });
        facts.push(CardFact {
            name: "报告".to_owned(),
            value: Self::gate_report(details)?,
        });
        if let Some(note) = Self::note(details) {
            facts.push(CardFact {
                name: "理由（用户原话）".to_owned(),
                value: note,
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
            let request = GateJudgeRequest {
                gate_task: Self::gate_task(details)?,
                gate_report: Self::gate_report(details)?,
                conclusion: Self::conclusion(details)?,
                note: Self::note(details),
            };
            let response = host
                .gate_judge(request)
                .await
                .map_err(|error| error.to_string())?;
            if response.changed || response.verdict.is_none() {
                return Err(response.summary);
            }
            Ok(ActionOutcome {
                summary: response.summary,
                data: json!({
                    "duplicate": response.duplicate,
                    "verdict": response.verdict,
                    "released": response.released,
                }),
            })
        })
    }
}
