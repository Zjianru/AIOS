//! 约定到点建触发任务、回合结束关、派出识别、看板过滤、四个工具拒。

use super::*;
use calendar::{Binding, CycleSpec, NewAppointment, Schedule};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn due_appointment(data: &Path, title: &str, cycle: Option<CycleSpec>) -> calendar::Appointment {
    let store = calendar::CalendarStore::open(data.join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let appointment = store
        .create_appointment(NewAppointment {
            title: title.to_owned(),
            condition: "一分钟后提醒我喝水".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now - 1_000,
            },
            binding: Binding::Kind,
            watch: None,
            cycle,
            duration_minutes: None,
        })
        .unwrap();
    store
        .set_next_trigger(&appointment.id, Some(now - 1))
        .unwrap();
    appointment
}

fn draft(data: &Path, title: &str) -> NewTask {
    NewTask {
        quote: None,
        quote_source: None,
        kind_note: None,
        project_id: None,
        cwd_source: None,
        title: title.to_owned(),
        requirement: "做点事".to_owned(),
        dod: "做完".to_owned(),
        cwd: data.to_path_buf(),
        conversation: ConversationId::new("kind-trigger-tests"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: Vec::new(),
    }
}

fn trigger_tasks(store: &Store) -> Vec<Task> {
    store
        .all_tasks()
        .unwrap()
        .into_iter()
        .filter(|task| {
            task.origin
                .as_ref()
                .map(|origin| origin.role == tasks::AppointmentRole::Trigger)
                == Some(true)
        })
        .collect()
}

async fn read_range(fixture: &Fixture) -> service::CalendarReadResponse {
    let response = fixture
        .host
        .dispatch(Call::CalendarRead(service::CalendarReadRequest {
            appointment_id: None,
            from: Some(now_ms() - 86_400_000),
            to: Some(now_ms() + 86_400_000),
        }))
        .await
        .expect("读日历");
    let Response::CalendarRead(read) = response else {
        panic!("日历读响应");
    };
    read
}

async fn wait(label: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..900 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等不到：{label}");
}

#[tokio::test]
async fn wake_creates_one_trigger_task_closes_on_turn_end_and_stays_off_the_board() {
    let fixture = Fixture::new().await;
    let address = due_appointment(fixture.data(), "喝水提醒", None);
    fixture.router.push_kind(crate::common::text_sse("办完了"));
    fixture.host.nudge_calendar();
    let tasks = tasks_store(fixture.data());
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    wait("送一条", || {
        store
            .runs(&address.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    wait("建触发任务", || trigger_tasks(&tasks).len() == 1).await;
    wait("写 round_end", || {
        trigger_tasks(&tasks)
            .first()
            .is_some_and(|task| task.round_end.is_some())
    })
    .await;
    let mut triggers = trigger_tasks(&tasks);
    let trigger = triggers.remove(0);
    assert_eq!(trigger.title, "喝水提醒");
    assert_eq!(trigger.requirement, "一分钟后提醒我喝水");
    assert!(trigger.runs.is_empty(), "触发任务不开 Attempt");
    assert!(trigger.reports.is_empty(), "触发任务不写报告");
    assert!(trigger.tools.is_empty(), "触发任务只读");
    assert_eq!(
        trigger.origin.as_ref().unwrap().role,
        tasks::AppointmentRole::Trigger
    );
    assert_eq!(trigger.origin.as_ref().unwrap().run_id, 1);
    let Response::Work(work) = fixture
        .host
        .dispatch(Call::Work(service::WorkRequest {}))
        .await
        .expect("工作快照")
    else {
        panic!("工作快照响应");
    };
    assert!(
        work.tasks.iter().all(|fact| fact.id != trigger.id.as_str()),
        "触发任务不能上看板"
    );
    assert_eq!(
        fixture
            .host
            .dispatch(Call::Work(service::WorkRequest {}))
            .await
            .map(|response| match response {
                Response::Work(work) => work
                    .tasks
                    .iter()
                    .filter(|fact| fact.id == trigger.id.as_str())
                    .count(),
                _ => 0,
            })
            .unwrap(),
        0
    );
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn cycle_due_also_creates_a_trigger_task() {
    let fixture = Fixture::new().await;
    let address = due_appointment(
        fixture.data(),
        "循环到点",
        Some(CycleSpec {
            task_id: "loop-task".to_owned(),
            max_rounds: Some(3),
        }),
    );
    fixture.router.push_kind(crate::common::text_sse("办了"));
    fixture.host.nudge_calendar();
    let tasks = tasks_store(fixture.data());
    wait("循环也建触发任务", || {
        trigger_tasks(&tasks).len() == 1
    })
    .await;
    let mut triggers = trigger_tasks(&tasks);
    let trigger = triggers.remove(0);
    assert_eq!(trigger.origin.as_ref().unwrap().appointment_id, address.id);
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn failed_delivery_does_not_precreate_a_trigger_task() {
    let fixture = Fixture::new().await;
    let address = due_appointment(fixture.data(), "没送成", None);
    fixture.host.crash_before_next_wake();
    fixture.host.nudge_calendar();
    let tasks = tasks_store(fixture.data());
    let _ = address;
    wait("送达失败一次", || {
        tasks_store(fixture.data())
            .effect_health(tasks::EffectKind::AppointmentWake)
            .is_ok_and(|health| health.attempts >= 1)
    })
    .await;
    assert!(
        trigger_tasks(&tasks).is_empty(),
        "没送成不建触发任务：{:?}",
        trigger_tasks(&tasks)
    );
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn trigger_tasks_refuse_continue_and_stop_with_fixed_words() {
    let fixture = Fixture::new().await;
    due_appointment(fixture.data(), "不能接着做", None);
    fixture.router.push_kind(crate::common::text_sse("办完"));
    fixture.host.nudge_calendar();
    let tasks = tasks_store(fixture.data());
    wait("触发任务", || trigger_tasks(&tasks).len() == 1).await;
    let mut triggers = trigger_tasks(&tasks);
    let trigger = triggers.remove(0);
    let error = fixture
        .host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: trigger.id.as_str().to_owned(),
            note: None,
        }))
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("这是约定到点的任务，不能接着做"), "{text}");
    let error = fixture
        .host
        .dispatch(Call::TaskStop(service::TaskStopRequest {
            task_id: trigger.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("这是约定到点的任务，不能接着做"), "{text}");
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn wake_turn_task_accept_marks_dispatched_but_user_turn_does_not() {
    let fixture = Fixture::new().await;
    due_appointment(fixture.data(), "派调查", None);
    let args = serde_json::json!({
        "title": "调查 elkjs",
        "quote": "一分钟后提醒我喝水",
        "requirement": "看新版",
        "dod": "有结论"
    });
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-wake-accept",
        "task_accept",
        &args.to_string(),
    ));
    fixture.router.push_kind(crate::common::text_sse("已派出"));
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture.host.nudge_calendar();
    let tasks = tasks_store(fixture.data());
    wait("派出任务", || {
        tasks
            .all_tasks()
            .unwrap()
            .iter()
            .any(|task| task.title == "调查 elkjs")
    })
    .await;
    let dispatched = tasks
        .all_tasks()
        .unwrap()
        .into_iter()
        .find(|task| task.title == "调查 elkjs")
        .unwrap();
    assert!(matches!(
        dispatched.versions[0].quote_source,
        Some(tasks::QuoteSource::Appointment { .. })
    ));
    assert_eq!(
        dispatched.origin.as_ref().map(|origin| origin.role),
        Some(tasks::AppointmentRole::Dispatched)
    );
    fixture.host.shutdown().await;
    let user_fixture = Fixture::new().await;
    let user_task = user_fixture.accept(true).await;
    assert_eq!(
        user_task.origin, None,
        "用户自己开的回合派的不带 dispatched"
    );
    user_fixture.host.shutdown().await;
}

#[tokio::test]
async fn wake_turn_graph_propose_marks_root_and_children_dispatched() {
    let fixture = Fixture::new().await;
    due_appointment(fixture.data(), "图派出", None);
    let graph = serde_json::json!({
        "title": "调查图",
        "quote": "一分钟后提醒我喝水",
        "requirement": "两步查",
        "dod": "有结论",
        "subtasks": [
            {"title": "先查", "requirement": "查一", "dod": "有"},
            {"title": "再查", "requirement": "查二", "dod": "有", "after": [0]}
        ]
    })
    .to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-wake-graph",
        "task_graph_propose",
        &graph,
    ));
    fixture
        .router
        .push_kind(crate::common::text_sse("图提好了"));
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture.host.nudge_calendar();
    let tasks = tasks_store(fixture.data());
    wait("建图", || {
        tasks
            .all_tasks()
            .unwrap()
            .iter()
            .any(|task| task.title == "调查图")
    })
    .await;
    let all = tasks.all_tasks().unwrap();
    let root = all.iter().find(|task| task.title == "调查图").unwrap();
    assert_eq!(
        root.origin.as_ref().map(|origin| origin.role),
        Some(tasks::AppointmentRole::Dispatched)
    );
    let children: Vec<_> = all
        .iter()
        .filter(|task| task.parent.as_ref() == Some(&root.id))
        .collect();
    assert_eq!(children.len(), 2);
    for child in children {
        assert_eq!(
            child.origin.as_ref().map(|origin| origin.role),
            Some(tasks::AppointmentRole::Dispatched),
            "图子任务也带 dispatched：{}",
            child.title
        );
    }
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn graph_cancel_makes_the_cycle_review_skip_without_a_trigger_task() {
    let fixture = Fixture::new().await;
    let tasks = tasks_store(fixture.data());
    let task = |title: &str| draft(fixture.data(), title);
    let root = tasks
        .create_graph(
            tasks::NewGraph {
                root: task("被取消的图"),
                subtasks: vec![
                    tasks::NewSubtask {
                        task: task("图里子任务"),
                        policy: tasks::TaskPolicy::default(),
                        after: Vec::new(),
                        milestone: false,
                        program_gate: Vec::new(),
                    },
                    tasks::NewSubtask {
                        task: task("图里第二个"),
                        policy: tasks::TaskPolicy::default(),
                        after: vec![0],
                        milestone: false,
                        program_gate: Vec::new(),
                    },
                ],
            },
            1_800_000_000_000,
        )
        .unwrap();
    tasks
        .cancel_graph(
            &root.id,
            tasks::CancellationReason::Dismissed,
            1_800_000_000_100,
        )
        .unwrap();
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let appointment = store
        .create_appointment(NewAppointment {
            title: "绑取消图".to_owned(),
            condition: "到点看绑的任务".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now - 1_000,
            },
            binding: Binding::Task(root.id.as_str().to_owned()),
            watch: None,
            cycle: None,
            duration_minutes: None,
        })
        .unwrap();
    store
        .set_next_trigger(&appointment.id, Some(now - 1))
        .unwrap();
    fixture.host.nudge_calendar();
    wait("记跳过", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Skipped)
    })
    .await;
    let run = store
        .runs(&appointment.id)
        .unwrap()
        .into_iter()
        .find(|run| run.result == calendar::RunResult::Skipped)
        .unwrap();
    assert!(
        run.reason.contains("跳过：绑定的任务已取消"),
        "{}",
        run.reason
    );
    assert_eq!(trigger_tasks(&tasks).len(), 0, "没送成不建触发任务");
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn two_hundred_trigger_tasks_do_not_slow_the_work_snapshot() {
    let fixture = Fixture::new().await;
    let tasks = tasks_store(fixture.data());
    for index in 0..200 {
        tasks
            .create_trigger_task_once(
                draft(fixture.data(), &format!("触发 {index}")),
                &format!("perf-trigger-{index}"),
                1_800_000_000_000 + index,
            )
            .unwrap();
        let created = tasks.all_tasks().unwrap();
        let id = created.last().unwrap().id.clone();
        tasks
            .set_origin(
                &id,
                tasks::AppointmentOrigin {
                    appointment_id: format!("perf-appointment-{index}"),
                    run_id: index,
                    role: tasks::AppointmentRole::Trigger,
                },
            )
            .unwrap();
    }
    let started = std::time::Instant::now();
    let response = fixture
        .host
        .dispatch(Call::Work(service::WorkRequest {}))
        .await
        .expect("工作快照");
    let elapsed = started.elapsed();
    let Response::Work(work) = response else {
        panic!("工作快照响应");
    };
    assert!(work.tasks.is_empty(), "触发任务不能上看板");
    assert!(work.wait.is_empty(), "触发任务不能进等你/现在");
    assert!(work.graphs.is_empty(), "触发任务不能进图");
    eprintln!(
        "200 条触发任务工作快照耗时：{} ms（200 个任务，快照 {} 项）",
        elapsed.as_millis(),
        work.tasks.len()
    );
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn startup_complement_writes_round_end_for_the_sent_turn() {
    let fixture = Fixture::new().await;
    let appointment = due_appointment(fixture.data(), "启动补", None);
    fixture.router.push_kind(crate::common::text_sse("办完"));
    fixture.host.nudge_calendar();
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    wait("先送一轮", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    wait("原来那条先关", || {
        trigger_tasks(&tasks_store(fixture.data()))
            .first()
            .is_some_and(|task| task.round_end.is_some())
    })
    .await;
    let run = store
        .runs(&appointment.id)
        .unwrap()
        .into_iter()
        .find(|run| run.result == calendar::RunResult::Sent)
        .unwrap();
    let sent_turn = u64::try_from(run.sent_turn.unwrap()).unwrap();
    let tasks = tasks_store(fixture.data());
    let mut complement = draft(fixture.data(), "补写结束轮");
    complement.conversation =
        tasks::ConversationId::new(fixture.host.info().session_id.expect("根会话"));
    let (task, _) = tasks
        .create_trigger_task_once(complement, "startup-complement", 1_800_000_000_000)
        .unwrap();
    tasks
        .set_origin(
            &task.id,
            tasks::AppointmentOrigin {
                appointment_id: appointment.id.clone(),
                run_id: run.id,
                role: tasks::AppointmentRole::Trigger,
            },
        )
        .unwrap();
    fixture.host.calendar_close_finished_turns();
    let task = tasks.task(&task.id).unwrap().unwrap();
    assert_eq!(task.round_end.unwrap().turn, sent_turn);
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn global_pause_merges_without_precreating_a_trigger_task_then_builds_one_on_send() {
    let fixture = Fixture::new().await;
    fixture
        .host
        .dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
            currency: None,
            paused: Some(true),
            seats: None,
            bash: None,
        }))
        .await
        .expect("进全局暂停");
    let appointment = due_appointment(fixture.data(), "暂停合并", None);
    fixture.host.nudge_calendar();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let tasks = tasks_store(fixture.data());
    assert!(
        trigger_tasks(&tasks).is_empty(),
        "排着/合并前不预建触发任务"
    );
    fixture.router.push_kind(crate::common::text_sse("补办"));
    fixture
        .host
        .dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
            currency: None,
            paused: Some(false),
            seats: None,
            bash: None,
        }))
        .await
        .expect("开闸");
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    wait("开闸后送一条并记 sent", || {
        trigger_tasks(&tasks).len() == 1
            && store
                .runs(&appointment.id)
                .unwrap()
                .iter()
                .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn calendar_read_returns_range_blocks_plans_colors_and_dense_counts() {
    use service::CalendarReadRequest;
    let fixture = Fixture::new().await;
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let appointment = store
        .create_appointment(NewAppointment {
            title: "区间事实".to_owned(),
            condition: "到点办".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now - 1_000,
            },
            binding: Binding::Kind,
            watch: None,
            cycle: None,
            duration_minutes: Some(60),
        })
        .unwrap();
    store
        .set_next_trigger(&appointment.id, Some(now - 1))
        .unwrap();
    fixture.router.push_kind(crate::common::text_sse("办完"));
    fixture.host.nudge_calendar();
    wait("区间块完成", || {
        trigger_tasks(&tasks_store(fixture.data()))
            .first()
            .is_some_and(|task| task.round_end.is_some())
    })
    .await;

    // 密规则每天计数：同一天两次跳过（没变两次）。
    let dense = store
        .create_appointment(NewAppointment {
            title: "密规则".to_owned(),
            condition: "每6小时查".to_owned(),
            schedule: Schedule {
                rule: "FREQ=HOURLY;INTERVAL=6".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now - 7_200_000,
            },
            binding: Binding::Kind,
            watch: None,
            cycle: None,
            duration_minutes: None,
        })
        .unwrap();
    for offset in [0_i64, 1] {
        let (run, _) = store
            .record_run(calendar::NewRun {
                appointment_id: dense.id.clone(),
                nominal_at: now - 3_000 + offset,
                manual: false,
                trigger: calendar::TriggerKind::Due,
                reason: "没变：和上次比内容一样".to_owned(),
                merged_count: 0,
                report_id: None,
            })
            .unwrap();
        store
            .finish_run(
                run.id,
                calendar::RunResult::Skipped,
                "没变：和上次比内容一样",
                None,
            )
            .unwrap();
    }

    let response = fixture
        .host
        .dispatch(Call::CalendarRead(CalendarReadRequest {
            appointment_id: None,
            from: Some(now - 86_400_000),
            to: Some(now + 86_400_000),
        }))
        .await
        .expect("区间日历读");
    let Response::CalendarRead(read) = response else {
        panic!("日历读响应");
    };
    assert_eq!(
        read.appointments
            .iter()
            .filter(|row| !row.id.starts_with("builtin:"))
            .count(),
        2
    );
    let block = read
        .ranges
        .iter()
        .find(|block| block.appointment_id == appointment.id)
        .expect("运行块");
    assert_eq!(block.stage.as_deref(), Some("closed"));
    assert!(block.task_id.is_none(), "Kind 自办块不点任务页");
    assert!(block.turn.is_some());
    assert!(block.end >= block.start);
    let plan = read
        .plans
        .iter()
        .find(|plan| plan.appointment_id == appointment.id)
        .expect("60 分钟计划块");
    assert_eq!(plan.end - plan.start, 3_600_000);
    assert!(
        read.colors
            .iter()
            .any(|color| color.appointment_id == appointment.id)
    );
    let day = read
        .dense
        .iter()
        .find(|day| day.appointment_id == dense.id)
        .expect("密规则当天计数");
    assert_eq!(day.runs, 2);
    assert_eq!(day.unchanged, 2);
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn ended_appointment_run_blocks_still_show_in_the_range() {
    let fixture = Fixture::new().await;
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let appointment = store
        .create_appointment(NewAppointment {
            title: "一次性结束".to_owned(),
            condition: "明早提醒".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY;COUNT=1".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now - 1_000,
            },
            binding: Binding::Kind,
            watch: None,
            cycle: None,
            duration_minutes: None,
        })
        .unwrap();
    store
        .set_next_trigger(&appointment.id, Some(now - 1))
        .unwrap();
    fixture.router.push_kind(crate::common::text_sse("办完"));
    fixture.host.nudge_calendar();
    wait("一次性结束", || {
        store.appointment(&appointment.id).unwrap().unwrap().status
            == calendar::AppointmentStatus::Ended
    })
    .await;
    wait("这一次也送出去", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    wait("这一轮也结束", || {
        trigger_tasks(&tasks_store(fixture.data()))
            .first()
            .is_some_and(|task| task.round_end.is_some())
    })
    .await;
    let response = fixture
        .host
        .dispatch(Call::CalendarRead(service::CalendarReadRequest {
            appointment_id: None,
            from: Some(now - 86_400_000),
            to: Some(now + 86_400_000),
        }))
        .await
        .expect("区间日历读");
    let Response::CalendarRead(read) = response else {
        panic!("日历读响应");
    };
    let block = read
        .ranges
        .iter()
        .find(|block| block.appointment_id == appointment.id)
        .expect("ended 一次性也要有块");
    assert_eq!(block.stage.as_deref(), Some("closed"));
    assert!(
        read.plans
            .iter()
            .all(|plan| plan.appointment_id != appointment.id),
        "ended 合约不出计划块"
    );
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn dispatched_block_follows_task_stage_until_it_closes() {
    let fixture = Fixture::new().await;
    let appointment = due_appointment(fixture.data(), "派任务块", None);
    fixture.router.push_kind(crate::common::text_sse("办完"));
    fixture.host.nudge_calendar();
    let tasks = tasks_store(fixture.data());
    wait("触发任务", || trigger_tasks(&tasks).len() == 1).await;
    let mut triggers = trigger_tasks(&tasks);
    let trigger = triggers.remove(0);
    let origin = trigger.origin.clone().unwrap();
    let (dispatched, _) = tasks
        .create_trigger_task_once(
            draft(fixture.data(), "派出的调查"),
            "dispatched-block",
            1_800_000_000_000,
        )
        .unwrap();
    tasks
        .set_origin(
            &dispatched.id,
            tasks::AppointmentOrigin {
                role: tasks::AppointmentRole::Dispatched,
                ..origin
            },
        )
        .unwrap();
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    wait("这次送出去", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    let open = read_range(&fixture).await;
    let block = open
        .ranges
        .iter()
        .find(|block| block.appointment_id == appointment.id)
        .expect("派出没关的块");
    assert_eq!(block.stage.as_deref(), Some("pending"));
    assert!(block.end >= now_ms() - 60_000);
    // 关掉派出任务：ready 报告 + 用户接受。
    let attempt = tasks
        .begin_attempt(&dispatched.id, 1_800_000_000_100)
        .unwrap();
    let report = tasks
        .record_report(
            &dispatched.id,
            attempt,
            tasks::NewReport {
                state: tasks::ReportState::Ready,
                origin: tasks::ReportOrigin::Agent,
                summary: "查完了".to_owned(),
                question: None,
            },
            1_800_000_000_200,
        )
        .unwrap();
    tasks
        .accept(&dispatched.id, &report, 1_800_000_000_300)
        .unwrap();
    let closed = read_range(&fixture).await;
    let block = closed
        .ranges
        .iter()
        .find(|block| block.appointment_id == appointment.id)
        .expect("关闭后的块");
    assert_eq!(block.stage.as_deref(), Some("closed"));
    drop(store);
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn duration_uses_history_average_and_until_limits_plans() {
    let fixture = Fixture::new().await;
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    let tasks = tasks_store(fixture.data());
    let now = now_ms();
    // 两条历史运行：各 10 / 20 分钟，平均 15 分钟。
    let history = store
        .create_appointment(NewAppointment {
            title: "历史平均".to_owned(),
            condition: "到点办".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now - 86_400_000,
            },
            binding: Binding::Kind,
            watch: None,
            cycle: None,
            duration_minutes: None,
        })
        .unwrap();
    for (index, minutes) in [10_i64, 20].into_iter().enumerate() {
        let (run, _) = store
            .record_run(calendar::NewRun {
                appointment_id: history.id.clone(),
                nominal_at: now - 3_600_000 + index as i64,
                manual: false,
                trigger: calendar::TriggerKind::Due,
                reason: String::new(),
                merged_count: 0,
                report_id: None,
            })
            .unwrap();
        store
            .finish_run(run.id, calendar::RunResult::Sent, "", Some(1))
            .unwrap();
        let (task, _) = tasks
            .create_trigger_task_once(
                draft(fixture.data(), &format!("历史 {index}")),
                &format!("history-{index}"),
                now - 3_600_000 + index as i64,
            )
            .unwrap();
        tasks
            .set_origin(
                &task.id,
                tasks::AppointmentOrigin {
                    appointment_id: history.id.clone(),
                    run_id: run.id,
                    role: tasks::AppointmentRole::Trigger,
                },
            )
            .unwrap();
        tasks
            .set_round_end(
                &task.id,
                tasks::RoundEnd {
                    turn: 1,
                    at: now - 3_600_000 + index as i64 + minutes * 60_000,
                },
            )
            .unwrap();
    }
    // 带 UNTIL 的规则：只出到指定时间。
    let until = now + 2 * 86_400_000;
    let limited = store
        .create_appointment(NewAppointment {
            title: "出差汇总".to_owned(),
            condition: "每晚汇总".to_owned(),
            schedule: Schedule {
                rule: format!(
                    "FREQ=DAILY;UNTIL={}",
                    chrono::DateTime::from_timestamp_millis(until)
                        .unwrap()
                        .format("%Y%m%dT%H%M%SZ")
                ),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now,
            },
            binding: Binding::Kind,
            watch: None,
            cycle: None,
            duration_minutes: None,
        })
        .unwrap();
    let response = fixture
        .host
        .dispatch(Call::CalendarRead(service::CalendarReadRequest {
            appointment_id: None,
            from: Some(now - 86_400_000),
            to: Some(now + 10 * 86_400_000),
        }))
        .await
        .expect("读日历");
    let Response::CalendarRead(read) = response else {
        panic!("日历读响应");
    };
    let plan = read
        .plans
        .iter()
        .find(|plan| plan.appointment_id == history.id)
        .expect("历史平均计划块");
    assert_eq!(plan.end - plan.start, 15 * 60_000);
    assert!(
        read.plans
            .iter()
            .all(|plan| plan.appointment_id != limited.id || plan.start <= until)
    );
    drop(store);
    fixture.host.shutdown().await;
}

#[tokio::test]
async fn cycle_due_attempt_end_sets_the_block_end() {
    let fixture = Fixture::new().await;
    let tasks = tasks_store(fixture.data());
    let bound = tasks
        .create_task(draft(fixture.data(), "循环绑定任务"), 1_800_000_000_000)
        .unwrap();
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    let now = now_ms();
    let appointment = store
        .create_appointment(NewAppointment {
            title: "循环块".to_owned(),
            condition: "接着做".to_owned(),
            schedule: Schedule {
                rule: "FREQ=DAILY".to_owned(),
                timezone: "Asia/Shanghai".to_owned(),
                start_at: now,
            },
            binding: Binding::Task(bound.id.as_str().to_owned()),
            watch: None,
            cycle: Some(CycleSpec {
                task_id: bound.id.as_str().to_owned(),
                max_rounds: None,
            }),
            duration_minutes: None,
        })
        .unwrap();
    let (run, _) = store
        .record_run(calendar::NewRun {
            appointment_id: appointment.id.clone(),
            nominal_at: now - 1_000,
            manual: false,
            trigger: calendar::TriggerKind::Due,
            reason: String::new(),
            merged_count: 0,
            report_id: None,
        })
        .unwrap();
    store
        .finish_run(run.id, calendar::RunResult::Sent, "", Some(1))
        .unwrap();
    let attempt = tasks
        .begin_attempt_with(
            &bound.id,
            now - 2_000,
            tasks::AttemptTrigger::KindContinue {
                cause: tasks::ContinueCause::CycleDue {
                    appointment_id: appointment.id.clone(),
                    run_id: run.id,
                },
                note: None,
            },
            Vec::new(),
        )
        .unwrap();
    tasks
        .end_attempt(&bound.id, attempt, now + 5_000, tasks::AttemptEnd::Reported)
        .unwrap();
    let response = fixture
        .host
        .dispatch(Call::CalendarRead(service::CalendarReadRequest {
            appointment_id: None,
            from: Some(now - 86_400_000),
            to: Some(now + 86_400_000),
        }))
        .await
        .expect("读日历");
    let Response::CalendarRead(read) = response else {
        panic!("日历读响应");
    };
    let block = read
        .ranges
        .iter()
        .find(|block| block.appointment_id == appointment.id)
        .expect("循环块");
    assert_eq!(block.end, now + 5_000);
    assert_eq!(block.stage.as_deref(), Some("closed"));
    fixture.host.shutdown().await;
}

fn dispatched_origin(appointment: &calendar::Appointment, run_id: i64) -> tasks::AppointmentOrigin {
    tasks::AppointmentOrigin {
        appointment_id: appointment.id.clone(),
        run_id,
        role: tasks::AppointmentRole::Dispatched,
    }
}

async fn sent_run(fixture: &Fixture, appointment: &calendar::Appointment) -> i64 {
    let store = calendar::CalendarStore::open(fixture.data().join("calendar.sqlite")).unwrap();
    wait("约定送出一条", || {
        store
            .runs(&appointment.id)
            .unwrap()
            .iter()
            .any(|run| run.result == calendar::RunResult::Sent)
    })
    .await;
    store
        .runs(&appointment.id)
        .unwrap()
        .into_iter()
        .find(|run| run.result == calendar::RunResult::Sent)
        .unwrap()
        .id
}

/// 单任务：blocked 报告和 stopped Attempt 都走 Store::stage 的「缺条件」，不再冒充在做/等你核。
#[tokio::test]
async fn range_block_reads_task_stage_for_blocked_and_stopped_single_tasks() {
    let fixture = Fixture::new().await;
    let appointment = due_appointment(fixture.data(), "单任务派出去", None);
    fixture
        .router
        .push_kind(crate::common::text_sse("派出去了"));
    fixture.host.nudge_calendar();
    let run_id = sent_run(&fixture, &appointment).await;
    let tasks = tasks_store(fixture.data());
    let now = now_ms();

    let blocked = tasks
        .create_task(draft(fixture.data(), "blocked 单任务"), now)
        .unwrap();
    tasks
        .set_origin(&blocked.id, dispatched_origin(&appointment, run_id))
        .unwrap();
    let blocked_attempt = tasks.begin_attempt(&blocked.id, now + 1).unwrap();
    tasks
        .record_report(
            &blocked.id,
            blocked_attempt,
            tasks::NewReport {
                state: tasks::ReportState::Blocked,
                origin: tasks::ReportOrigin::Agent,
                summary: "缺一个目录".to_owned(),
                question: None,
            },
            now + 2,
        )
        .unwrap();

    let stopped = tasks
        .create_task(draft(fixture.data(), "stopped 单任务"), now)
        .unwrap();
    tasks
        .set_origin(&stopped.id, dispatched_origin(&appointment, run_id))
        .unwrap();
    let stopped_attempt = tasks.begin_attempt(&stopped.id, now + 3).unwrap();
    tasks
        .stop_attempt(&stopped.id, stopped_attempt, now + 4)
        .unwrap();

    let read = read_range(&fixture).await;
    let rows: Vec<_> = read
        .ranges
        .iter()
        .filter(|block| block.appointment_id == appointment.id)
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].stage.as_deref(),
        Some("needs_conditions"),
        "{:?}",
        rows[0]
    );
    // 两个任务同一阶段时取表序最靠前的一个；单任务直接就是它。
    assert!(rows[0].end >= now, "{:?}", rows[0]);
}

/// 整张图派出：子任务全接受后块是 closed，结束时间取接受事实且不再跟着 now 长。
#[tokio::test]
async fn range_block_for_dispatched_graph_closes_and_freezes_end() {
    let fixture = Fixture::new().await;
    let appointment = due_appointment(fixture.data(), "图派出去", None);
    fixture
        .router
        .push_kind(crate::common::text_sse("图派出去了"));
    fixture.host.nudge_calendar();
    let run_id = sent_run(&fixture, &appointment).await;
    let tasks = tasks_store(fixture.data());
    let now = now_ms();
    let root = tasks
        .create_graph(
            tasks::NewGraph {
                root: draft(fixture.data(), "派出图"),
                subtasks: vec![
                    tasks::NewSubtask {
                        task: draft(fixture.data(), "图里先查"),
                        policy: tasks::TaskPolicy::default(),
                        after: Vec::new(),
                        milestone: false,
                        program_gate: Vec::new(),
                    },
                    tasks::NewSubtask {
                        task: draft(fixture.data(), "图里再查"),
                        policy: tasks::TaskPolicy::default(),
                        after: vec![0],
                        milestone: false,
                        program_gate: Vec::new(),
                    },
                ],
            },
            now,
        )
        .unwrap();
    tasks.confirm_graph(&root.id, now + 1).unwrap();
    tasks
        .set_origin(&root.id, dispatched_origin(&appointment, run_id))
        .unwrap();
    let children: Vec<_> = tasks
        .all_tasks()
        .unwrap()
        .into_iter()
        .filter(|task| task.parent.as_ref() == Some(&root.id))
        .collect();
    assert_eq!(children.len(), 2);
    let mut closed_at = 0;
    for (index, child) in children.iter().enumerate() {
        tasks
            .set_origin(&child.id, dispatched_origin(&appointment, run_id))
            .unwrap();
        let attempt = tasks
            .begin_attempt(&child.id, now + 2 + index as i64)
            .unwrap();
        let report = tasks
            .record_report(
                &child.id,
                attempt,
                tasks::NewReport {
                    state: tasks::ReportState::Ready,
                    origin: tasks::ReportOrigin::Agent,
                    summary: format!("{} 做完了", child.title),
                    question: None,
                },
                now + 3 + index as i64,
            )
            .unwrap();
        tasks
            .end_attempt(
                &child.id,
                attempt,
                now + 4 + index as i64,
                tasks::AttemptEnd::Reported,
            )
            .unwrap();
        let acceptance = tasks
            .accept(&child.id, &report, now + 5 + index as i64)
            .unwrap();
        closed_at = closed_at.max(acceptance.at);
    }
    let first = read_range(&fixture).await;
    let block = first
        .ranges
        .iter()
        .find(|block| block.appointment_id == appointment.id)
        .unwrap();
    assert_eq!(block.stage.as_deref(), Some("closed"), "{block:?}");
    assert_eq!(block.end, closed_at, "结束取关闭事实");
    let second = read_range(&fixture).await;
    let block = second
        .ranges
        .iter()
        .find(|block| block.appointment_id == appointment.id)
        .unwrap();
    assert_eq!(block.end, closed_at, "接受后结束时间固定");
}
