//! T064.5：Kind 的日历面——一个查询、建/改确认卡事实与旧版本拒绝、直接取消的原因区分。

use std::path::Path;
use std::sync::Arc;

use calendar::{Binding, NewAppointment, Schedule};
use serde_json::{Value, json};
use service::{
    ActionEnv, ActionRegistry, CalendarActionKind, CalendarActionRequest, Call, Response,
};
use tools::{Tool, ToolCall, ToolContext, ToolSessionState};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn create_appointment(
    data: &Path,
    title: &str,
) -> (calendar::CalendarStore, calendar::Appointment) {
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let appointment = store
        .create_appointment(NewAppointment {
            title: title.to_owned(),
            condition: "每天九点提醒我".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now,
            },
            binding: Binding::Kind,
            watch: None,
            cycle: None,
        })
        .unwrap();
    (store, appointment)
}

fn context(data: &Path) -> ToolContext {
    ToolContext::with_session(data, Arc::new(ToolSessionState::new(data, "kind-calendar")))
}

#[tokio::test]
async fn aios_query_calendar_returns_appointments_and_runs() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let (_store, appointment) = create_appointment(data, "读日历");
    let tool = service::aios_query::QueryTool::new(data.to_path_buf(), None);
    let result = tool
        .execute(
            ToolCall {
                id: "call_query".to_owned(),
                name: "aios_query".to_owned(),
                arguments: json!({ "topic": "calendar" }).to_string(),
            },
            &context(data),
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
    let value: Value = serde_json::from_str(&result.content).expect("query json");
    assert_eq!(value["appointments"][0]["id"], appointment.id);
    assert_eq!(value["appointments"][0]["title"], "读日历");
}

#[tokio::test]
async fn propose_create_and_update_facts_and_stale_version_refusal() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let mut options = crate::common::options(data);
    options.actions = Arc::new(ActionRegistry::settings(data.to_path_buf(), None));
    let host = crate::common::open(options).await;
    let registry = ActionRegistry::settings(data.to_path_buf(), None);
    let env = ActionEnv {
        data_dir: data,
        identity: None,
        events: &[],
    };
    let create = registry.get("create_appointment").expect("create 动作");
    assert_eq!(create.skill(), "aios-calendar");
    let details = json!({
        "title": "提醒喝水",
        "condition": "每天九点提醒我喝水",
        "rule": "FREQ=DAILY",
        "timezone": "Asia/Shanghai",
        "startAt": now_ms(),
        "binding": "kind"
    });
    let normalized = create.details(&details).unwrap();
    let facts = create.describe(&normalized, &env).unwrap();
    assert!(
        facts
            .iter()
            .any(|fact| fact.name == "触发" && fact.value.contains("FREQ=DAILY"))
    );
    assert!(
        facts
            .iter()
            .any(|fact| fact.name == "原话" && fact.value.contains("喝水"))
    );
    let outcome = create.act(&normalized, &host).await.unwrap();
    let appointment_id = outcome.data["appointmentId"].as_str().unwrap().to_owned();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let created = store.appointment(&appointment_id).unwrap().unwrap();
    assert_eq!(created.title, "提醒喝水");

    let update = registry.get("update_appointment").expect("update 动作");
    let change = json!({
        "appointmentId": appointment_id,
        "expectedVersion": created.version,
        "rule": "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
        "startAt": created.schedule.start_at
    });
    let normalized_change = update.details(&change).unwrap();
    let change_facts = update.describe(&normalized_change, &env).unwrap();
    assert!(
        change_facts
            .iter()
            .any(|fact| fact.name == "版本" && fact.value.contains("第 1 版 → 第 2 版"))
    );
    // 在别处先改一次，旧版本卡确认被拒。
    store
        .update_appointment(
            &appointment_id,
            created.version,
            calendar::AppointmentPatch {
                title: Some("别处改的".to_owned()),
                ..calendar::AppointmentPatch::default()
            },
            "elsewhere",
        )
        .unwrap();
    let stale = update.reconcile(&normalized_change, &change_facts, &env);
    assert_eq!(stale, Err("这条约定刚被改过".to_owned()));
    // 用新版本再提一次，执行成功。
    let current = store.appointment(&appointment_id).unwrap().unwrap();
    let mut fresh = change.clone();
    fresh["expectedVersion"] = json!(current.version);
    let normalized_fresh = update.details(&fresh).unwrap();
    update.reconcile(&normalized_fresh, &[], &env).unwrap();
    update.act(&normalized_fresh, &host).await.unwrap();
    let changed = store.appointment(&appointment_id).unwrap().unwrap();
    assert_eq!(changed.title, "别处改的");
    assert_eq!(changed.schedule.rule, "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR");
    host.shutdown().await;
}

#[tokio::test]
async fn appointment_cancel_tool_distinguishes_user_and_stopped() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let (store, first) = create_appointment(data, "用户取消");
    let (_store2, second) = create_appointment(data, "程序结束");
    let host = crate::common::open(crate::common::options(data)).await;
    let runtime = host.task_runtime().cloned().expect("runtime");
    let tool = service::AppointmentCancelTool::new(runtime);
    let cancelled = tool
        .execute(
            ToolCall {
                id: "call_cancel_user".to_owned(),
                name: "appointment_cancel".to_owned(),
                arguments: json!({ "appointmentId": first.id, "reason": "user" }).to_string(),
            },
            &context(data),
        )
        .await;
    assert!(!cancelled.is_error, "{}", cancelled.content);
    assert!(cancelled.content.contains("取消"));
    assert_eq!(
        store.appointment(&first.id).unwrap().unwrap().status,
        calendar::AppointmentStatus::Cancelled
    );
    let stopped = tool
        .execute(
            ToolCall {
                id: "call_cancel_stopped".to_owned(),
                name: "appointment_cancel".to_owned(),
                arguments: json!({ "appointmentId": second.id, "reason": "stopped" }).to_string(),
            },
            &context(data),
        )
        .await;
    assert!(!stopped.is_error, "{}", stopped.content);
    assert!(stopped.content.contains("结束"));
    assert_eq!(
        store.appointment(&second.id).unwrap().unwrap().status,
        calendar::AppointmentStatus::Ended
    );
    host.shutdown().await;
}

#[tokio::test]
async fn builtin_dream_pause_toggles_memory_enabled_and_cannot_cancel() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let host = crate::common::open(crate::common::options(data)).await;
    let response = host
        .dispatch(Call::CalendarRead(service::CalendarReadRequest::default()))
        .await
        .expect("读日历");
    let Response::CalendarRead(read) = response else {
        panic!("日历读响应");
    };
    let builtin = read
        .appointments
        .iter()
        .find(|appointment| appointment.id == "builtin:dream")
        .expect("内置做梦行");
    assert_eq!(builtin.title, "做梦");

    let pause = host
        .dispatch(Call::CalendarAction(service::CalendarActionRequest {
            appointment_id: "builtin:dream".to_owned(),
            action: service::CalendarActionKind::Pause,
        }))
        .await
        .expect("暂停做梦");
    let Response::CalendarAction(pause) = pause else {
        panic!("暂停响应");
    };
    assert_eq!(pause.appointment.status, "paused");
    let memory = host
        .dispatch(Call::MemoryRead(service::MemoryReadRequest::default()))
        .await
        .expect("读记忆");
    let Response::MemoryRead(memory) = memory else {
        panic!("记忆读响应");
    };
    assert!(!memory.dream.enabled, "暂停内置行要关掉记忆里的做梦开关");

    let cancel = host
        .dispatch(Call::CalendarAction(service::CalendarActionRequest {
            appointment_id: "builtin:dream".to_owned(),
            action: service::CalendarActionKind::Cancel,
        }))
        .await;
    assert!(cancel.is_err(), "内置做梦行不能取消");

    host.dispatch(Call::CalendarAction(service::CalendarActionRequest {
        appointment_id: "builtin:dream".to_owned(),
        action: service::CalendarActionKind::Resume,
    }))
    .await
    .expect("恢复做梦");
    let memory = host
        .dispatch(Call::MemoryRead(service::MemoryReadRequest::default()))
        .await
        .expect("再读记忆");
    let Response::MemoryRead(memory) = memory else {
        panic!("记忆读响应");
    };
    assert!(memory.dream.enabled, "恢复内置行要打开记忆里的做梦开关");
    host.shutdown().await;
}
