//! 确认卡上的建/改约定（属于 aios-calendar）。
//!
//! 事实由服务端按当前状态填：建给原话、触发、绑定、下次触发；改给触发的之前→之后和版本。
//! 改带 expectedVersion，点确认时版本对不上就拒（卡片上的之前和之后已经变了）。

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};

fn store(env: &ActionEnv<'_>) -> Result<calendar::CalendarStore, String> {
    calendar::CalendarStore::open(env.data_dir.join("calendar.sqlite"))
        .map_err(|error| format!("日历库读不出来：{error}"))
}

fn schedule_fact(rule: &str, timezone: &str, start_at: i64) -> String {
    let schedule = calendar::Schedule {
        rule: rule.to_owned(),
        timezone: timezone.to_owned(),
        start_at,
    };
    if schedule.validate().is_err() {
        return format!("{rule}（{timezone}）");
    }
    let next = schedule
        .next_after(now_ms())
        .ok()
        .flatten()
        .map(format_ms)
        .unwrap_or_else(|| "不再触发".to_owned());
    format!("{rule}（{timezone}），下次 {next}")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn format_ms(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| at.to_string())
}

fn source_fact(details: &Value) -> Result<String, String> {
    let binding = match details.get("binding").and_then(Value::as_str) {
        Some("task") => format!(
            "任务 {}",
            details.get("taskId").and_then(Value::as_str).unwrap_or("")
        ),
        _ => "Kind".to_owned(),
    };
    let watch = details
        .get("watch")
        .and_then(|watch| {
            Some(format!(
                "{}:{}",
                watch.get("kind").and_then(Value::as_str)?,
                watch.get("target").and_then(Value::as_str)?
            ))
        })
        .unwrap_or_else(|| "不盯".to_owned());
    Ok(format!("{binding}；盯变化 {watch}"))
}

pub(crate) struct CreateAppointmentAction;

impl ActionKind for CreateAppointmentAction {
    fn name(&self) -> &str {
        "create_appointment"
    }

    fn skill(&self) -> &str {
        "aios-calendar"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
        for field in ["title", "condition", "rule"] {
            object
                .get(field)
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| format!("details.{field} 必须是非空字符串"))?;
        }
        object
            .get("startAt")
            .and_then(Value::as_i64)
            .ok_or_else(|| "details.startAt 必须是毫秒时间戳".to_owned())?;
        Ok(raw.clone())
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let rule = details
            .get("rule")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let timezone = details
            .get("timezone")
            .and_then(Value::as_str)
            .unwrap_or("Asia/Shanghai");
        let start_at = details
            .get("startAt")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        Ok(vec![
            CardFact {
                name: "原话".to_owned(),
                value: details
                    .get("condition")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            CardFact {
                name: "触发".to_owned(),
                value: schedule_fact(rule, timezone, start_at),
            },
            CardFact {
                name: "绑定".to_owned(),
                value: source_fact(details)?,
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a crate::root_lifecycle::RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务库没开，建不了约定".to_owned())?;
            let appointment = runtime.calendar_create(details)?;
            Ok(ActionOutcome {
                summary: format!("建好约定「{}」", appointment.title),
                data: json!({ "appointmentId": appointment.id }),
            })
        })
    }
}

pub(crate) struct UpdateAppointmentAction;

impl ActionKind for UpdateAppointmentAction {
    fn name(&self) -> &str {
        "update_appointment"
    }

    fn skill(&self) -> &str {
        "aios-calendar"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
        object
            .get("appointmentId")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| "details.appointmentId 必须是非空字符串".to_owned())?;
        object
            .get("expectedVersion")
            .and_then(Value::as_u64)
            .ok_or_else(|| "details.expectedVersion 必须是正整数".to_owned())?;
        Ok(raw.clone())
    }

    fn describe(&self, details: &Value, env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let store = store(env)?;
        let id = details
            .get("appointmentId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let current = store
            .appointment(id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("约定不存在：{id}"))?;
        let rule = details
            .get("rule")
            .and_then(Value::as_str)
            .unwrap_or(&current.schedule.rule);
        let timezone = details
            .get("timezone")
            .and_then(Value::as_str)
            .unwrap_or(&current.schedule.timezone);
        let start_at = details
            .get("startAt")
            .and_then(Value::as_i64)
            .unwrap_or(current.schedule.start_at);
        let mut facts = vec![
            CardFact {
                name: "触发".to_owned(),
                value: format!(
                    "{} → {}",
                    schedule_fact(
                        &current.schedule.rule,
                        &current.schedule.timezone,
                        current.schedule.start_at
                    ),
                    schedule_fact(rule, timezone, start_at)
                ),
            },
            CardFact {
                name: "版本".to_owned(),
                value: format!("第 {} 版 → 第 {} 版", current.version, current.version + 1),
            },
        ];
        if let Some(title) = details.get("title").and_then(Value::as_str) {
            facts.insert(
                0,
                CardFact {
                    name: "标题".to_owned(),
                    value: format!("{} → {title}", current.title),
                },
            );
        }
        if let Some(condition) = details.get("condition").and_then(Value::as_str) {
            facts.push(CardFact {
                name: "原话".to_owned(),
                value: format!("{} → {condition}", current.condition),
            });
        }
        Ok(facts)
    }

    fn reconcile(
        &self,
        details: &Value,
        _facts: &[CardFact],
        env: &ActionEnv<'_>,
    ) -> Result<(), String> {
        let store = store(env)?;
        let id = details
            .get("appointmentId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected = details
            .get("expectedVersion")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_default();
        let current = store
            .appointment(id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("约定不存在：{id}"))?;
        if current.version != expected {
            return Err("这条约定刚被改过".to_owned());
        }
        Ok(())
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a crate::root_lifecycle::RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let runtime = host
                .task_runtime()
                .ok_or_else(|| "任务库没开，改不了约定".to_owned())?;
            let appointment = runtime.calendar_update(details)?;
            Ok(ActionOutcome {
                summary: format!(
                    "改好约定「{}」，第 {} 版",
                    appointment.title, appointment.version
                ),
                data: json!({ "appointmentId": appointment.id, "version": appointment.version }),
            })
        })
    }
}
