//! T077.5：取消任务（属于 aios-tasks）。任务页菜单和 Kind 确认卡共用同一个登记动作：
//! details 是一组 task id；describe 读当前阶段和连带下游；act 逐项取消，不全会或全无。

use std::path::PathBuf;

use serde_json::{Value, json};
use tasks::{TaskError, TaskId};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

/// 和 web/src/pages/work/workModel.ts 的 STAGE_LABEL 同一张表；G1 共用夹具两边断言。
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

pub(crate) struct CancelTasksAction {
    data_dir: PathBuf,
}

impl CancelTasksAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn task_ids(&self, raw: &Value) -> Result<Vec<String>, String> {
        let Some(ids) = raw
            .as_object()
            .and_then(|object| object.get("task_ids"))
            .and_then(Value::as_array)
        else {
            return Err("details.task_ids 必须是非空数组".to_owned());
        };
        let mut out = Vec::new();
        for id in ids {
            let Some(id) = id.as_str().map(str::trim).filter(|id| !id.is_empty()) else {
                return Err("details.task_ids 里每项都必须是非空任务 id".to_owned());
            };
            if out.iter().any(|existing: &String| existing == id) {
                continue;
            }
            out.push(id.to_owned());
        }
        if out.is_empty() {
            return Err("details.task_ids 必须至少有一项".to_owned());
        }
        Ok(out)
    }

    fn store(&self) -> Result<tasks::Store, String> {
        tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
            .map_err(|error| format!("任务库读不出来：{error}"))
    }

    fn task(&self, store: &tasks::Store, id: &str) -> Result<tasks::Task, String> {
        store
            .task(&TaskId::from(id))
            .map_err(|error| format!("任务读不出来：{error}"))?
            .ok_or_else(|| format!("找不到这个任务：{id}"))
    }
}

impl ActionKind for CancelTasksAction {
    fn name(&self) -> &str {
        "cancel_tasks"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({ "task_ids": self.task_ids(raw)? }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let ids = self.task_ids(details)?;
        let store = self.store()?;
        let mut facts = Vec::new();
        for id in &ids {
            let Ok(task) = self.task(&store, id) else {
                facts.push(CardFact {
                    name: id.clone(),
                    value: "找不到这个任务".to_owned(),
                });
                continue;
            };
            if task.cancelled.is_some() {
                facts.push(CardFact {
                    name: task.title.clone(),
                    value: "已取消；不会重复取消".to_owned(),
                });
                continue;
            }
            let stage = store.stage(&task.id).map_err(|error| error.to_string())?;
            if stage == tasks::Stage::Closed {
                facts.push(CardFact {
                    name: task.title.clone(),
                    value: "已关闭；不会取消".to_owned(),
                });
                continue;
            }
            let scope = store
                .cancel_scope(&task.id)
                .map_err(|error| error.to_string())?;
            let cascaded: Vec<&str> = scope
                .iter()
                .skip(1)
                .map(|task| task.title.as_str())
                .collect();
            facts.push(CardFact {
                name: task.title.clone(),
                value: format!(
                    "阶段：{}；连带下游：{}",
                    stage_zh(stage),
                    if cascaded.is_empty() {
                        "无".to_owned()
                    } else {
                        cascaded.join("、")
                    }
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
            let ids = self.task_ids(details)?;
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务执行者没起来，取消不了".to_owned())?;
            let mut cancelled = Vec::new();
            let mut refused = Vec::new();
            for id in ids {
                let task_id = TaskId::from(id.as_str());
                let Ok(task) = self.task(&self.store()?, id.as_str()) else {
                    refused.push(json!({ "task_id": id, "reason": "找不到" }));
                    continue;
                };
                if task.cancelled.is_some() {
                    refused.push(json!({ "task_id": id, "reason": "已取消" }));
                    continue;
                }
                if self
                    .store()?
                    .stage(&task_id)
                    .map_err(|error| error.to_string())?
                    == tasks::Stage::Closed
                {
                    refused.push(json!({ "task_id": id, "reason": "已关闭" }));
                    continue;
                }
                match runtime.cancel_with_downstream(
                    &task_id,
                    tasks::CancellationSource::Kind,
                    None,
                    now_ms(),
                ) {
                    Ok(items) => {
                        for (cancelled_id, _) in items {
                            cancelled.push(cancelled_id.as_str().to_owned());
                        }
                    }
                    Err(TaskError::AlreadyClosed) => {
                        refused.push(json!({ "task_id": id, "reason": "已关闭" }))
                    }
                    Err(TaskError::Cancelled) => {
                        refused.push(json!({ "task_id": id, "reason": "已取消" }))
                    }
                    Err(error) => {
                        refused.push(json!({ "task_id": id, "reason": error.to_string() }))
                    }
                }
            }
            let summary = if refused.is_empty() {
                format!("已取消 {} 项。", cancelled.len())
            } else {
                format!(
                    "取消了 {} 项，没取消 {} 项：{}",
                    cancelled.len(),
                    refused.len(),
                    refused
                        .iter()
                        .filter_map(|item| item.get("reason").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("；")
                )
            };
            Ok(ActionOutcome {
                summary,
                data: json!({ "cancelled": cancelled, "refused": refused }),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn store(data: &std::path::Path) -> tasks::Store {
        tasks::Store::open(data.join(crate::TASKS_DB_FILE)).unwrap()
    }

    fn draft() -> tasks::NewTask {
        tasks::NewTask {
            quote: None,
            quote_source: None,
            kind_note: None,
            project_id: None,
            cwd_source: None,
            title: "要取消的任务".to_owned(),
            requirement: "做".to_owned(),
            dod: "做完".to_owned(),
            cwd: PathBuf::from("/tmp/cancel-action"),
            conversation: tasks::ConversationId::new("kind-cancel-action"),
            model: tasks::ModelChoice {
                connection: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                effort_wanted: "high".to_owned(),
            },
            tools: vec!["read".to_owned()],
        }
    }

    #[test]
    fn details_rejects_empty_and_dedupes_task_ids() {
        let action = CancelTasksAction::new(PathBuf::from("/tmp"));
        assert!(
            action
                .details(&json!({"task_ids": []}))
                .unwrap_err()
                .contains("至少有一项")
        );
        let details = action
            .details(&json!({"task_ids": ["a", "a", "b"]}))
            .unwrap();
        assert_eq!(details["task_ids"], json!(["a", "b"]));
    }

    #[test]
    fn describe_fills_title_stage_and_cascade_scope() {
        let data = tempfile::tempdir().unwrap();
        let store = store(data.path());
        let task = store.create_task(draft(), 1_700_000_000_000).unwrap();
        let action = CancelTasksAction::new(data.path().to_path_buf());
        let env = ActionEnv {
            data_dir: data.path(),
            identity: None,
            events: &[],
            conversation: None,
        };
        let facts = action
            .describe(&json!({"task_ids": [task.id.as_str()]}), &env)
            .unwrap();
        assert_eq!(facts.len(), 1);
        assert!(facts[0].name.contains("要取消的任务"));
        assert!(facts[0].value.contains("连带下游"));

        // 混合 active + 已关闭 + 已取消：不整张卡报错，逐项标出原因。
        let done = store.create_task(draft(), 1_700_000_001_000).unwrap();
        let attempt = store.begin_attempt(&done.id, 1_700_000_001_001).unwrap();
        let report = store
            .record_report(
                &done.id,
                attempt,
                tasks::NewReport {
                    state: tasks::ReportState::Ready,
                    origin: tasks::ReportOrigin::Agent,
                    summary: "已完成".to_owned(),
                    question: None,
                },
                1_700_000_001_002,
            )
            .unwrap();
        store.accept(&done.id, &report, 1_700_000_001_003).unwrap();
        let gone = store.create_task(draft(), 1_700_000_002_000).unwrap();
        store
            .cancel(
                &gone.id,
                tasks::CancellationSource::User,
                None,
                1_700_000_002_001,
            )
            .unwrap();
        let mixed = action
            .describe(
                &json!({"task_ids": [task.id.as_str(), done.id.as_str(), gone.id.as_str()]}),
                &env,
            )
            .unwrap();
        assert_eq!(mixed.len(), 3);
        assert!(mixed[1].value.contains("已关闭"));
        assert!(mixed[2].value.contains("已取消"));
    }

    #[test]
    fn stage_labels_match_the_shared_web_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../web/src/pages/work/work-grouping.fixture.json"
        ))
        .unwrap();
        let labels = fixture["stageLabels"].as_object().unwrap();
        for (key, label) in labels {
            let stage = match key.as_str() {
                "needs_conditions" => tasks::Stage::NeedsConditions,
                "pending" => tasks::Stage::Pending,
                "running" => tasks::Stage::Running,
                "ready_for_review" => tasks::Stage::ReadyForReview,
                "requirement_met" => tasks::Stage::RequirementMet,
                "closed" => tasks::Stage::Closed,
                other => panic!("夹具里有未知阶段：{other}"),
            };
            assert_eq!(stage_zh(stage), label.as_str().unwrap(), "阶段 {key}");
        }
    }
}
