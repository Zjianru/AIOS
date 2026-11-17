//! T064.2：日历运行时——timer 最近一条、到点送达、启动补、全局暂停合并、协议读/动作。

use std::time::{SystemTime, UNIX_EPOCH};

use calendar::{Binding, NewAppointment, Schedule};
use service::{CalendarActionKind, CalendarActionRequest, CalendarReadRequest, Call, Response};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

async fn wait_until(label: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..240 {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("等不到：{label}");
}

fn appointment(
    store: &calendar::CalendarStore,
    title: &str,
    start_at: i64,
    rule: &str,
) -> calendar::Appointment {
    store
        .create_appointment(NewAppointment {
            title: title.to_owned(),
            condition: "到点提醒我".to_owned(),
            schedule: Schedule {
                rule: rule.to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at,
            },
            binding: Binding::Kind,
            watch: None,
            cycle: None,
        })
        .unwrap()
}

fn run_result(store: &calendar::CalendarStore, appointment_id: &str) -> Option<String> {
    store
        .runs(appointment_id)
        .ok()
        .and_then(|runs| runs.first().map(|run| run.result.as_str().to_owned()))
}

#[tokio::test]
async fn nearest_appointment_wakes_and_far_one_waits() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let near = appointment(&store, "近的提醒", now, "FREQ=DAILY");
    store.set_next_trigger(&near.id, Some(now + 300)).unwrap();
    let far = appointment(&store, "远的提醒", now, "FREQ=DAILY");
    store.set_next_trigger(&far.id, Some(now + 5_000)).unwrap();

    let host = crate::common::open(crate::common::options(data)).await;
    wait_until("近的约定送到", || {
        run_result(&store, &near.id).as_deref() == Some("sent")
    })
    .await;
    assert!(
        store.runs(&far.id).unwrap().is_empty(),
        "远的没到点，不该提前 wake"
    );
    let session_id = host.info().session_id.expect("根会话");
    let snapshot = session::SessionStore::open_read_only(
        data,
        &session::SessionId::parse(&session_id).expect("会话 id"),
    )
    .expect("会话日志");
    let prefix = format!("appointment-wake:{}:", near.id);
    assert!(
        snapshot.events().iter().any(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return false;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return false;
            };
            rpc_id
                .as_deref()
                .is_some_and(|rpc| rpc.starts_with(&prefix))
        }),
        "根会话日志里要有 appointment-wake 那条用户消息"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn failed_wake_retries_with_backoff_then_sends() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = appointment(&store, "退避重试", now, "FREQ=DAILY");
    store.set_next_trigger(&due.id, Some(now + 300)).unwrap();

    let host = crate::common::open(crate::common::options(data)).await;
    host.crash_before_next_wake();
    wait_until("退避后送到", || {
        run_result(&store, &due.id).as_deref() == Some("sent")
    })
    .await;
    let runs = store.runs(&due.id).unwrap();
    assert_eq!(runs.len(), 1, "同一唤醒身份只有一条运行");
    assert_eq!(runs[0].result, calendar::RunResult::Sent);
    host.shutdown().await;
}

#[tokio::test]
async fn startup_recovery_sends_a_queued_run_without_effect() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = appointment(&store, "启动补", now, "FREQ=DAILY");
    store.set_next_trigger(&due.id, Some(now + 60_000)).unwrap();
    let (run, _) = store
        .record_run(calendar::NewRun {
            appointment_id: due.id.clone(),
            nominal_at: now - 1_000,
            manual: false,
            trigger: calendar::TriggerKind::Due,
            reason: String::new(),
            merged_count: 0,
        })
        .unwrap();

    let host = crate::common::open(crate::common::options(data)).await;
    wait_until("补送完成", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|stored| stored.id == run.id && stored.result == calendar::RunResult::Sent)
    })
    .await;
    host.shutdown().await;
}

#[tokio::test]
async fn global_pause_merges_missed_occurrences_and_sends_once() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let mut settings = service::settings::Settings {
        last_selection: None,
        currency: "usd".to_owned(),
        disabled_models: Vec::new(),
        exec: service::settings::ExecSettings {
            paused: true,
            seats: 3,
            bash: false,
        },
    };
    service::settings::save(data, &settings).unwrap();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = appointment(&store, "暂停合并", now - 5_000, "FREQ=SECONDLY");
    store.set_next_trigger(&due.id, Some(now - 5_000)).unwrap();

    let host = crate::common::open(crate::common::options(data)).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(store.runs(&due.id).unwrap().is_empty(), "暂停期间不排运行");

    settings.exec.paused = false;
    service::settings::save(data, &settings).unwrap();
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        currency: None,
        paused: Some(false),
        seats: None,
        bash: None,
    }))
    .await
    .expect("开闸");
    wait_until("合并后送一次", || {
        run_result(&store, &due.id).as_deref() == Some("sent")
    })
    .await;
    let runs = store.runs(&due.id).unwrap();
    assert_eq!(runs.len(), 1, "排着的合并成一次");
    assert!(
        runs[0].merged_count >= 4,
        "合并次数：{}",
        runs[0].merged_count
    );
    assert!(runs[0].reason.contains("合并了"), "{}", runs[0].reason);
    host.shutdown().await;
}

#[tokio::test]
async fn paused_appointment_does_not_deliver_a_queued_wake() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = appointment(&store, "暂停不发", now - 1_000, "FREQ=SECONDLY");
    store.set_next_trigger(&due.id, Some(now - 1)).unwrap();
    let (run, _) = store
        .record_run(calendar::NewRun {
            appointment_id: due.id.clone(),
            nominal_at: now - 1,
            manual: false,
            trigger: calendar::TriggerKind::Due,
            reason: String::new(),
            merged_count: 0,
        })
        .unwrap();
    {
        let tasks = tasks::Store::open(data.join("tasks.sqlite")).unwrap();
        tasks
            .enqueue_effect(
                tasks::EffectPayload::AppointmentWake {
                    appointment_id: due.id.clone(),
                    run_id: run.id,
                },
                now,
            )
            .unwrap();
    }
    store
        .set_status(&due.id, calendar::AppointmentStatus::Paused)
        .unwrap();

    let host = crate::common::open(crate::common::options(data)).await;
    wait_until("暂停的排着收口", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|stored| stored.id == run.id && stored.result == calendar::RunResult::Skipped)
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let session_id = host.info().session_id.expect("根会话");
    let snapshot = session::SessionStore::open_read_only(
        data,
        &session::SessionId::parse(&session_id).expect("会话 id"),
    )
    .expect("会话日志");
    assert!(
        !snapshot.events().iter().any(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return false;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return false;
            };
            rpc_id
                .as_deref()
                .is_some_and(|rpc| rpc.starts_with("appointment-wake:"))
        }),
        "暂停后不能再送已排的 wake"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn resume_wakes_the_pump_and_sends_the_due_run() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = appointment(&store, "恢复就叫醒", now - 5_000, "FREQ=SECONDLY");
    store.set_next_trigger(&due.id, Some(now - 5_000)).unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    host.dispatch(Call::CalendarAction(CalendarActionRequest {
        appointment_id: due.id.clone(),
        action: CalendarActionKind::Pause,
    }))
    .await
    .expect("暂停");
    host.dispatch(Call::CalendarAction(CalendarActionRequest {
        appointment_id: due.id.clone(),
        action: CalendarActionKind::Resume,
    }))
    .await
    .expect("恢复");
    wait_until("恢复后送到", || {
        run_result(&store, &due.id).is_some_and(|result| result == "sent" || result == "skipped")
    })
    .await;
    assert_eq!(
        store.appointment(&due.id).unwrap().unwrap().status,
        calendar::AppointmentStatus::Active
    );
    host.shutdown().await;
}

#[tokio::test]
async fn calendar_read_and_actions_move_status() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let appointment = appointment(&store, "读日历", now_ms(), "FREQ=DAILY");
    let host = crate::common::open(crate::common::options(data)).await;

    let response = host
        .dispatch(Call::CalendarRead(CalendarReadRequest::default()))
        .await
        .expect("日历读");
    let Response::CalendarRead(read) = response else {
        panic!("日历读响应");
    };
    assert_eq!(read.appointments.len(), 1);
    assert_eq!(read.appointments[0].id, appointment.id);
    assert_eq!(read.appointments[0].status, "active");

    let week = host
        .dispatch(Call::CalendarRead(CalendarReadRequest {
            appointment_id: None,
            week: Some(now_ms() - 2 * 24 * 60 * 60 * 1000),
        }))
        .await
        .expect("日历周计划");
    let Response::CalendarRead(week) = week else {
        panic!("日历周读响应");
    };
    assert!(
        week.week.iter().any(|point| point.kind == "plan"),
        "周视图要有计划点：{:?}",
        week.week
    );

    let mut events = host.subscribe_events();
    let response = host
        .dispatch(Call::CalendarAction(CalendarActionRequest {
            appointment_id: appointment.id.clone(),
            action: CalendarActionKind::Pause,
        }))
        .await
        .expect("暂停");
    let Response::CalendarAction(action) = response else {
        panic!("日历动作响应");
    };
    assert_eq!(action.appointment.status, "paused");
    assert_eq!(
        store.appointment(&appointment.id).unwrap().unwrap().status,
        calendar::AppointmentStatus::Paused
    );
    let pushed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("事件流");
            if event.event_type == "calendar/changed" {
                return event;
            }
        }
    })
    .await
    .expect("推 calendar/changed");
    assert_eq!(
        pushed.data.get("appointmentId").and_then(|id| id.as_str()),
        Some(appointment.id.as_str())
    );

    let response = host
        .dispatch(Call::CalendarAction(CalendarActionRequest {
            appointment_id: appointment.id.clone(),
            action: CalendarActionKind::Resume,
        }))
        .await
        .expect("恢复");
    let Response::CalendarAction(resumed) = response else {
        panic!("恢复响应");
    };
    assert_eq!(resumed.appointment.status, "active");
    let before = store.appointment(&appointment.id).unwrap().unwrap();

    let response = host
        .dispatch(Call::CalendarAction(CalendarActionRequest {
            appointment_id: appointment.id.clone(),
            action: CalendarActionKind::RunNow,
        }))
        .await
        .expect("立即跑一次");
    let Response::CalendarAction(_) = response else {
        panic!("立即跑响应");
    };
    wait_until("手动运行送到", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.manual && run.result == calendar::RunResult::Sent)
    })
    .await;
    let after = store.appointment(&appointment.id).unwrap().unwrap();
    assert_eq!(
        after.next_trigger_at, before.next_trigger_at,
        "立即跑不改下次触发"
    );

    let response = host
        .dispatch(Call::CalendarAction(CalendarActionRequest {
            appointment_id: appointment.id.clone(),
            action: CalendarActionKind::Cancel,
        }))
        .await
        .expect("取消");
    let Response::CalendarAction(cancelled) = response else {
        panic!("取消响应");
    };
    assert_eq!(cancelled.appointment.status, "cancelled");
    assert!(
        store
            .appointment(&appointment.id)
            .unwrap()
            .unwrap()
            .next_trigger_at
            .is_none()
    );
    host.shutdown().await;
}
