//! Kind 的日历面——一个查询、建/改确认卡事实与旧版本拒绝、直接取消的原因区分。

use std::path::Path;
use std::sync::Arc;

use calendar::{Binding, NewAppointment, Schedule};
use serde_json::{Value, json};
use service::{ActionEnv, ActionRegistry, Call, Response};
use tools::{Tool, ToolCall, ToolContext, ToolSessionState};

async fn record_user_words(host: &service::RootHost, text: &str) {
    host.append_user_message_for_test(text)
        .await
        .expect("写入用户原话");
}

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
            duration_minutes: None,
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
    let trigger = value["appointments"][0]["trigger"]
        .as_str()
        .expect("中文触发");
    assert!(trigger.contains("每天"), "{trigger}");
    assert!(!trigger.contains("FREQ="), "{trigger}");
}

#[tokio::test]
async fn propose_create_and_update_facts_and_stale_version_refusal() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let mut options = crate::common::options(data);
    options.actions = Arc::new(ActionRegistry::settings(data.to_path_buf(), None));
    let host = crate::common::open(options).await;
    let registry = ActionRegistry::settings(data.to_path_buf(), None);
    let session_id = host.info().session_id.expect("根会话");
    let env = ActionEnv {
        data_dir: data,
        identity: None,
        events: &[],
        conversation: Some(&session_id),
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
    record_user_words(&host, "每天九点提醒我喝水").await;
    let normalized = create.details(&details).unwrap();
    let facts = create.describe(&normalized, &env).unwrap();
    assert!(facts.iter().any(|fact| fact.name == "触发"
        && fact.value.contains("每天")
        && !fact.value.contains("FREQ=")));
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
    let unsaid_condition = json!({
        "appointmentId": appointment_id,
        "expectedVersion": created.version,
        "condition": "用户没说过的新条件"
    });
    let normalized_unsaid = update.details(&unsaid_condition).unwrap();
    let error = update
        .describe(&normalized_unsaid, &env)
        .expect_err("改 condition 也要用户原话");
    assert!(error.contains("约定的条件要用用户原话"), "{error}");
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
async fn calendar_read_carries_source_turn_and_old_records_stay_empty() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let (store, with_turn) = create_appointment(data, "有来源轮");
    let (_same, without_turn) = create_appointment(data, "旧记录");
    store.set_source_turn(&with_turn.id, 5).unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    let response = host
        .dispatch(Call::CalendarRead(service::CalendarReadRequest::default()))
        .await
        .expect("读日历");
    let Response::CalendarRead(read) = response else {
        panic!("日历读响应");
    };
    let row = read
        .appointments
        .iter()
        .find(|row| row.id == with_turn.id)
        .expect("有来源轮");
    assert_eq!(row.source_turn, Some(5));
    let old = read
        .appointments
        .iter()
        .find(|row| row.id == without_turn.id)
        .expect("旧记录");
    assert_eq!(old.source_turn, None);
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

#[tokio::test]
async fn builtin_run_now_triggers_a_dream_run_and_calendar_changed() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let host = crate::common::open(crate::common::options(data)).await;
    // 允许做梦观察，否则 request_now 只出许可卡、不写 DreamRun。
    {
        let store = memory::MemoryStore::open(data.join("memory.sqlite")).expect("记忆库");
        store
            .update_dream_settings(&memory::types::DreamSettingsPatch {
                enabled: Some(true),
                permit: Some(true),
                ..memory::types::DreamSettingsPatch::default()
            })
            .expect("打开做梦许可");
    }
    let mut events = host.subscribe_events();
    let outcome = host
        .dispatch(Call::CalendarAction(service::CalendarActionRequest {
            appointment_id: "builtin:dream".to_owned(),
            action: service::CalendarActionKind::RunNow,
        }))
        .await
        .expect("立即跑一次");
    let Response::CalendarAction(outcome) = outcome else {
        panic!("立即跑响应");
    };
    assert_eq!(outcome.appointment.status, "active");
    // 日历内置行改完要推 calendar/changed。
    let pushed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
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
        pushed
            .data
            .get("appointmentId")
            .and_then(|value| value.as_str()),
        Some("builtin:dream")
    );
    // 真起了做梦：DreamRun 至少一条（模型端点不可达时也记 failed，不丢记录）。
    let store = memory::MemoryStore::open(data.join("memory.sqlite")).expect("记忆库");
    for _ in 0..200 {
        let runs = store.dream_runs(5).unwrap_or_default();
        if !runs.is_empty() {
            host.shutdown().await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("立即跑一次之后没有 DreamRun 记录");
}

#[tokio::test]
async fn create_card_accepts_number_string_start_delay_and_binding_objects() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let mut options = crate::common::options(data);
    options.actions = Arc::new(ActionRegistry::settings(data.to_path_buf(), None));
    let host = crate::common::open(options).await;
    let registry = ActionRegistry::settings(data.to_path_buf(), None);
    let session_id = host.info().session_id.expect("根会话");
    let env = ActionEnv {
        data_dir: data,
        identity: None,
        events: &[],
        conversation: Some(&session_id),
    };
    let create = registry.get("create_appointment").expect("create 动作");

    // D78 现场的形态：数字字符串 startAt + binding 对象（kind=chat）；用未来一分钟避免测试跑着就过期。
    let start_at = now_ms() + 60_000;
    let raw = json!({
        "title": "喝水提醒",
        "condition": "一分钟后提醒我喝水",
        "rule": "FREQ=DAILY",
        "timezone": "Asia/Shanghai",
        "startAt": start_at.to_string(),
        "binding": { "kind": "chat" },
        "durationMinutes": 45
    });
    record_user_words(&host, "一分钟后提醒我喝水").await;
    let normalized = create
        .details(&raw)
        .expect("数字字符串 startAt 和 binding 对象要过");
    assert_eq!(normalized["startAt"], json!(start_at));
    assert_eq!(normalized["binding"]["kind"], "kind");
    let facts = create.describe(&normalized, &env).expect("建卡事实");
    assert!(
        facts
            .iter()
            .any(|fact| fact.name == "原话" && fact.value.contains("喝水"))
    );
    assert!(facts.iter().any(|fact| fact.name == "触发"));
    assert!(
        facts
            .iter()
            .any(|fact| fact.name == "绑定" && fact.value.contains("Kind"))
    );
    let outcome = create.act(&normalized, &host).await.expect("按卡建约定");
    let appointment_id = outcome.data["appointmentId"].as_str().unwrap().to_owned();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let created = store.appointment(&appointment_id).unwrap().unwrap();
    assert_eq!(created.schedule.start_at, start_at);
    assert_eq!(created.binding, calendar::Binding::Kind);
    assert_eq!(created.duration_minutes, Some(45));
    // 起点在未来：DTSTART 就是第一次触发，next_trigger_at 不能落到次日。
    assert_eq!(
        created.next_trigger_at,
        Some(start_at),
        "未来起点的 next_trigger_at 必须是 start_at 本身"
    );
    let expected_local = chrono::DateTime::from_timestamp_millis(start_at)
        .unwrap()
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    assert!(
        facts
            .iter()
            .any(|fact| fact.name == "触发" && fact.value.contains(&expected_local)),
        "卡上的触发时间要按约定时区显示：{facts:?}（期望含 {expected_local}）"
    );
    eprintln!(
        "D78 手工检查：start_at={} next_trigger_at={:?} 卡事实={:?}",
        created.schedule.start_at, created.next_trigger_at, facts
    );

    // delaySeconds 替代 startAt + binding 任务对象。
    let before = now_ms();
    let delayed = json!({
        "title": "稍后提醒",
        "condition": "一分钟后提醒",
        "rule": "FREQ=DAILY",
        "timezone": "Asia/Shanghai",
        "delaySeconds": "60",
        "binding": { "kind": "task", "taskId": "task-demo" }
    });
    let normalized = create
        .details(&delayed)
        .expect("delaySeconds + binding 任务对象要过");
    let start_at = normalized["startAt"].as_i64().expect("绝对时间");
    assert!(
        (start_at - (before + 60_000)).abs() < 5_000,
        "startAt={start_at}"
    );
    assert_eq!(normalized["binding"]["kind"], "task");
    assert_eq!(normalized["binding"]["taskId"], "task-demo");
    let outcome = create
        .act(&normalized, &host)
        .await
        .expect("按卡建延迟约定");
    let appointment_id = outcome.data["appointmentId"].as_str().unwrap().to_owned();
    let created = store.appointment(&appointment_id).unwrap().unwrap();
    assert_eq!(
        created.binding,
        calendar::Binding::Task("task-demo".to_owned())
    );
    assert!((created.schedule.start_at - (before + 60_000)).abs() < 5_000);
    assert_eq!(
        created.next_trigger_at,
        Some(created.schedule.start_at),
        "一分钟后新建：next_trigger_at 不能是次日"
    );

    // 不明 binding kind 不再静默回落 Kind，直接拒。
    let bad = json!({
        "title": "坏绑定",
        "condition": "坏绑定",
        "rule": "FREQ=DAILY",
        "delaySeconds": 5,
        "binding": { "kind": "nope" }
    });
    let error = create.details(&bad).unwrap_err();
    assert!(error.contains("只认 chat/kind/task"), "{error}");
    host.shutdown().await;
}

#[tokio::test]
async fn card_act_records_the_kind_turn_that_proposed_it() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let mut options = crate::common::options(data);
    options.actions = Arc::new(ActionRegistry::settings(data.to_path_buf(), None));
    let host = crate::common::open(options).await;
    host.dispatch(Call::Send(service::SendRequest {
        request_id: "source-turn".to_owned(),
        content: "明天九点提醒我".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        object_refs: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
    }))
    .await
    .expect("发一句让根会话开一轮");
    let session_id = host.info().session_id.expect("根会话");
    let id = session::SessionId::parse(&session_id).expect("会话 id");
    let mut saw_turn = false;
    for _ in 0..100 {
        if session::SessionStore::open_read_only(data, &id).is_ok_and(|store| {
            store
                .events()
                .iter()
                .any(|event| matches!(&event.kind, session::EventKind::TurnStart { .. }))
        }) {
            saw_turn = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(saw_turn, "根会话没有开出一轮");

    let registry = ActionRegistry::settings(data.to_path_buf(), None);
    let create = registry.get("create_appointment").expect("create 动作");
    let details = create
        .details(&json!({
            "title": "来源轮提醒",
            "condition": "明天九点提醒我",
            "rule": "FREQ=DAILY",
            "timezone": "Asia/Shanghai",
            "delaySeconds": 60,
            "binding": "kind"
        }))
        .unwrap();
    record_user_words(&host, "明天九点提醒我").await;
    let outcome = create.act(&details, &host).await.expect("按卡建约定");
    let appointment_id = outcome.data["appointmentId"].as_str().unwrap().to_owned();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let created = store.appointment(&appointment_id).unwrap().unwrap();
    assert_eq!(created.source_turn, Some(1), "确认卡 act 要写发起轮");
    host.shutdown().await;
}

#[tokio::test]
async fn create_appointment_rejects_a_condition_the_user_never_said() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let mut options = crate::common::options(data);
    options.actions = Arc::new(ActionRegistry::settings(data.to_path_buf(), None));
    let host = crate::common::open(options).await;
    let registry = ActionRegistry::settings(data.to_path_buf(), None);
    let session_id = host.info().session_id.expect("根会话");
    let env = ActionEnv {
        data_dir: data,
        identity: None,
        events: &[],
        conversation: Some(&session_id),
    };
    let create = registry.get("create_appointment").expect("create 动作");
    let details = create
        .details(&json!({
            "title": "伪造约定",
            "condition": "用户没说过这句",
            "rule": "FREQ=DAILY",
            "timezone": "Asia/Shanghai",
            "delaySeconds": 60,
            "binding": "kind"
        }))
        .unwrap();
    let error = create.describe(&details, &env).expect_err("非用户原话要拒");
    assert!(error.contains("约定的条件要用用户原话"), "{error}");
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    assert!(store.list_appointments().unwrap().is_empty());
    host.shutdown().await;
}
