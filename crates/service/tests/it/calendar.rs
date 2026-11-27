//! 日历运行时——timer 最近一条、到点送达、启动补、全局暂停合并、协议读/动作。

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
            duration_minutes: None,
        })
        .unwrap()
}

fn watch_appointment(
    store: &calendar::CalendarStore,
    title: &str,
    source: calendar::WatchSource,
) -> calendar::Appointment {
    store
        .create_appointment(NewAppointment {
            title: title.to_owned(),
            condition: "盯着变化".to_owned(),
            schedule: Schedule {
                rule: "FREQ=SECONDLY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now_ms() - 5_000,
            },
            binding: Binding::Kind,
            watch: Some(source),
            cycle: None,
            duration_minutes: None,
        })
        .unwrap()
}

fn latest_run(store: &calendar::CalendarStore, id: &str) -> Option<calendar::Run> {
    store.runs(id).ok().and_then(|runs| runs.last().cloned())
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
    let near_runs = store.runs(&near.id).unwrap();
    let (sent_turn, wake_text) = snapshot
        .events()
        .iter()
        .find_map(|event| {
            let session::EventKind::UserMessage { turn, message, .. } = &event.kind else {
                return None;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return None;
            };
            rpc_id
                .as_deref()
                .filter(|rpc| rpc.starts_with(&prefix))
                .map(|_| {
                    (
                        i64::try_from(*turn).unwrap_or(i64::MAX),
                        message
                            .content
                            .iter()
                            .filter_map(model::ContentBlock::as_text)
                            .collect::<String>(),
                    )
                })
        })
        .expect("根会话日志里要有 appointment-wake 那条用户消息");
    assert!(
        wake_text.contains("appointment_wake") && wake_text.contains("read its facts first"),
        "appointment-wake 正文带第 20 条规则：{wake_text}"
    );
    let wake: serde_json::Value = serde_json::from_str(&wake_text).unwrap();
    let local = wake["appointment"]["nominal_local"].as_str().unwrap();
    assert!(
        local.contains('-') && local.contains(':'),
        "本地时间：{local}"
    );
    let late = wake["appointment"]["late"].as_str().unwrap();
    assert!(
        late.starts_with("比约定时间晚了") || late == "准时",
        "晚多久：{late}"
    );
    assert_eq!(
        near_runs[0].sent_turn,
        Some(sent_turn),
        "运行记录的 sent_turn 要等于那条唤醒消息的轮次"
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
            report_id: None,
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
async fn two_appointments_due_under_one_global_pause_both_use_the_pause_reason() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let first = appointment(&store, "暂停一", now - 5_000, "FREQ=SECONDLY");
    let second = appointment(&store, "暂停二", now - 5_000, "FREQ=SECONDLY");
    // 先排远点，开宿主后再进全局暂停，最后把两条都改成已到期，一起开闸。
    store
        .set_next_trigger(&first.id, Some(now + 3_600_000))
        .unwrap();
    store
        .set_next_trigger(&second.id, Some(now + 3_600_000))
        .unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        currency: None,
        paused: Some(true),
        seats: None,
        bash: None,
    }))
    .await
    .expect("进全局暂停");
    store
        .set_next_trigger(&first.id, Some(now_ms() + 100))
        .unwrap();
    store
        .set_next_trigger(&second.id, Some(now_ms() + 100))
        .unwrap();
    // 暂停期间到点，且攒出多次（FREQ=SECONDLY），再开闸。
    tokio::time::sleep(std::time::Duration::from_millis(3_200)).await;
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        currency: None,
        paused: Some(false),
        seats: None,
        bash: None,
    }))
    .await
    .expect("开闸");
    wait_until("两条都送", || {
        store
            .runs(&first.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
            && store
                .runs(&second.id)
                .unwrap()
                .iter()
                .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    for id in [&first.id, &second.id] {
        let run = store
            .runs(id)
            .unwrap()
            .into_iter()
            .find(|run| run.result == calendar::RunResult::Sent)
            .unwrap();
        assert!(
            run.reason.contains("全局暂停期间到点"),
            "同一次开闸的多条 due 都应写暂停原因：{}",
            run.reason
        );
    }
    host.shutdown().await;
}
#[tokio::test]
async fn missed_while_service_was_down_uses_the_missed_merge_reason() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = appointment(&store, "错过合并", now - 5_000, "FREQ=SECONDLY");
    store.set_next_trigger(&due.id, Some(now - 5_000)).unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    wait_until("补送一次", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    let run = store
        .runs(&due.id)
        .unwrap()
        .into_iter()
        .find(|run| run.result == calendar::RunResult::Sent)
        .unwrap();
    assert!(run.merged_count >= 4, "merged={}", run.merged_count);
    assert!(
        run.reason.contains("服务没在跑期间到点"),
        "服务停机错过的合并原因不能写成全局暂停：{}",
        run.reason
    );
    host.shutdown().await;
}

#[tokio::test]
async fn global_pause_merges_missed_occurrences_and_sends_once() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let mut settings = service::settings::Settings {
        plugins: service::settings::PluginsSettings::default(),
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
    wait_until("合并后只留一次运行", || {
        let runs = store.runs(&due.id).unwrap_or_default();
        runs.len() == 1 && runs[0].result == calendar::RunResult::Sent
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
async fn file_watch_sends_diff_then_unchanged_skips() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let file = data.join("watch.txt");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let due = watch_appointment(
        &store,
        "盯文件",
        calendar::WatchSource::File(file.to_string_lossy().into_owned()),
    );
    let now = now_ms();
    store.set_next_trigger(&due.id, Some(now - 1)).unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    wait_until("先记下基线", || {
        !store.runs(&due.id).unwrap().is_empty()
    })
    .await;
    let baseline_runs = store.runs(&due.id).unwrap().len();
    // 内容变了：再排一次，带新行 diff。
    std::fs::write(&file, "alpha\ngamma\n").unwrap();
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("文件变化送到", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    assert!(
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent && run.reason.contains("gamma")),
        "变了要带 gamma 的 diff"
    );
    let after_change = store.runs(&due.id).unwrap().len();
    assert!(after_change > baseline_runs);
    let session_id = host.info().session_id.expect("根会话");
    let snapshot = session::SessionStore::open_read_only(
        data,
        &session::SessionId::parse(&session_id).expect("会话 id"),
    )
    .expect("会话日志");
    assert!(
        snapshot.events().iter().any(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return false;
            };
            let text = serde_json::to_string(message).unwrap_or_default();
            text.contains("appointment-wake") && text.contains("change") && text.contains("gamma")
        }),
        "wake 正文要带变了的内容"
    );
    // 内容没变：记跳过，不再送。
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("没变记跳过", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Skipped)
    })
    .await;
    assert!(
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Skipped && run.reason.contains("没变")),
        "没变要记固定说法"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn failed_file_read_keeps_the_previous_content() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let file = data.join("watch.txt");
    std::fs::write(&file, "same\n").unwrap();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let due = watch_appointment(
        &store,
        "失败不改上次",
        calendar::WatchSource::File(file.to_string_lossy().into_owned()),
    );
    store.set_next_trigger(&due.id, Some(now_ms() - 1)).unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    wait_until("基线记下", || !store.runs(&due.id).unwrap().is_empty()).await;
    std::fs::remove_file(&file).unwrap();
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("失败记没送成", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Failed)
    })
    .await;
    let failed = latest_run(&store, &due.id).unwrap();
    assert!(failed.reason.contains("没送成"), "{}", failed.reason);
    // 恢复同样的内容：上次内容没被失败覆盖，应记「没变」。
    std::fs::write(&file, "same\n").unwrap();
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("恢复后认为没变", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Skipped)
    })
    .await;
    assert!(
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Skipped && run.reason.contains("没变"))
    );
    host.shutdown().await;
}

#[tokio::test]
async fn task_facts_watch_sends_then_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = {
        let tasks = tasks::Store::open(data.join("tasks.sqlite")).unwrap();
        let draft = tasks::NewTask {
            quote: None,
            quote_source: None,
            kind_note: None,
            project_id: None,
            cwd_source: None,
            title: "盯任务事实".to_owned(),
            requirement: "看它变没变".to_owned(),
            dod: "DoD".to_owned(),
            cwd: data.to_path_buf(),
            conversation: tasks::ConversationId::new("kind-calendar-watch"),
            model: tasks::ModelChoice {
                connection: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                effort_wanted: "high".to_owned(),
            },
            tools: vec!["read".to_owned()],
        };
        let task = tasks.create_task(draft, now_ms()).unwrap();
        // 本用例只造任务事实，不跑任务：先把自动排的 launch 收掉，免得宿主起来后写 blocked。
        if let Ok(Some(claim)) = tasks.claim_effect_ordered(tasks::EffectKind::Launch, now_ms()) {
            tasks.complete_effect(&claim).unwrap();
        }
        let attempt = tasks.begin_attempt(&task.id, now_ms()).unwrap();
        tasks
            .record_report(
                &task.id,
                attempt,
                tasks::NewReport {
                    state: tasks::ReportState::Ready,
                    origin: tasks::ReportOrigin::Agent,
                    summary: "第一份结果".to_owned(),
                    question: None,
                },
                now_ms(),
            )
            .unwrap();
        tasks
            .end_attempt(&task.id, attempt, now_ms(), tasks::AttemptEnd::Reported)
            .unwrap();
        task.id.to_string()
    };
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let due = watch_appointment(
        &store,
        "盯任务",
        calendar::WatchSource::TaskFacts(task_id.clone()),
    );
    store.set_next_trigger(&due.id, Some(now_ms() - 1)).unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    wait_until("基线记下", || !store.runs(&due.id).unwrap().is_empty()).await;
    // 任务事实变了：再加一份报告。
    {
        let tasks = tasks::Store::open(data.join("tasks.sqlite")).unwrap();
        let task = tasks.task(&tasks::TaskId::from(task_id)).unwrap().unwrap();
        let attempt = tasks.begin_attempt(&task.id, now_ms()).unwrap();
        tasks
            .record_report(
                &task.id,
                attempt,
                tasks::NewReport {
                    state: tasks::ReportState::Ready,
                    origin: tasks::ReportOrigin::Agent,
                    summary: "第二份结果".to_owned(),
                    question: None,
                },
                now_ms(),
            )
            .unwrap();
        tasks
            .end_attempt(&task.id, attempt, now_ms(), tasks::AttemptEnd::Reported)
            .unwrap();
    }
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("任务事实变化送到", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    assert!(
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent && run.reason.contains("第二份结果")),
        "任务事实变了要带新报告"
    );
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("任务事实没变", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Skipped)
    })
    .await;
    assert!(
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Skipped && run.reason.contains("没变"))
    );
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
            report_id: None,
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
async fn resume_from_paused_wakes_a_pump_that_was_sleeping() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = appointment(&store, "暂停后恢复", now + 50, "FREQ=SECONDLY");
    store
        .set_status(&due.id, calendar::AppointmentStatus::Paused)
        .unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(
        store
            .appointment(&due.id)
            .unwrap()
            .unwrap()
            .next_trigger_at
            .is_none()
    );
    let version = store.appointment(&due.id).unwrap().unwrap().version;
    store
        .update_appointment(
            &due.id,
            version,
            calendar::AppointmentPatch {
                schedule: Some(calendar::Schedule {
                    rule: "FREQ=SECONDLY".to_owned(),
                    timezone: "Asia/Shanghai".to_owned(),
                    start_at: now_ms() + 50,
                }),
                ..calendar::AppointmentPatch::default()
            },
            "test",
        )
        .unwrap();
    let started = std::time::Instant::now();
    host.dispatch(Call::CalendarAction(CalendarActionRequest {
        appointment_id: due.id.clone(),
        action: CalendarActionKind::Resume,
    }))
    .await
    .expect("恢复");
    let deadline = std::time::Duration::from_millis(1_500);
    loop {
        if store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
        {
            break;
        }
        assert!(
            started.elapsed() < deadline,
            "恢复没有及时叫醒泵（{:?} 还没送）",
            started.elapsed()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
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
async fn one_off_appointment_ends_after_its_run() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = appointment(&store, "一次性提醒", now - 1_000, "FREQ=DAILY;COUNT=1");
    store.set_next_trigger(&due.id, Some(now - 1)).unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    wait_until("一次性送到", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    wait_until("一次性结束", || {
        store.appointment(&due.id).unwrap().unwrap().status == calendar::AppointmentStatus::Ended
    })
    .await;
    let ended = store.appointment(&due.id).unwrap().unwrap();
    assert!(ended.next_trigger_at.is_none());
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
    let rows: Vec<_> = read
        .appointments
        .iter()
        .filter(|row| row.id != "builtin:dream")
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, appointment.id);
    assert_eq!(rows[0].status, "active");

    let week = host
        .dispatch(Call::CalendarRead(CalendarReadRequest {
            appointment_id: None,
            from: Some(now_ms() - 2 * 24 * 60 * 60 * 1000),
            to: Some(now_ms() + 5 * 24 * 60 * 60 * 1000),
        }))
        .await
        .expect("日历周计划");
    let Response::CalendarRead(week) = week else {
        panic!("日历周读响应");
    };
    assert!(!week.plans.is_empty(), "区间要有计划块：{:?}", week.plans);
    assert!(!week.colors.is_empty(), "每条规则要有颜色序号");

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

#[tokio::test]
async fn url_watch_sends_baseline_changed_unchanged_and_keeps_state_on_failure() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let v1 = "第一版\n内容\n".to_owned();
    let v2 = (0..400)
        .map(|index| {
            format!(
                "第二版第 {index} 行，这一行故意写长一点，让完整 diff 超过四千字，好检查附件。\n"
            )
        })
        .collect::<String>();
    Mock::given(method("GET"))
        .and(path("/v1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(v1.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(v2.clone()))
        .mount(&server)
        .await;

    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let due = store
        .create_appointment(NewAppointment {
            title: "盯网页".to_owned(),
            condition: "网页变了告诉我".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now,
            },
            binding: Binding::Kind,
            watch: Some(calendar::WatchSource::Url(
                "http://93.184.216.34/v1".to_owned(),
            )),
            cycle: None,
            duration_minutes: None,
        })
        .unwrap();
    // 先排远点，等测试夹具装好代理再让它到点。
    store
        .set_next_trigger(&due.id, Some(now + 3_600_000))
        .unwrap();
    let host = crate::common::open(crate::common::options(data)).await;
    host.set_calendar_watch_proxy(server.uri());
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();

    wait_until("URL 基线", || !store.runs(&due.id).unwrap().is_empty()).await;
    let baseline = latest_run(&store, &due.id).unwrap();
    assert_eq!(
        baseline.result,
        calendar::RunResult::Skipped,
        "第一次只记基线"
    );
    assert!(baseline.reason.contains("没变"), "{}", baseline.reason);
    let state = store.watch_state(&due.id).unwrap().expect("基线状态");
    assert!(state.content.contains("第一版"), "{}", state.content);
    assert!(state.attachment_id.is_none());

    // 同一版内容再来一次：不送、记没变、状态不变（还没送过 Kind，不会撞 Kind 忙）。
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("URL 没变", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .filter(|run| run.result == calendar::RunResult::Skipped && run.reason.contains("没变"))
            .count()
            >= 2
    })
    .await;
    assert_eq!(
        store.watch_state(&due.id).unwrap().unwrap().content,
        state.content
    );

    // 抓取失败：记失败，不覆盖上次内容。
    let version = store.appointment(&due.id).unwrap().unwrap().version;
    store
        .update_appointment(
            &due.id,
            version,
            calendar::AppointmentPatch {
                watch: Some(Some(calendar::WatchSource::Url(
                    "http://93.184.216.34/missing".to_owned(),
                ))),
                ..calendar::AppointmentPatch::default()
            },
            "test",
        )
        .unwrap();
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("URL 抓取失败", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Failed)
    })
    .await;
    let failed = latest_run(&store, &due.id).unwrap();
    assert!(failed.reason.contains("没送成"), "{}", failed.reason);
    let state_after_failure = store.watch_state(&due.id).unwrap().unwrap();
    assert_eq!(state_after_failure.content, state.content);
    assert_eq!(state_after_failure.attachment_id, state.attachment_id);

    // 换到第二版：内容变了，带 diff 与长 diff 附件，wake 正文带 change 摘要和附件 id。
    let version = store.appointment(&due.id).unwrap().unwrap().version;
    store
        .update_appointment(
            &due.id,
            version,
            calendar::AppointmentPatch {
                watch: Some(Some(calendar::WatchSource::Url(
                    "http://93.184.216.34/v2".to_owned(),
                ))),
                ..calendar::AppointmentPatch::default()
            },
            "test",
        )
        .unwrap();
    store.set_next_trigger(&due.id, Some(now_ms())).unwrap();
    host.nudge_calendar();
    wait_until("URL 变化送到", || {
        store
            .runs(&due.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    let changed = store
        .runs(&due.id)
        .unwrap()
        .into_iter()
        .find(|run| run.result == calendar::RunResult::Sent)
        .unwrap();
    assert!(changed.reason.contains("变了"), "{}", changed.reason);
    let state = store.watch_state(&due.id).unwrap().expect("变化后状态");
    assert!(
        state.content.contains("第二版第 399 行"),
        "{}",
        state.content
    );
    let attachment_id = state.attachment_id.clone().expect("长 diff 要存附件");
    assert!(attachment_id.starts_with("sha256:"));

    let session_id = host.info().session_id.expect("根会话");
    let session_id = session::SessionId::parse(&session_id).expect("会话 id");
    // 运行结果落库和会话日志追加不是一个原子动作；等 wake 事件真的落日志，别抢跑。
    wait_until("wake 要带 change.attachment_id", || {
        let Ok(snapshot) = session::SessionStore::open_read_only(data, &session_id) else {
            return false;
        };
        snapshot.events().iter().any(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return false;
            };
            let text = serde_json::to_string(message).unwrap_or_default();
            text.contains("appointment-wake")
                && text.contains("change")
                && text.contains(&attachment_id)
        })
    })
    .await;

    host.shutdown().await;
}
