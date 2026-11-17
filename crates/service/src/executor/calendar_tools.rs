//! T064.5：Kind 的日历工具——appointment_cancel（独立工具），以及卡片动作背后的建/改内部入口。

use super::*;
use calendar::{AppointmentStatus, Binding, CycleSpec, NewAppointment, Schedule, WatchSource};

impl TaskRuntime {
    /// 确认卡 create_appointment 的正式入口：建一条约定并推 calendar/changed。
    pub(crate) fn calendar_create(&self, details: &Value) -> Result<calendar::Appointment, String> {
        let draft = appointment_from_details(details)?;
        let store = self.calendar()?;
        let appointment = store
            .create_appointment(draft)
            .map_err(|error| error.to_string())?;
        self.wake();
        self.emit_calendar_changed(&appointment.id);
        Ok(appointment)
    }

    /// 确认卡 update_appointment 的正式入口：带 expectedVersion 的改。
    pub(crate) fn calendar_update(&self, details: &Value) -> Result<calendar::Appointment, String> {
        let id = details
            .get("appointmentId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "details.appointmentId 必须是非空字符串".to_owned())?;
        let expected = details
            .get("expectedVersion")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| "details.expectedVersion 必须是正整数".to_owned())?;
        let store = self.calendar()?;
        let mut patch = calendar::AppointmentPatch::default();
        if let Some(title) = details.get("title").and_then(Value::as_str) {
            patch.title = Some(title.to_owned());
        }
        if let Some(condition) = details.get("condition").and_then(Value::as_str) {
            patch.condition = Some(condition.to_owned());
        }
        if details.get("rule").is_some()
            || details.get("timezone").is_some()
            || details.get("startAt").is_some()
        {
            let current = store
                .appointment(id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("约定不存在：{id}"))?;
            patch.schedule = Some(Schedule {
                rule: details
                    .get("rule")
                    .and_then(Value::as_str)
                    .unwrap_or(&current.schedule.rule)
                    .to_owned(),
                timezone: details
                    .get("timezone")
                    .and_then(Value::as_str)
                    .unwrap_or(&current.schedule.timezone)
                    .to_owned(),
                start_at: details
                    .get("startAt")
                    .and_then(Value::as_i64)
                    .unwrap_or(current.schedule.start_at),
            });
        }
        let appointment = store
            .update_appointment(id, expected, patch, "Kind 确认卡")
            .map_err(|error| error.to_string())?;
        self.wake();
        self.emit_calendar_changed(&appointment.id);
        Ok(appointment)
    }

    /// 直接取消：用户叫停记已取消，程序按停止条件停记已结束。
    pub(crate) fn calendar_cancel(
        &self,
        id: &str,
        stopped: bool,
    ) -> Result<calendar::Appointment, String> {
        let store = self.calendar()?;
        let status = if stopped {
            AppointmentStatus::Ended
        } else {
            AppointmentStatus::Cancelled
        };
        let appointment = store
            .set_status(id, status)
            .map_err(|error| error.to_string())?;
        self.wake();
        self.emit_calendar_changed(&appointment.id);
        Ok(appointment)
    }
}

fn appointment_from_details(details: &Value) -> Result<NewAppointment, String> {
    let title = details
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| "details.title 必须是非空字符串".to_owned())?;
    let condition = details
        .get("condition")
        .and_then(Value::as_str)
        .filter(|condition| !condition.trim().is_empty())
        .ok_or_else(|| "details.condition 必须是非空字符串".to_owned())?;
    let rule = details
        .get("rule")
        .and_then(Value::as_str)
        .filter(|rule| !rule.trim().is_empty())
        .ok_or_else(|| "details.rule 必须是非空字符串".to_owned())?;
    let timezone = details
        .get("timezone")
        .and_then(Value::as_str)
        .unwrap_or("Asia/Shanghai");
    let start_at = details
        .get("startAt")
        .and_then(Value::as_i64)
        .ok_or_else(|| "details.startAt 必须是毫秒时间戳".to_owned())?;
    let binding = match details.get("binding").and_then(Value::as_str) {
        Some("task") => Binding::Task(
            details
                .get("taskId")
                .and_then(Value::as_str)
                .ok_or_else(|| "绑定任务时 details.taskId 必须给".to_owned())?
                .to_owned(),
        ),
        _ => Binding::Kind,
    };
    let watch = details.get("watch").and_then(|watch| {
        let kind = watch.get("kind").and_then(Value::as_str)?;
        let target = watch.get("target").and_then(Value::as_str)?;
        match kind {
            "url" => Some(WatchSource::Url(target.to_owned())),
            "file" => Some(WatchSource::File(target.to_owned())),
            "taskFacts" => Some(WatchSource::TaskFacts(target.to_owned())),
            _ => None,
        }
    });
    let cycle = details
        .get("cycleTaskId")
        .and_then(Value::as_str)
        .map(|task_id| CycleSpec {
            task_id: task_id.to_owned(),
            max_rounds: details
                .get("maxRounds")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok()),
        });
    Ok(NewAppointment {
        title: title.to_owned(),
        condition: condition.to_owned(),
        schedule: Schedule {
            rule: rule.to_owned(),
            timezone: timezone.to_owned(),
            start_at,
        },
        binding,
        watch,
        cycle,
    })
}

/// appointment_cancel：由 aios-calendar 开出的独立取消工具。
pub struct AppointmentCancelTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl AppointmentCancelTool {
    pub fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "appointmentId": { "type": "string", "description": "Appointment id to cancel." },
                    "reason": { "type": "string", "enum": ["user", "stopped"], "description": "user: the user asked to cancel. stopped: the program stopped it by its own condition." }
                },
                "required": ["appointmentId", "reason"]
            }),
        }
    }
}

impl Tool for AppointmentCancelTool {
    fn name(&self) -> &str {
        "appointment_cancel"
    }

    fn description(&self) -> &str {
        "Cancel an appointment immediately without a card. reason=user when the user asked to cancel; reason=stopped when the program stops it by its own condition (the appointment is marked ended). Load skill aios-calendar first."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkill(vec!["aios-calendar".to_owned()])
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let Ok(args) = serde_json::from_str::<Value>(&call.arguments) else {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain("arguments must be a JSON object"),
                );
            };
            let Some(id) = args.get("appointmentId").and_then(Value::as_str) else {
                return ToolResult::error(call.id, ToolFailure::plain("appointmentId is required"));
            };
            let reason = args.get("reason").and_then(Value::as_str).unwrap_or("user");
            if !matches!(reason, "user" | "stopped") {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain("reason must be user or stopped"),
                );
            }
            match self.runtime.calendar_cancel(id, reason == "stopped") {
                Ok(appointment) => ToolResult::success(
                    call.id,
                    format!(
                        "已{}约定「{}」（{}）。",
                        if reason == "stopped" {
                            "结束"
                        } else {
                            "取消"
                        },
                        appointment.title,
                        appointment.id
                    ),
                ),
                Err(error) => ToolResult::error(call.id, ToolFailure::plain(error)),
            }
        })
    }
}
