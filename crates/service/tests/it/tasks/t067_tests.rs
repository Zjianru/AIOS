//! T067.2：ThinAgent 不等、等后台 30 分钟、到点/停止/换轮。

use super::*;

fn attempt_of(fixture: &Fixture, task: &Task) -> tasks::Attempt {
    tasks_store(fixture.data())
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("有 Attempt")
}

fn background_limit_pending(fixture: &Fixture, attempt_id: tasks::AttemptId) -> bool {
    tasks_store(fixture.data())
        .effect_pending(
            tasks::EffectKind::BackgroundLimit,
            &format!("bg-limit:{attempt_id}"),
        )
        .unwrap()
}

#[tokio::test]
async fn thin_agent_rejects_sleeping_commands_before_dispatch() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-wait",
            "bash",
            &serde_json::json!({
                "description": "不要等",
                "command": "touch started.txt; sleep 60"
            })
            .to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("收工"));
    let task = fixture.accept_args(false, write_task_args(), 1).await;
    wait_until("看不到工具结果", || {
        let attempt = attempt_of(&fixture, &task);
        let Some(conversation) = attempt.conversation else {
            return false;
        };
        let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
            return false;
        };
        session::SessionStore::open_read_only(fixture.data(), &id).is_ok_and(|session| {
            session.events().iter().any(|event| {
                matches!(&event.kind, session::EventKind::ToolResult { .. })
                    && serde_json::to_string(&event.kind)
                        .is_ok_and(|value| value.contains(agent::NO_WAIT))
            })
        })
    })
    .await;
    assert!(
        !fixture.cwd.path().join("started.txt").exists(),
        "被拒绝的命令不能真的起进程"
    );
}

#[tokio::test]
async fn background_limit_fires_kills_and_reports_waiting() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_background_limit_ms(250);
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-bg",
            "bash",
            &serde_json::json!({
                "description": "等后台",
                "command": "trap 'echo stopped > killed.txt' TERM; while true; do :; done",
                "run_in_background": true
            })
            .to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("转后台了"));
    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let attempt = attempt_of(&fixture, &task);
    wait_until("到点效果已排", || {
        background_limit_pending(&fixture, attempt.id)
    })
    .await;
    // 到点之前：不结束 Attempt、不报在等。
    let before = tasks_store(fixture.data()).task(&task.id).unwrap().unwrap();
    assert!(
        before
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none()),
        "后台还在跑时 Attempt 不能结束"
    );
    assert_eq!(
        fixture.host.task_runtime().unwrap().used_seats(),
        1,
        "后台还在跑时这一轮的席位还占着"
    );
    assert!(before.latest_report().is_none(), "不能先合成在等");
    wait_until("到点杀命令并合成在等", || {
        let task = tasks_store(fixture.data()).task(&task.id).unwrap().unwrap();
        task.latest_report()
            .is_some_and(|report| report.summary.contains("跑过 30 分钟被停"))
    })
    .await;
    assert!(
        fixture.cwd.path().join("killed.txt").exists(),
        "到点要杀掉后台命令"
    );
    assert!(
        !background_limit_pending(&fixture, attempt.id),
        "到点后效果收掉"
    );
}

#[tokio::test]
async fn a_new_turn_supersedes_the_old_background_limit() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_background_limit_ms(300);
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-bg",
            "bash",
            &serde_json::json!({
                "description": "等后台",
                "command": "trap 'echo stopped > killed.txt' TERM; while true; do :; done",
                "run_in_background": true
            })
            .to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("转后台了"));
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("接着说"));
    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let attempt = attempt_of(&fixture, &task);
    wait_until("旧到点已排", || {
        background_limit_pending(&fixture, attempt.id)
    })
    .await;
    // 用户说话开新一轮：旧到点不能再杀命令/报在等。
    fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "接着做".to_owned(),
            pastes: Vec::new(),
            attachments: Vec::new(),
        }))
        .await
        .expect("说一句");
    wait_until("新一轮开始", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !fixture.cwd.path().join("killed.txt").exists(),
        "旧到点不能杀掉新一轮的后台命令"
    );
    let task_now = tasks_store(fixture.data()).task(&task.id).unwrap().unwrap();
    assert!(
        task_now
            .latest_report()
            .is_none_or(|report| !report.summary.contains("跑过 30 分钟被停")),
        "旧到点不能合成在等：{:?}",
        task_now.latest_report()
    );
}

#[tokio::test]
async fn stopping_while_waiting_kills_commands_cancels_deadline_and_does_not_report_waiting() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_background_limit_ms(60_000);
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-bg",
            "bash",
            &serde_json::json!({
                "description": "等后台",
                "command": "trap 'echo stopped > killed.txt' TERM; while true; do :; done",
                "run_in_background": true
            })
            .to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("转后台了"));
    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let attempt = attempt_of(&fixture, &task);
    wait_until("到点已排", || {
        background_limit_pending(&fixture, attempt.id)
    })
    .await;

    let response = fixture
        .host
        .dispatch(Call::TaskInterrupt(TaskInterruptRequest {
            task_id: task.id.as_str().to_owned(),
        }))
        .await
        .expect("停这一轮");
    let Response::TaskInterrupt(response) = response else {
        panic!("不是停一轮响应");
    };
    assert!(response.interrupted, "{response:?}");
    wait_until("后台命令被杀", || {
        fixture.cwd.path().join("killed.txt").exists()
    })
    .await;
    assert!(
        !background_limit_pending(&fixture, attempt.id),
        "停止要撤掉到点"
    );
    assert_eq!(
        fixture.host.task_runtime().unwrap().used_seats(),
        0,
        "等后台时停止要放掉席位"
    );
    let task_now = tasks_store(fixture.data()).task(&task.id).unwrap().unwrap();
    assert!(
        task_now.latest_report().is_none(),
        "等后台时停止不合成在等：{:?}",
        task_now.latest_report()
    );
    assert!(
        task_now
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none()),
        "停一轮不结束 Attempt，任务照常用于接着做"
    );
    drop(
        fixture
            .host
            .dispatch(Call::TaskStop(TaskStopRequest {
                task_id: task.id.as_str().to_owned(),
                mode: StopMode::Now,
            }))
            .await,
    );
}

#[tokio::test]
async fn idle_pump_does_not_wake_and_resume_wakes_it_once() {
    let fixture = Fixture::new().await;
    let runtime = fixture.host.task_runtime().expect("runtime");
    runtime.set_pump_paused(true);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let baseline = runtime.pump_wake_count();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        runtime.pump_wake_count(),
        baseline,
        "没到点、没暂停变化时泵不该醒"
    );
    runtime.set_pump_paused(false);
    wait_until("恢复后泵醒一次", || {
        runtime.pump_wake_count() > baseline
    })
    .await;
}

#[tokio::test]
async fn exec_pause_alone_keeps_the_pump_asleep_and_resume_wakes_it() {
    let fixture = Fixture::new().await;
    let runtime = fixture.host.task_runtime().expect("runtime");
    runtime.set_exec_paused_for_test(true);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let baseline = runtime.pump_wake_count();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        runtime.pump_wake_count(),
        baseline,
        "exec_paused=true 时没有到点，泵只该等通知"
    );
    runtime.set_exec_paused_for_test(false);
    wait_until("恢复后泵立刻醒", || {
        runtime.pump_wake_count() > baseline
    })
    .await;
}
