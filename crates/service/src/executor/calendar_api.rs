//! 日历运行时：计时/复核/排运行/送 Kind 都在这里；executor 不 import calendar 的业务规则之外的东西。

use super::calendar_watch::WatchResult;
use super::*;
use crate::protocol::{
    CalendarActionKind, CalendarActionRequest, CalendarActionResponse, CalendarAppointmentView,
    CalendarReadRequest, CalendarReadResponse, CalendarRunView, CalendarWeekPoint,
};
use calendar::{
    Appointment, AppointmentStatus, Binding, CalendarError, NewRun, Run, RunResult, TriggerKind,
};

const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

impl TaskRuntime {
    pub(crate) fn calendar(&self) -> Result<&calendar::CalendarStore, String> {
        self.calendar
            .as_ref()
            .map(std::convert::AsRef::as_ref)
            .ok_or_else(|| "日历库没开".to_owned())
    }

    pub(crate) fn calendar_read(
        &self,
        request: CalendarReadRequest,
    ) -> Result<CalendarReadResponse, String> {
        let store = self.calendar()?;
        let now = Self::now();
        let all = store.list_appointments().map_err(calendar_error)?;
        let mut appointments = Vec::new();
        for appointment in all {
            if let Some(id) = &request.appointment_id
                && &appointment.id != id
            {
                continue;
            }
            appointments.push(
                self.calendar_view(store, &appointment)
                    .map_err(calendar_error)?,
            );
        }
        let mut week = Vec::new();
        if let Some(week_start) = request.week {
            let week_end = week_start.saturating_add(WEEK_MS);
            for appointment in store.list_appointments().map_err(calendar_error)? {
                if appointment.status != AppointmentStatus::Active {
                    continue;
                }
                let points = appointment
                    .schedule
                    .occurrences_between(week_start, week_end)
                    .map_err(calendar_error)?;
                for at in points {
                    week.push(CalendarWeekPoint {
                        at,
                        appointment_id: appointment.id.clone(),
                        kind: "plan".to_owned(),
                        result: None,
                    });
                }
                for run in store.runs(&appointment.id).map_err(calendar_error)? {
                    if run.nominal_at >= week_start
                        && run.nominal_at <= week_end
                        && run.result != RunResult::Queued
                    {
                        week.push(CalendarWeekPoint {
                            at: run.nominal_at,
                            appointment_id: appointment.id.clone(),
                            kind: "run".to_owned(),
                            result: Some(run.result.as_str().to_owned()),
                        });
                    }
                }
            }
            week.sort_by_key(|point| point.at);
        }
        Ok(CalendarReadResponse {
            generated_at: now,
            appointments,
            week,
        })
    }

    pub(crate) fn calendar_action(
        &self,
        request: CalendarActionRequest,
    ) -> Result<CalendarActionResponse, String> {
        let store = self.calendar()?;
        let appointment = match request.action {
            CalendarActionKind::Pause => store
                .set_status(&request.appointment_id, AppointmentStatus::Paused)
                .map_err(calendar_error)?,
            CalendarActionKind::Resume => store
                .set_status(&request.appointment_id, AppointmentStatus::Active)
                .map_err(calendar_error)?,
            CalendarActionKind::Cancel => store
                .set_status(&request.appointment_id, AppointmentStatus::Cancelled)
                .map_err(calendar_error)?,
            CalendarActionKind::RunNow => {
                let appointment = store
                    .appointment(&request.appointment_id)
                    .map_err(calendar_error)?
                    .ok_or_else(|| format!("约定不存在：{}", request.appointment_id))?;
                let now = Self::now();
                let (run, created) = store
                    .record_run(NewRun {
                        appointment_id: appointment.id.clone(),
                        nominal_at: now,
                        manual: true,
                        trigger: TriggerKind::Manual,
                        reason: String::new(),
                        merged_count: 0,
                        report_id: None,
                    })
                    .map_err(calendar_error)?;
                if created && !self.calendar_effect_pending(run.id) {
                    self.store
                        .enqueue_effect(
                            EffectPayload::AppointmentWake {
                                appointment_id: appointment.id.clone(),
                                run_id: run.id,
                            },
                            now,
                        )
                        .map_err(|error| error.to_string())?;
                    self.wake();
                }
                appointment
            }
        };
        // 状态动作改完 next_trigger 后叫醒泵：可能还睡在旧的远点。
        self.wake();
        self.emit_calendar_changed(&appointment.id);
        Ok(CalendarActionResponse {
            appointment: self
                .calendar_view(store, &appointment)
                .map_err(calendar_error)?,
        })
    }

    /// 启动补：有排着的运行记录、没有效果行的，补一条效果。
    pub(crate) fn calendar_nudge(&self) {
        self.wake();
    }

    /// 测试夹具：URL 盯变化走给定代理（wiremock）。
    pub(crate) fn set_watch_proxy(&self, proxy: String) {
        let env = model::http::HttpEnv {
            http_proxy: Some(proxy),
            https_proxy: None,
            all_proxy: None,
            no_proxy: None,
        };
        if let Ok(mut guard) = self.watch_http.lock() {
            *guard = Some(env);
        }
    }

    pub(crate) fn calendar_recover(&self) {
        let Ok(store) = self.calendar() else {
            return;
        };
        let Ok(appointments) = store.list_appointments() else {
            return;
        };
        for appointment in appointments {
            let Ok(runs) = store.runs(&appointment.id) else {
                continue;
            };
            for run in runs {
                if run.result != RunResult::Queued {
                    continue;
                }
                if matches!(
                    appointment.status,
                    AppointmentStatus::Paused | AppointmentStatus::Cancelled
                ) {
                    let reason = calendar_status_reason(appointment.status);
                    if let Err(error) = store.finish_run(run.id, RunResult::Skipped, reason, None) {
                        eprintln!("暂停/取消后没写运行结果：{error}");
                    }
                    continue;
                }
                if self.calendar_effect_pending(run.id) {
                    continue;
                }
                if let Err(error) = self.store.enqueue_effect(
                    EffectPayload::AppointmentWake {
                        appointment_id: appointment.id.clone(),
                        run_id: run.id,
                    },
                    Self::now(),
                ) {
                    eprintln!("日历唤醒效果没排上：{error}");
                }
            }
        }
    }

    pub(crate) fn calendar_effect_pending(&self, run_id: i64) -> bool {
        self.store
            .effect_pending(EffectKind::AppointmentWake, &run_id.to_string())
            .unwrap_or(false)
    }

    async fn calendar_review_one(
        self: &Arc<Self>,
        store: &calendar::CalendarStore,
        appointment: &Appointment,
        now: i64,
        pause_since: i64,
    ) -> Result<(), String> {
        let next = appointment
            .next_trigger_at
            .ok_or_else(|| "没有下次触发".to_owned())?;
        let mut occurrences = appointment
            .schedule
            .occurrences_between(next, now)
            .map_err(calendar_error)?;
        if occurrences.is_empty() {
            occurrences.push(next);
        }
        let latest = *occurrences.last().unwrap_or(&next);
        let merged = u32::try_from(occurrences.len().saturating_sub(1)).unwrap_or(u32::MAX);

        let bound_task = match &appointment.binding {
            Binding::Task(task_id) => match self.store.task(&TaskId::from(task_id.clone())) {
                Ok(Some(task)) => {
                    let stage = self
                        .store
                        .stage(&task.id)
                        .map_err(|error| error.to_string())?;
                    let graph_cancelled = match &task.parent {
                        Some(parent) => self
                            .store
                            .task(parent)
                            .ok()
                            .flatten()
                            .and_then(|root| root.arrangement)
                            .is_some_and(|arrangement| arrangement.cancelled.is_some()),
                        None => task
                            .arrangement
                            .as_ref()
                            .is_some_and(|arrangement| arrangement.cancelled.is_some()),
                    };
                    Some(calendar::BoundTaskFacts {
                        exists: true,
                        graph_cancelled,
                        closed: stage == Stage::Closed,
                        running: task
                            .latest_attempt()
                            .is_some_and(|attempt| attempt.ended_at.is_none()),
                    })
                }
                _ => Some(calendar::BoundTaskFacts {
                    exists: false,
                    graph_cancelled: false,
                    closed: false,
                    running: false,
                }),
            },
            Binding::Kind => None,
        };
        let kind_busy = if let Some(inner) = self.inner.upgrade() {
            let guard = inner.lock().await;
            guard.as_ref().is_some_and(|host| host.agent().is_running())
        } else {
            false
        };
        let base = calendar::ReviewFacts {
            status: appointment.status,
            bound_task,
            kind_busy,
            global_paused: false,
            watch: None,
        };
        let mut outcome = calendar::review(&base);
        let mut watch_summary = None;
        if matches!(outcome, calendar::Review::Send) && appointment.watch.is_some() {
            match self.watch_check(appointment).await {
                WatchResult::Unchanged => {
                    outcome = calendar::Review::Skip("没变：和上次比内容一样".to_owned());
                }
                WatchResult::Failed(reason) => {
                    outcome = calendar::Review::Fail(format!("没送成：{reason}"));
                }
                WatchResult::Changed { summary } => {
                    watch_summary = Some(summary);
                }
            }
        }

        let skip_reason = match outcome {
            calendar::Review::NoRun => return Ok(()),
            calendar::Review::End => {
                store
                    .set_status(&appointment.id, AppointmentStatus::Ended)
                    .map_err(calendar_error)?;
                self.calendar_advance(store, appointment, latest)?;
                self.emit_calendar_changed(&appointment.id);
                return Ok(());
            }
            calendar::Review::Queue(reason) => Some(reason),
            calendar::Review::Skip(reason) => Some(reason),
            calendar::Review::Fail(reason) => {
                let (run, _) = store
                    .record_run(NewRun {
                        appointment_id: appointment.id.clone(),
                        nominal_at: latest,
                        manual: false,
                        trigger: TriggerKind::Due,
                        reason: reason.clone(),
                        merged_count: merged,
                        report_id: None,
                    })
                    .map_err(calendar_error)?;
                store
                    .finish_run(run.id, RunResult::Failed, &reason, None)
                    .map_err(calendar_error)?;
                self.calendar_advance(store, appointment, latest)?;
                self.emit_calendar_changed(&appointment.id);
                return Ok(());
            }
            calendar::Review::Send => None,
        };

        let merged_reason = if merged > 0 {
            let since = pause_since;
            if since > 0 && next >= since {
                calendar::merged_reason_global_pause(merged)
            } else {
                calendar::merged_reason_missed(merged)
            }
        } else {
            String::new()
        };
        let (run, created) = store
            .record_run(NewRun {
                appointment_id: appointment.id.clone(),
                nominal_at: latest,
                manual: false,
                trigger: TriggerKind::Due,
                reason: watch_summary.unwrap_or(merged_reason),
                merged_count: merged,
                report_id: None,
            })
            .map_err(calendar_error)?;
        match skip_reason {
            Some(reason) => {
                store
                    .finish_run(run.id, RunResult::Skipped, &reason, None)
                    .map_err(calendar_error)?;
            }
            None => {
                if (created || run.result == RunResult::Failed)
                    && !self.calendar_effect_pending(run.id)
                {
                    self.store
                        .enqueue_effect(
                            EffectPayload::AppointmentWake {
                                appointment_id: appointment.id.clone(),
                                run_id: run.id,
                            },
                            now,
                        )
                        .map_err(|error| error.to_string())?;
                    self.wake();
                }
            }
        }
        self.calendar_advance(store, appointment, latest)?;
        self.emit_calendar_changed(&appointment.id);
        Ok(())
    }

    fn calendar_advance(
        &self,
        store: &calendar::CalendarStore,
        appointment: &Appointment,
        latest: i64,
    ) -> Result<(), String> {
        let next = appointment
            .schedule
            .next_after(latest)
            .map_err(calendar_error)?;
        match next {
            Some(next) => {
                store
                    .set_next_trigger(&appointment.id, Some(next))
                    .map_err(calendar_error)?;
            }
            None => {
                store
                    .set_status(&appointment.id, AppointmentStatus::Ended)
                    .map_err(calendar_error)?;
            }
        }
        Ok(())
    }

    /// 计时器到点：一条循环只处理到期约定；全局暂停时不动，开闸后合并。
    pub(crate) async fn calendar_tick(self: &Arc<Self>, now: i64) {
        let Ok(store) = self.calendar() else {
            return;
        };
        if self.exec_paused.load(Ordering::SeqCst) {
            return;
        }
        let pause_since = self.global_pause_since.load(Ordering::SeqCst);
        let appointments = store.due_appointments(now).unwrap_or_default();
        for appointment in appointments {
            if self.shutting_down.load(Ordering::SeqCst) {
                return;
            }
            if let Err(error) = self
                .calendar_review_one(store, &appointment, now, pause_since)
                .await
            {
                eprintln!("日历复核失败：{error}");
            }
        }
        if pause_since > 0 {
            self.global_pause_since.store(0, Ordering::SeqCst);
        }
    }

    /// 下一次醒来的等待毫秒：只等最近一条约定；没有就 100ms 轻轮询。
    pub(crate) fn calendar_sleep_ms(&self, now: i64) -> u64 {
        let Some(store) = &self.calendar else {
            return 100;
        };
        if self.exec_paused.load(Ordering::SeqCst) {
            return 500;
        }
        match store.next_wake_at() {
            Ok(Some(at)) => u64::try_from((at - now).clamp(20, 60_000)).unwrap_or(100),
            _ => 10_000,
        }
    }

    pub(crate) async fn deliver_appointment_wake(self: &Arc<Self>, claim: EffectClaim) {
        let EffectPayload::AppointmentWake {
            appointment_id,
            run_id,
        } = &claim.payload
        else {
            self.finish_or_fail(&claim, Err("appointment wake 载荷不对".to_owned()));
            return;
        };
        let Ok(store) = self.calendar() else {
            self.finish_or_fail(&claim, Err("日历库没开".to_owned()));
            return;
        };
        let Ok(Some(appointment)) = store.appointment(appointment_id) else {
            self.finish_or_fail(&claim, Ok(()));
            return;
        };
        let Ok(runs) = store.runs(appointment_id) else {
            self.finish_or_fail(&claim, Err("读运行记录失败".to_owned()));
            return;
        };
        let Some(run) = runs.into_iter().find(|run| run.id == *run_id) else {
            self.finish_or_fail(&claim, Ok(()));
            return;
        };
        if run.result != RunResult::Queued && run.result != RunResult::Failed {
            self.finish_or_fail(&claim, Ok(()));
            return;
        }
        if matches!(
            appointment.status,
            AppointmentStatus::Paused | AppointmentStatus::Cancelled
        ) {
            let reason = calendar_status_reason(appointment.status);
            if let Err(error) = store.finish_run(run.id, RunResult::Skipped, reason, None) {
                eprintln!("暂停/取消后没写运行结果：{error}");
            }
            self.finish_or_fail(&claim, Ok(()));
            self.emit_calendar_changed(&appointment.id);
            return;
        }
        if self.crash_before_wake.swap(false, Ordering::SeqCst) {
            self.finish_or_fail(&claim, Err("模拟日历唤醒送达前崩掉".to_owned()));
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            self.later(&claim);
            return;
        };
        let guard = inner.lock().await;
        let Some(host) = guard.as_ref() else {
            drop(guard);
            self.later(&claim);
            return;
        };
        let conversation = host.agent().id().as_str().to_owned();
        let trigger_task = match self.trigger_task_for_run(&appointment, &run, &conversation) {
            Ok(task) => task,
            Err(error) => {
                drop(guard);
                self.finish_or_fail(&claim, Err(error));
                return;
            }
        };
        let mut body = json!({
            "task_id": trigger_task.id.as_str(),
            "kind": "appointment_wake",
            "appointment": {
                "id": appointment.id,
                "title": appointment.title,
                "condition": appointment.condition,
                "version": appointment.version,
                "binding": match &appointment.binding { Binding::Kind => "kind", Binding::Task(_) => "task" },
                "run_id": run.id,
                "trigger": run.trigger.as_str(),
                "nominal_at": run.nominal_at,
                "merged_count": run.merged_count,
                "manual": run.manual,
                "reason": run.reason,
            }
        });
        if appointment.watch.is_some()
            && let Ok(Some(state)) = store.watch_state(&appointment.id)
        {
            body["change"] = json!({
                "summary": run.reason,
                "attachment_id": state.attachment_id,
            });
        }
        let request_id = format!("appointment-wake:{}:{}", appointment.id, run.id);
        let message = UserMessage::text(body.to_string());
        let sent = host
            .send_message(&request_id, message, DeliveryMode::Steer)
            .await;
        let mut sent_turn = None;
        if sent.is_ok()
            && let Ok(session_id) = session::SessionId::parse(host.agent().id().as_str())
        {
            // 送达返回时事件可能还没落盘：短轮询会话日志到那条 user/message 出现。
            for _ in 0..20 {
                if let Ok(store) =
                    session::SessionStore::open_read_only(&self.data_dir, &session_id)
                {
                    sent_turn = store.events().iter().rev().find_map(|event| {
                        let session::EventKind::UserMessage { turn, message, .. } = &event.kind
                        else {
                            return None;
                        };
                        let model::MessageSource::User { rpc_id } = &message.source else {
                            return None;
                        };
                        rpc_id
                            .as_deref()
                            .filter(|rpc| *rpc == request_id)
                            .map(|_| i64::try_from(*turn).unwrap_or(i64::MAX))
                    });
                }
                if sent_turn.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        if let Some(turn) = sent_turn.and_then(|turn| u64::try_from(turn).ok()) {
            self.remember_trigger_turn(&conversation, turn, &appointment.id, run.id);
        }
        drop(guard);
        match sent {
            Ok(_) => {
                if let Err(error) =
                    store.finish_run(run.id, RunResult::Sent, &run.reason, sent_turn)
                {
                    eprintln!("日历运行记录没写成功：{error}");
                }
                self.finish_or_fail(&claim, Ok(()));
                self.emit_calendar_changed(&appointment.id);
            }
            Err(error) => {
                let reason = format!("没送成：{error}");
                if let Err(error) = store.finish_run(run.id, RunResult::Failed, &reason, None) {
                    eprintln!("日历失败记录没写成功：{error}");
                }
                self.finish_or_fail(&claim, Err(error.to_string()));
                self.emit_calendar_changed(&appointment.id);
            }
        }
    }

    fn calendar_view(
        &self,
        store: &calendar::CalendarStore,
        appointment: &Appointment,
    ) -> Result<CalendarAppointmentView, CalendarError> {
        let runs = store
            .runs(&appointment.id)?
            .into_iter()
            .map(calendar_run_view)
            .collect();
        Ok(CalendarAppointmentView {
            id: appointment.id.clone(),
            title: appointment.title.clone(),
            condition: appointment.condition.clone(),
            rule: appointment.schedule.rule.clone(),
            trigger: crate::calendar_trigger::describe(
                &appointment.schedule.rule,
                &appointment.schedule.timezone,
                appointment.schedule.start_at,
            ),
            timezone: appointment.schedule.timezone.clone(),
            status: appointment.status.as_str().to_owned(),
            version: appointment.version,
            binding: match &appointment.binding {
                Binding::Kind => "kind".to_owned(),
                Binding::Task(_) => "task".to_owned(),
            },
            bound_task_id: match &appointment.binding {
                Binding::Kind => None,
                Binding::Task(task) => Some(task.clone()),
            },
            next_trigger_at: appointment.next_trigger_at,
            pause_reason: if appointment.status == AppointmentStatus::Paused {
                store
                    .latest_version_reason(&appointment.id)
                    .ok()
                    .flatten()
                    .and_then(|reason| reason.strip_prefix("循环约定暂停：").map(str::to_owned))
            } else {
                None
            },
            watch_kind: appointment.watch.as_ref().map(|watch| {
                match watch {
                    calendar::WatchSource::Url(_) => "url",
                    calendar::WatchSource::File(_) => "file",
                    calendar::WatchSource::TaskFacts(_) => "taskFacts",
                }
                .to_owned()
            }),
            watch_target: appointment.watch.as_ref().map(|watch| match watch {
                calendar::WatchSource::Url(url) => url.clone(),
                calendar::WatchSource::File(file) => file.clone(),
                calendar::WatchSource::TaskFacts(task) => task.clone(),
            }),
            cycle_task_id: appointment
                .cycle
                .as_ref()
                .map(|cycle| cycle.task_id.clone()),
            cycle_max_rounds: appointment
                .cycle
                .as_ref()
                .and_then(|cycle| cycle.max_rounds),
            source_turn: appointment.source_turn,
            created_at: appointment.created_at,
            updated_at: appointment.updated_at,
            runs,
        })
    }

    pub(crate) fn emit_calendar_changed(&self, appointment_id: &str) {
        let event = ServiceEvent {
            seq: 0,
            time: 0,
            event_type: "calendar/changed".to_owned(),
            data: json!({ "appointmentId": appointment_id }),
            cause: None,
        };
        if let Some(events) = &self.calendar_events
            && let Err(error) = events.send(event)
        {
            eprintln!("calendar/changed 推送失败：{error}");
        }
    }
}

fn calendar_run_view(run: Run) -> CalendarRunView {
    CalendarRunView {
        id: run.id,
        nominal_at: run.nominal_at,
        manual: run.manual,
        trigger: run.trigger.as_str().to_owned(),
        result: run.result.as_str().to_owned(),
        reason: run.reason,
        merged_count: run.merged_count,
        created_at: run.created_at,
        updated_at: run.updated_at,
        sent_turn: run.sent_turn,
    }
}

fn calendar_status_reason(status: AppointmentStatus) -> &'static str {
    match status {
        AppointmentStatus::Active => "",
        AppointmentStatus::Paused => "跳过：约定已暂停",
        AppointmentStatus::Cancelled => "跳过：约定已取消",
        AppointmentStatus::Ended => "跳过：约定已结束",
    }
}

fn calendar_error(error: CalendarError) -> String {
    error.to_string()
}
