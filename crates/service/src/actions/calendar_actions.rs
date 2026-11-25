//! 日历约定暂停/恢复/立即跑一次（属于 aios-calendar）；执行走页面同一个 CalendarAction。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::protocol::{CalendarActionKind, CalendarActionRequest};
use crate::root_lifecycle::RootHost;

pub(crate) struct AppointmentAction {
    data_dir: PathBuf,
    kind: CalendarActionKind,
}

impl AppointmentAction {
    pub(crate) fn pause(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: CalendarActionKind::Pause,
        }
    }

    pub(crate) fn resume(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: CalendarActionKind::Resume,
        }
    }

    pub(crate) fn run_now(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: CalendarActionKind::RunNow,
        }
    }

    fn id(raw: &Value) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get("appointment_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.appointment_id 必须是非空的约定 id".to_owned())
    }

    fn store(&self) -> Result<calendar::CalendarStore, String> {
        calendar::CalendarStore::open(self.data_dir.join("calendar.sqlite"))
            .map_err(|error| format!("日历库读不出来：{error}"))
    }

    fn appointment(&self, id: &str) -> Result<calendar::Appointment, String> {
        if id == "builtin:dream" {
            return Err("做梦是内置行，请用 memory_dream_settings".to_owned());
        }
        self.store()?
            .appointment(id)
            .map_err(|error| format!("约定读不出来：{error}"))?
            .ok_or_else(|| format!("找不到这个约定：{id}"))
    }

    fn status_zh(status: calendar::AppointmentStatus) -> &'static str {
        match status {
            calendar::AppointmentStatus::Active => "进行中",
            calendar::AppointmentStatus::Paused => "已暂停",
            calendar::AppointmentStatus::Ended => "已结束",
            calendar::AppointmentStatus::Cancelled => "已取消",
        }
    }
}

impl ActionKind for AppointmentAction {
    fn name(&self) -> &str {
        match self.kind {
            CalendarActionKind::Pause => "appointment_pause",
            CalendarActionKind::Resume => "appointment_resume",
            CalendarActionKind::RunNow => "appointment_run_now",
            CalendarActionKind::Cancel => "appointment_cancel",
        }
    }

    fn skill(&self) -> &str {
        "aios-calendar"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({ "appointment_id": Self::id(raw)? }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        let id = Self::id(details)?;
        if id == "builtin:dream" {
            let settings = memory::MemoryStore::open(self.data_dir.join("memory.sqlite"))
                .map_err(|error| format!("记忆库读不出来：{error}"))?
                .dream_settings()
                .map_err(|error| format!("做梦设置读不出来：{error}"))?;
            let (fact, action) = match self.kind {
                CalendarActionKind::Pause => (
                    format!(
                        "做梦{} → 做梦关",
                        if settings.enabled { "开" } else { "关" }
                    ),
                    "关闭做梦",
                ),
                CalendarActionKind::Resume => (
                    format!(
                        "做梦{} → 做梦开",
                        if settings.enabled { "开" } else { "关" }
                    ),
                    "打开做梦",
                ),
                CalendarActionKind::RunNow => ("立即整理一次".to_owned(), "立刻整理"),
                CalendarActionKind::Cancel => return Err("做梦是内置行，不能取消".to_owned()),
            };
            return Ok(vec![
                CardFact {
                    name: "约定".to_owned(),
                    value: "做梦（内置）".to_owned(),
                },
                CardFact {
                    name: "状态".to_owned(),
                    value: fact,
                },
                CardFact {
                    name: "动作".to_owned(),
                    value: action.to_owned(),
                },
            ]);
        }
        let appointment = self.appointment(&id)?;
        let (fact, action) = match self.kind {
            CalendarActionKind::Pause => (
                format!("{} → {}", Self::status_zh(appointment.status), "已暂停"),
                "暂停这条约定",
            ),
            CalendarActionKind::Resume => (
                format!("{} → {}", Self::status_zh(appointment.status), "进行中"),
                "恢复这条约定",
            ),
            CalendarActionKind::RunNow => (
                format!("{}；立即跑一次", Self::status_zh(appointment.status)),
                "立刻触发一次",
            ),
            CalendarActionKind::Cancel => return Err("取消不走这张卡".to_owned()),
        };
        Ok(vec![
            CardFact {
                name: "约定".to_owned(),
                value: appointment.title,
            },
            CardFact {
                name: "状态".to_owned(),
                value: fact,
            },
            CardFact {
                name: "动作".to_owned(),
                value: action.to_owned(),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let id = Self::id(details)?;
            let response = if id == "builtin:dream" {
                host.calendar_builtin_action(&self.kind)
            } else {
                host.task_runtime()
                    .ok_or_else(|| "日历库没开，改不了约定".to_owned())?
                    .calendar_action(CalendarActionRequest {
                        appointment_id: id,
                        action: self.kind.clone(),
                    })
                    .map_err(|error| error.to_string())
            }?;
            Ok(ActionOutcome {
                summary: format!("约定「{}」已处理", response.appointment.title),
                data: json!({ "appointment": response.appointment }),
            })
        })
    }
}
