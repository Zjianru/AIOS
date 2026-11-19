//! T068.1：报告后保留宿主、backgroundRunning、停止入口。

use super::*;

fn pgid_alive(path: &std::path::Path) -> bool {
    let pid: i32 = std::fs::read_to_string(path)
        .expect("shell.pid")
        .trim()
        .parse()
        .expect("pid");
    std::process::Command::new("kill")
        .args(["-0", &format!("-{pid}")])
        .status()
        .expect("kill -0")
        .success()
}

async fn task_read(fixture: &Fixture, task: &Task) -> service::TaskReadHeader {
    let response = fixture
        .host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(response) = response else {
        panic!("不是 task-read");
    };
    response.task
}

/// 让 ThinAgent 起一个后台命令再报回：命令写 shell.pid，循环到 done 或被杀。
async fn report_with_background(fixture: &Fixture, command: &str) -> (Task, std::path::PathBuf) {
    set_bash(&fixture.host, true).await;
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-bg",
            "bash",
            &serde_json::json!({
                "description": "后台命令",
                "command": command,
                "run_in_background": true
            })
            .to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-report",
            "task_report",
            r#"{"state":"ready","summary":"先报回，后台还在跑"}"#,
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("报了"));
    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let pid_file = fixture.cwd.path().join("shell.pid");
    wait_until("后台命令写 pid", || pid_file.exists()).await;
    wait_until("报告落库", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().cloned())
            .is_some_and(|report| report.state == ReportState::Ready)
    })
    .await;
    (task, pid_file)
}

#[tokio::test]
async fn report_keeps_the_host_with_background_running() {
    let fixture = Fixture::new_with_actions().await;
    let command = "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done";
    let (task, _pid) = report_with_background(&fixture, command).await;
    let header = task_read(&fixture, &task).await;
    assert!(!header.running, "报告后 running 要 false：{header:?}");
    assert!(
        header.background_running,
        "报告后后台命令还在跑：{header:?}"
    );
    assert_eq!(
        fixture.host.task_runtime().unwrap().used_seats(),
        0,
        "retained 不占席位"
    );
    let attempt = tasks_store(fixture.data())
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .unwrap();
    assert!(
        !tasks_store(fixture.data())
            .effect_pending(
                tasks::EffectKind::BackgroundLimit,
                &format!("bg-limit:{}", attempt.id)
            )
            .unwrap(),
        "报告后 retained 不排 BackgroundLimit"
    );
    // 自然结束：不叫醒模型、宿主移除、发 changed。
    let mut channel = fixture
        .host
        .subscribe_task_changed()
        .expect("根宿主 changed 频道");
    std::fs::write(fixture.cwd.path().join("done"), "").unwrap();
    for _ in 0..900 {
        if !task_read(&fixture, &task).await.background_running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let header = task_read(&fixture, &task).await;
    assert!(!header.running);
    assert!(!header.background_running);
    let received = tokio::time::timeout(Duration::from_secs(1), channel.recv())
        .await
        .expect("移除宿主要发 changed")
        .expect("changed 频道没关");
    assert_eq!(received, task.id.as_str());
    // 会话锁放掉：能只读打开会话（写锁已放开）。
    let conversation = attempt.conversation.unwrap();
    let id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("宿主移除后会话锁放掉", || {
        session::SessionStore::open(fixture.data(), &id).is_ok()
    })
    .await;
}

#[tokio::test]
async fn stop_entries_kill_retained_background_commands() {
    let fixture = Fixture::new_with_actions().await;
    let command = "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done";
    let (task, pid) = report_with_background(&fixture, command).await;
    let response = fixture
        .host
        .dispatch(Call::TaskInterrupt(TaskInterruptRequest {
            task_id: task.id.as_str().to_owned(),
        }))
        .await
        .expect("停下来");
    let Response::TaskInterrupt(response) = response else {
        panic!("不是 interrupt");
    };
    assert!(response.interrupted, "{response:?}");
    wait_until("进程组没了", || !pgid_alive(&pid)).await;
    let header = task_read(&fixture, &task).await;
    assert!(!header.running && !header.background_running);
}

#[tokio::test]
async fn immediate_stop_kills_retained_background_commands() {
    let fixture = Fixture::new_with_actions().await;
    let command = "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done";
    let (task, pid) = report_with_background(&fixture, command).await;
    let stopped = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stopped.stopped, "{stopped:?}");
    wait_until("进程组没了", || !pgid_alive(&pid)).await;
}

#[tokio::test]
async fn after_report_stop_retains_and_a_later_immediate_stop_kills() {
    let fixture = Fixture::new_with_actions().await;
    set_bash(&fixture.host, true).await;
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-bg",
            "bash",
            &serde_json::json!({
                "description": "后台命令",
                "command": "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done",
                "run_in_background": true
            })
            .to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("转后台了"));
    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let pid_file = fixture.cwd.path().join("shell.pid");
    wait_until("后台命令写 pid", || pid_file.exists()).await;
    // 收尾停：这一轮还没报、Attempt 未结束，不杀后台命令，转 retained。
    let stopped = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(stopped.stopped, "{stopped:?}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut header = task_read(&fixture, &task).await;
    for _ in 0..250 {
        if header.background_running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        header = task_read(&fixture, &task).await;
    }
    assert!(pgid_alive(&pid_file), "收尾停不杀后台命令");
    assert!(
        header.background_running,
        "收尾停后宿主 retained：{header:?}"
    );
    let stopped = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stopped.stopped, "{stopped:?}");
    wait_until("retained 再立即停杀掉进程组", || {
        !pgid_alive(&pid_file)
    })
    .await;
}
