//! T057：换程序门的整张命令列表；门在跑先停，再对当前候选重排检查。

use std::path::PathBuf;

use serde_json::{Value, json};
use tasks::{GateCommand, TaskId};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

pub(crate) struct UpdateGateCommandsAction {
    data_dir: PathBuf,
}

impl UpdateGateCommandsAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    fn gate_task_id(&self, raw: &Value) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get("gate_task_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.gate_task_id 必须是非空的门任务 id".to_owned())
    }

    fn commands(&self, raw: &Value) -> Result<Vec<GateCommand>, String> {
        let Some(items) = raw
            .as_object()
            .and_then(|object| object.get("commands"))
            .and_then(Value::as_array)
        else {
            return Err("details.commands 必须是数组".to_owned());
        };
        let mut commands = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            let Some(object) = item.as_object() else {
                return Err(format!("details.commands[{index}] 必须是对象"));
            };
            let Some(command) = object.get("command").and_then(Value::as_str) else {
                return Err(format!("details.commands[{index}].command 不能为空"));
            };
            let timeout_ms = match object.get("timeout_ms") {
                None | Some(Value::Null) => None,
                Some(Value::Number(number)) => Some(
                    number
                        .as_u64()
                        .ok_or_else(|| "timeout_ms 必须是非负整数".to_owned())?,
                ),
                Some(_) => return Err("timeout_ms 必须是整数".to_owned()),
            };
            commands.push(GateCommand {
                command: command.trim().to_owned(),
                timeout_ms,
            });
        }
        tasks::validate_gate_commands(&commands).map_err(|error| error.to_string())?;
        if commands.is_empty() {
            return Err("程序门至少要一条命令".to_owned());
        }
        Ok(commands)
    }

    fn task(&self, gate_id: &str) -> Result<tasks::Task, String> {
        let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
            .map_err(|error| format!("任务库读不出来：{error}"))?;
        store
            .task(&TaskId::from(gate_id))
            .map_err(|error| format!("门任务读不出来：{error}"))?
            .ok_or_else(|| format!("找不到这个门：{gate_id}"))
    }

    fn facts(&self, gate: &tasks::Task) -> Vec<CardFact> {
        let before = gate
            .gate_commands()
            .iter()
            .map(|item| item.command.clone())
            .collect::<Vec<_>>()
            .join("；");
        let current = gate.commands_version();
        vec![
            CardFact {
                name: "命令".to_owned(),
                value: format!("改前：{before}"),
            },
            CardFact {
                name: "命令版本".to_owned(),
                value: format!("第 {current} 版 → 第 {} 版", current + 1),
            },
            CardFact {
                name: "确认后".to_owned(),
                value: "重跑当前候选，旧判定作废".to_owned(),
            },
        ]
    }
}

impl UpdateGateCommandsAction {
    fn card_facts(&self, gate: &tasks::Task, commands: &[GateCommand]) -> Vec<CardFact> {
        let mut facts = self.facts(gate);
        facts[0].value = format!(
            "改前：{}；改后：{}",
            gate.gate_commands()
                .iter()
                .map(|item| item.command.clone())
                .collect::<Vec<_>>()
                .join("；"),
            commands
                .iter()
                .map(|item| item.command.clone())
                .collect::<Vec<_>>()
                .join("；")
        );
        facts
    }
}

impl ActionKind for UpdateGateCommandsAction {
    fn name(&self) -> &str {
        "update_gate_commands"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let commands = self.commands(raw)?;
        Ok(json!({
            "gate_task_id": self.gate_task_id(raw)?,
            "commands": commands.into_iter().map(|item| json!({
                "command": item.command,
                "timeout_ms": item.timeout_ms,
            })).collect::<Vec<_>>(),
        }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let gate = self.task(&self.gate_task_id(details)?)?;
        if !gate.is_program_gate() {
            return Err("这不是程序门；用户判的里程碑门没有命令可改".to_owned());
        }
        let commands = self.commands(details)?;
        if gate.gate_commands() == commands.as_slice() {
            return Err("程序门命令没有变化".to_owned());
        }
        Ok(self.card_facts(&gate, &commands))
    }

    fn reconcile(
        &self,
        details: &Value,
        facts: &[CardFact],
        _env: &ActionEnv<'_>,
    ) -> Result<(), String> {
        let gate = self.task(&self.gate_task_id(details)?)?;
        if !gate.is_program_gate() {
            return Err("这个门已经不是程序门了".to_owned());
        }
        let commands = self.commands(details)?;
        if self.card_facts(&gate, &commands) != facts {
            return Err("命令已经改过了，这张卡是按改之前写的".to_owned());
        }
        Ok(())
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let gate_id = self.gate_task_id(details)?;
            let commands = self.commands(details)?;
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务执行者没起来，改不了程序门命令".to_owned())?;
            let before = self.task(&gate_id)?;
            if !before.is_program_gate() {
                return Err("这不是程序门；用户判的里程碑门没有命令可改".to_owned());
            }
            if before
                .latest_attempt()
                .is_some_and(|attempt| attempt.ended_at.is_none())
            {
                let _ = runtime
                    .stop_task(&gate_id, crate::protocol::StopMode::Now)
                    .await;
                for _ in 0..100 {
                    let ended = self
                        .task(&gate_id)
                        .ok()
                        .and_then(|task| task.latest_attempt().cloned())
                        .is_some_and(|attempt| attempt.ended_at.is_some());
                    if ended {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
            let updated = runtime
                .store()
                .update_gate_commands(&TaskId::from(gate_id.as_str()), commands, now_millis())
                .map_err(|error| error.to_string())?;
            let version = updated.commands_version();
            if updated
                .needs
                .first()
                .is_some_and(|need| need.handoff.is_some())
            {
                runtime.queue_launch(&updated.id)?;
            }
            Ok(ActionOutcome {
                summary: format!(
                    "程序门命令已换成第 {version} 版；已对当前候选重排检查，旧判定作废"
                ),
                data: json!({ "gate_task_id": gate_id, "commands_version": version }),
            })
        })
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
