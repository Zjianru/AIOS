//! 循环约定——报回后送、程序事实暂停进等你、task_continue 准入、结束/恢复。

use std::time::{SystemTime, UNIX_EPOCH};

use calendar::{Binding, CycleSpec, NewAppointment, Schedule};
use service::{
    CalendarActionKind, CalendarActionRequest, Call, DeliveryMode, Response, SendRequest,
    TaskContinueRequest,
};

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

fn create_task(data: &std::path::Path) -> tasks::TaskId {
    let tasks = tasks::Store::open(data.join("tasks.sqlite")).unwrap();
    let task = tasks
        .create_task(
            tasks::NewTask {
                quote: None,
                quote_source: None,
                kind_note: None,
                project_id: None,
                cwd_source: None,
                title: "循环任务".to_owned(),
                requirement: "一轮一轮看".to_owned(),
                dod: "DoD".to_owned(),
                cwd: data.to_path_buf(),
                conversation: tasks::ConversationId::new("kind-cycle"),
                model: tasks::ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: Vec::new(),
            },
            now_ms(),
        )
        .unwrap();
    // 直接建的任务会排 launch；本测试只造事实，先把它收掉，免得宿主起来后真去开一轮。
    if let Ok(Some(claim)) = tasks.claim_effect_ordered(tasks::EffectKind::Launch, now_ms()) {
        tasks.complete_effect(&claim).unwrap();
    }
    task.id
}

fn add_report(
    data: &std::path::Path,
    task_id: &tasks::TaskId,
    state: tasks::ReportState,
    summary: &str,
) {
    let tasks = tasks::Store::open(data.join("tasks.sqlite")).unwrap();
    let attempt = tasks.begin_attempt(task_id, now_ms()).unwrap();
    tasks
        .record_report(
            task_id,
            attempt,
            tasks::NewReport {
                state,
                origin: tasks::ReportOrigin::Agent,
                summary: summary.to_owned(),
                question: None,
            },
            now_ms(),
        )
        .unwrap();
    tasks
        .end_attempt(task_id, attempt, now_ms(), tasks::AttemptEnd::Reported)
        .unwrap();
}

fn add_report_with_session(
    data: &std::path::Path,
    task_id: &tasks::TaskId,
    state: tasks::ReportState,
    summary: &str,
    session: &str,
) {
    let tasks = tasks::Store::open(data.join("tasks.sqlite")).unwrap();
    let attempt = tasks.begin_attempt(task_id, now_ms()).unwrap();
    tasks
        .attach_session(task_id, attempt, tasks::ConversationId::new(session))
        .unwrap();
    tasks
        .record_report(
            task_id,
            attempt,
            tasks::NewReport {
                state,
                origin: tasks::ReportOrigin::Agent,
                summary: summary.to_owned(),
                question: None,
            },
            now_ms(),
        )
        .unwrap();
    tasks
        .end_attempt(task_id, attempt, now_ms(), tasks::AttemptEnd::Reported)
        .unwrap();
}

fn cycle_appointment_with_max(
    data: &std::path::Path,
    task_id: &str,
    max_rounds: Option<u32>,
) -> (calendar::CalendarStore, calendar::Appointment) {
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let appointment = store
        .create_appointment(NewAppointment {
            title: "循环约定".to_owned(),
            condition: "每轮报回后接着做".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now,
            },
            binding: Binding::Task(task_id.to_owned()),
            watch: None,
            cycle: Some(CycleSpec {
                task_id: task_id.to_owned(),
                max_rounds,
            }),
            duration_minutes: None,
        })
        .unwrap();
    store
        .set_next_trigger(&appointment.id, Some(now + 3_600_000))
        .unwrap();
    (store, appointment)
}

fn cycle_appointment(
    data: &std::path::Path,
    task_id: &str,
) -> (calendar::CalendarStore, calendar::Appointment) {
    cycle_appointment_with_max(data, task_id, None)
}

#[tokio::test]
async fn max_rounds_pauses_into_wait_answer() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = create_task(data);
    add_report(data, &task_id, tasks::ReportState::Ready, "第一轮");
    let (store, appointment) = cycle_appointment_with_max(data, task_id.as_str(), Some(1));
    let host = crate::common::open(crate::common::options(data)).await;
    host.calendar_task_report(task_id.as_str());
    wait_until("第一轮送到", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    add_report(data, &task_id, tasks::ReportState::Ready, "第二轮");
    host.calendar_task_report(task_id.as_str());
    wait_until("到轮数暂停", || {
        store.appointment(&appointment.id).unwrap().unwrap().status
            == calendar::AppointmentStatus::Paused
    })
    .await;
    assert!(
        store
            .latest_version_reason(&appointment.id)
            .unwrap()
            .unwrap()
            .contains("到了约定的轮数")
    );
    let response = host
        .dispatch(Call::Work(service::WorkRequest::default()))
        .await
        .expect("work");
    let Response::Work(work) = response else {
        panic!("work")
    };
    assert!(work.wait.iter().any(|item| {
        item.appointment_id.as_deref() == Some(appointment.id.as_str())
            && item.need.contains("到了约定的轮数")
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn root_forward_recognizes_a_real_task_report_message() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = create_task(data);
    add_report(data, &task_id, tasks::ReportState::Ready, "报回了");
    let (store, appointment) = cycle_appointment(data, task_id.as_str());
    let host = crate::common::open(crate::common::options(data)).await;
    host.dispatch(Call::Send(SendRequest {
        request_id: format!("task-report:{}:manual", task_id.as_str()),
        content: "任务报回了".to_owned(),
        delivery: DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        object_refs: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
    }))
    .await
    .expect("发送报回事件");
    wait_until("root forward 触发循环唤醒", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    host.shutdown().await;
}

#[tokio::test]
async fn task_continue_goes_through_real_admission_and_rejects_stale_reports() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = create_task(data);
    // 合成真的开过会话、已报回的 Attempt，让 continue 有会话可接。
    add_report_with_session(
        data,
        &task_id,
        tasks::ReportState::Ready,
        "第一轮",
        "00000000-0000-4000-8000-000000000001",
    );
    let (store, appointment) = cycle_appointment(data, task_id.as_str());
    let host = crate::common::open(crate::common::options(data)).await;
    host.calendar_task_report(task_id.as_str());
    wait_until("报回送到", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    // 新报告还没被循环处理：旧 run 不能给新报告当准入。
    add_report_with_session(
        data,
        &task_id,
        tasks::ReportState::Ready,
        "第二轮",
        "00000000-0000-4000-8000-000000000002",
    );
    let stale = host
        .dispatch(Call::TaskContinue(TaskContinueRequest {
            task_id: task_id.as_str().to_owned(),
            note: None,
        }))
        .await;
    assert!(stale.is_err(), "新报告没处理前不该按旧 run 准入");
    // 处理第二轮后可准入。
    host.calendar_task_report(task_id.as_str());
    wait_until("第二轮送到", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .filter(|run| run.result == calendar::RunResult::Sent)
            .count()
            >= 2
    })
    .await;
    let admitted = host
        .dispatch(Call::TaskContinue(TaskContinueRequest {
            task_id: task_id.as_str().to_owned(),
            note: None,
        }))
        .await;
    // 日历准入核通过了，才会走到任务包检查；任务包缺项是后续步骤的拒绝，不是没认出循环约定。
    let admitted_error = admitted.expect_err("合成的任务包不完整，应在准入后因缺项被拒");
    assert!(
        admitted_error.to_string().contains("任务包缺"),
        "应已通过日历准入、进到任务包检查：{admitted_error}"
    );
    // 绑定别的约定的 task_continue 应拒。
    let other = create_task(data);
    let rejected = host
        .dispatch(Call::TaskContinue(TaskContinueRequest {
            task_id: other.as_str().to_owned(),
            note: None,
        }))
        .await;
    assert!(rejected.is_err(), "没有循环约定的任务不该按循环到点准入");
    host.shutdown().await;
}

#[tokio::test]
async fn report_after_cycle_sends_then_task_continue_accepts_cycle_due() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = create_task(data);
    add_report(data, &task_id, tasks::ReportState::Ready, "第一轮结果");
    let (store, appointment) = cycle_appointment(data, task_id.as_str());
    let host = crate::common::open(crate::common::options(data)).await;
    host.calendar_task_report(task_id.as_str());
    wait_until("报回后送到", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    let run = store.runs(&appointment.id).unwrap().pop().unwrap();
    assert_eq!(run.trigger, calendar::TriggerKind::Report);
    assert!(run.reason.contains("第 1 轮报回"), "{}", run.reason);
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
            serde_json::to_string(message)
                .unwrap_or_default()
                .contains("第 1 轮报回")
        }),
        "根会话日志里要有第 1 轮报回"
    );

    // Kind 读账本后调 task_continue：服务准入核认出「循环约定到点」。
    let cause = host.calendar_cycle_cause(task_id.as_str());
    assert!(
        cause
            .as_deref()
            .is_some_and(|cause| cause.contains("CycleDue")),
        "task_continue 应该按循环约定到点准入：{cause:?}"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn blocked_report_pauses_into_wait_answer() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = create_task(data);
    add_report(data, &task_id, tasks::ReportState::Blocked, "卡住了");
    let (store, appointment) = cycle_appointment(data, task_id.as_str());
    let host = crate::common::open(crate::common::options(data)).await;
    host.calendar_task_report(task_id.as_str());
    wait_until("暂停", || {
        store.appointment(&appointment.id).unwrap().unwrap().status
            == calendar::AppointmentStatus::Paused
    })
    .await;
    assert!(
        store
            .latest_version_reason(&appointment.id)
            .unwrap()
            .unwrap()
            .contains("卡住了")
    );
    let response = host
        .dispatch(Call::Work(service::WorkRequest::default()))
        .await
        .expect("工作快照");
    let Response::Work(work) = response else {
        panic!("工作快照响应");
    };
    let wait = work
        .wait
        .iter()
        .find(|item| item.appointment_id.as_deref() == Some(appointment.id.as_str()))
        .expect("等你里要有循环约定暂停项");
    assert_eq!(wait.kind, "answer");
    assert!(wait.need.contains("卡住了"), "{}", wait.need);
    assert_eq!(wait.task_id.as_deref(), Some(task_id.as_str()));
    host.shutdown().await;
}

#[tokio::test]
async fn failed_report_pauses_with_the_failed_reason() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = create_task(data);
    add_report(data, &task_id, tasks::ReportState::Failed, "跑挂了");
    let (store, appointment) = cycle_appointment(data, task_id.as_str());
    let host = crate::common::open(crate::common::options(data)).await;
    host.calendar_task_report(task_id.as_str());
    wait_until("失败暂停", || {
        store.appointment(&appointment.id).unwrap().unwrap().status
            == calendar::AppointmentStatus::Paused
    })
    .await;
    assert!(
        store
            .latest_version_reason(&appointment.id)
            .unwrap()
            .unwrap()
            .contains("失败")
    );
    host.shutdown().await;
}

#[tokio::test]
async fn three_same_signatures_pause_the_cycle() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = create_task(data);
    for _ in 0..3 {
        add_report(data, &task_id, tasks::ReportState::Ready, "一模一样");
    }
    let (store, appointment) = cycle_appointment(data, task_id.as_str());
    let host = crate::common::open(crate::common::options(data)).await;
    host.calendar_task_report(task_id.as_str());
    wait_until("连续没进展暂停", || {
        store.appointment(&appointment.id).unwrap().unwrap().status
            == calendar::AppointmentStatus::Paused
    })
    .await;
    assert!(
        store
            .latest_version_reason(&appointment.id)
            .unwrap()
            .unwrap()
            .contains("连续 3 轮没有进展")
    );
    host.shutdown().await;
}

#[tokio::test]
async fn closed_task_ends_the_cycle_and_resume_only_changes_status() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let task_id = create_task(data);
    add_report(data, &task_id, tasks::ReportState::Ready, "可以交了");
    let (store, appointment) = cycle_appointment(data, task_id.as_str());
    let host = crate::common::open(crate::common::options(data)).await;
    host.calendar_task_report(task_id.as_str());
    wait_until("关闭前先送一轮", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;

    let tasks = tasks::Store::open(data.join("tasks.sqlite")).unwrap();
    let task = tasks.task(&task_id).unwrap().unwrap();
    let latest = task.latest_report().unwrap();
    let report = latest.id.clone();
    tasks.accept(&task_id, &report, now_ms()).unwrap();
    drop(tasks);
    host.calendar_task_report(task_id.as_str());
    wait_until("任务关闭→约定结束", || {
        store.appointment(&appointment.id).unwrap().unwrap().status
            == calendar::AppointmentStatus::Ended
    })
    .await;

    // 恢复只把状态改回在用（这里用另一条暂停的约定验证动作）。
    let (store2, paused) = {
        let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
        let now = now_ms();
        let appointment = store
            .create_appointment(NewAppointment {
                title: "暂停的约定".to_owned(),
                condition: "恢复我".to_owned(),
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
        store
            .set_status(&appointment.id, calendar::AppointmentStatus::Paused)
            .unwrap();
        (store, appointment)
    };
    host.dispatch(Call::CalendarAction(CalendarActionRequest {
        appointment_id: paused.id.clone(),
        action: CalendarActionKind::Resume,
    }))
    .await
    .expect("恢复");
    assert_eq!(
        store2.appointment(&paused.id).unwrap().unwrap().status,
        calendar::AppointmentStatus::Active
    );
    host.shutdown().await;
}
