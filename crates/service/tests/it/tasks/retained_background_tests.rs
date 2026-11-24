//! 报告后保留宿主、backgroundRunning、停止入口。

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
    // 等这一轮真正结束（后台命令还在时会保留 Attempt/席位），收尾停落在空闲会话上，结算确定。
    for _ in 0..500 {
        if !task_read(&fixture, &task).await.running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

#[tokio::test]
async fn handback_also_retains_the_host_and_stays_stoppable() {
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
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-handback",
            "task_handback",
            r#"{"reason":"做不下去了，先交回"}"#,
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("交回"));
    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let pid_file = fixture.cwd.path().join("shell.pid");
    wait_until("后台命令写 pid", || pid_file.exists()).await;
    wait_until("blocked 报告落库", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().cloned())
            .is_some_and(|report| report.state == ReportState::Blocked)
    })
    .await;
    let header = task_read(&fixture, &task).await;
    assert!(header.background_running, "交回后 retained：{header:?}");
    let stopped = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stopped.stopped, "{stopped:?}");
    wait_until("交回 retained 也能停掉进程组", || {
        !pgid_alive(&pid_file)
    })
    .await;
}

#[tokio::test]
async fn shutdown_stops_all_running_and_retained_background_commands() {
    let fixture = Fixture::new_with_actions().await;
    set_bash(&fixture.host, true).await;
    let runtime = fixture.host.task_runtime().expect("runtime");
    runtime.set_pump_paused(true);
    let store = tasks_store(fixture.data());
    let mut tasks = Vec::new();
    for (index, title) in ["任务甲", "任务乙"].into_iter().enumerate() {
        let args = serde_json::json!({
            "title": title,
            "quote": "派一个任务",
            "requirement": "等后台命令",
            "dod": "能停掉",
            "write_scope": fixture.cwd.path().join("target.py").to_string_lossy()
        })
        .to_string();
        kind_turn(
            &fixture.host,
            &fixture.router,
            fixture.data(),
            &format!("kind-{index}"),
            &format!(
                "派一个任务 {}",
                fixture.cwd.path().join("target.py").display()
            ),
            index + 1,
            vec![
                crate::common::tool_use_sse(
                    0,
                    &format!("call-accept-{index}"),
                    "task_accept",
                    &args,
                ),
                crate::common::text_sse("已派出"),
            ],
        )
        .await;
        let task = store
            .task_ids()
            .unwrap()
            .into_iter()
            .filter_map(|id| store.task(&id).ok().flatten())
            .find(|task| task.title == title)
            .expect("刚受理的任务");
        tasks.push(task);
    }
    // 任务都受理完再给每个 task id 排 ThinAgent 回复，放泵开跑。
    for (index, (pid_name, task)) in [("pid-a", &tasks[0]), ("pid-b", &tasks[1])]
        .into_iter()
        .enumerate()
    {
        fixture.router.push_thin_for(
            task.id.as_str(),
            crate::common::tool_use_sse(
                1,
                &format!("call-bg-{pid_name}"),
                "bash",
                &serde_json::json!({
                    "description": "长命令",
                    "command": format!("echo $$ > {pid_name}; trap 'exit 0' TERM; while [ ! -f {pid_name}.done ]; do :; done"),
                    "run_in_background": true
                })
                .to_string(),
            ),
        );
        fixture
            .router
            .push_thin_for(task.id.as_str(), crate::common::text_sse("转后台了"));
        let _ = index;
    }
    runtime.set_pump_paused(false);
    tokio::time::sleep(Duration::from_millis(500)).await;
    for task in &tasks {
        eprintln!(
            "DEBUG task {} attempt={:?}",
            task.id,
            store
                .task(&task.id)
                .unwrap()
                .unwrap()
                .latest_attempt()
                .map(|a| a.ended_at)
        );
    }
    eprintln!("DEBUG bodies={}", fixture.router.bodies(true).len());
    let pid_a = fixture.cwd.path().join("pid-a");
    let pid_b = fixture.cwd.path().join("pid-b");
    wait_until("甲 pid 文件", || pid_a.exists()).await;
    wait_until("乙 pid 文件", || pid_b.exists()).await;
    let pid_a: i32 = std::fs::read_to_string(&pid_a)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let pid_b: i32 = std::fs::read_to_string(&pid_b)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(group_alive_at(pid_a) && group_alive_at(pid_b));
    // 生产退出入口：RootHost::shutdown 先停任务命令，再关 Kind。
    fixture.host.shutdown().await;
    wait_until("退出后两个进程组都没了", || {
        !group_alive_at(pid_a) && !group_alive_at(pid_b)
    })
    .await;
    // 不再叫醒：两个会话都只有 1 轮。
    for task in &tasks {
        let attempt = store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .cloned()
            .unwrap();
        let conversation = attempt.conversation.unwrap();
        let id = session::SessionId::parse(conversation.as_str()).unwrap();
        let session = session::SessionStore::open_read_only(fixture.data(), &id).unwrap();
        assert_eq!(
            session
                .events()
                .iter()
                .filter(|event| matches!(&event.kind, session::EventKind::TurnStart { .. }))
                .count(),
            1,
            "退出后不能再开新一轮"
        );
    }
}

fn group_alive_at(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &format!("-{pid}")])
        .status()
        .expect("kill -0")
        .success()
}

#[tokio::test]
async fn saying_while_retained_stops_commands_then_reopens_a_new_attempt() {
    let fixture = Fixture::new_with_actions().await;
    let command = "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done";
    let (task, pid) = report_with_background(&fixture, command).await;
    let old_attempt = tasks_store(fixture.data())
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .unwrap()
        .id;
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("接着做"));
    let response = fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "接着改".to_owned(),
            pastes: Vec::new(),
            attachments: Vec::new(),
        }))
        .await
        .expect("说一句");
    let Response::TaskSay(said) = response else {
        panic!("不是 task-say");
    };
    assert!(!said.input_id.is_empty(), "{said:?}");
    wait_until("保留命令先被停掉", || !pgid_alive(&pid)).await;
    wait_until("新 Attempt 开成", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some_and(|attempt| attempt.id != old_attempt)
    })
    .await;
    let header = task_read(&fixture, &task).await;
    assert!(!header.background_running, "{header:?}");
}

#[tokio::test]
async fn kind_continue_while_retained_stops_commands_then_reopens() {
    let fixture = Fixture::new_with_actions().await;
    set_bash(&fixture.host, true).await;
    let title = "改一下判重";
    fixture.router.push_thin_for_title(
        title,
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
    fixture.router.push_thin_for_title(
        title,
        crate::common::tool_use_sse(
            2,
            "call-report",
            "task_report",
            r#"{"state":"failed","summary":"先报失败，后台还在跑"}"#,
        ),
    );
    fixture
        .router
        .push_thin_for_title(title, crate::common::text_sse("报了"));
    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let pid_file = fixture.cwd.path().join("shell.pid");
    wait_until("后台命令写 pid", || pid_file.exists()).await;
    wait_until("failed 报告落库", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().cloned())
            .is_some_and(|report| report.state == ReportState::Failed)
    })
    .await;
    let old_attempt = tasks_store(fixture.data())
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .unwrap()
        .id;
    fixture
        .router
        .push_thin_for_title(title, crate::common::text_sse("接着做"));
    let response = fixture
        .host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: task.id.as_str().to_owned(),
            note: None,
        }))
        .await
        .expect("接着做");
    let Response::TaskContinue(response) = response else {
        panic!("不是 task-continue");
    };
    assert!(response.attempt_id > 0, "{response:?}");
    wait_until("保留命令先被停掉", || !pgid_alive(&pid_file)).await;
    wait_until("Kind 接续开新 Attempt", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some_and(|attempt| attempt.id != old_attempt)
    })
    .await;
}

#[tokio::test]
async fn changing_requirement_while_retained_stops_commands_then_reopens() {
    let fixture = Fixture::new_with_actions().await;
    let command = "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done";
    let (task, pid) = report_with_background(&fixture, command).await;
    let old_attempt = tasks_store(fixture.data())
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .unwrap()
        .id;
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("按新要求做"));
    let summary = fixture
        .host
        .task_runtime()
        .unwrap()
        .update_requirement_for_test(
            task.id.as_str(),
            "改成按标题判重".to_owned(),
            Some("同一标题只留一条".to_owned()),
            None,
        )
        .await
        .expect("改要求");
    assert!(summary.contains("第 2 版"), "{summary}");
    wait_until("改要求前先停保留命令", || !pgid_alive(&pid)).await;
    wait_until("改要求后新 Attempt 开成", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some_and(|attempt| attempt.id != old_attempt)
    })
    .await;
}

#[tokio::test]
async fn changing_write_scope_while_retained_stops_commands_then_reopens() {
    let fixture = Fixture::new_with_actions().await;
    let command = "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done";
    let (task, pid) = report_with_background(&fixture, command).await;
    let old_attempt = tasks_store(fixture.data())
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .unwrap()
        .id;
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("按新范围做"));
    let summary = fixture
        .host
        .task_runtime()
        .unwrap()
        .update_scope_for_test(task.id.as_str(), "other.py".to_owned())
        .await
        .expect("改写入范围");
    assert!(summary.contains("第 2 版"), "{summary}");
    wait_until("改写集前先停保留命令", || !pgid_alive(&pid)).await;
    wait_until("改写集后新 Attempt 开成", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some_and(|attempt| attempt.id != old_attempt)
    })
    .await;
}

#[tokio::test]
async fn queued_message_while_retained_stops_commands_then_reopens() {
    let fixture = Fixture::new_with_actions().await;
    let command = "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done";
    let (task, pid) = report_with_background(&fixture, command).await;
    let old_attempt = tasks_store(fixture.data())
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .unwrap()
        .id;
    let store = tasks_store(fixture.data());
    store
        .say_input(
            &task.id,
            tasks::NewInput {
                text: "接着说一句".to_owned(),
                pastes: Vec::new(),
                attachments: Vec::new(),
            },
            1_700_000_100_000,
        )
        .expect("记输入");
    fixture.host.task_runtime().unwrap().wake_for_test();
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("收到了"));
    wait_until("消息送到时先停保留命令", || !pgid_alive(&pid)).await;
    wait_until("消息送到后新 Attempt 开成", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some_and(|attempt| attempt.id != old_attempt)
    })
    .await;
}

#[tokio::test]
async fn kind_task_stop_tool_kills_retained_background_commands() {
    use service::TaskStopTool;
    use tools::{Tool, ToolCall, ToolContext, ToolSessionState};

    let fixture = Fixture::new_with_actions().await;
    let command = "echo $$ > shell.pid; trap 'exit 0' TERM; while [ ! -f done ]; do :; done";
    let (task, pid) = report_with_background(&fixture, command).await;
    let context = ToolContext::with_session(
        fixture.cwd.path(),
        Arc::new(ToolSessionState::new(fixture.cwd.path(), "kind")),
    );
    let tool = TaskStopTool::new(Arc::clone(fixture.host.task_runtime().unwrap()));
    let result = tool
        .execute(
            ToolCall {
                id: "call-stop-retained".to_owned(),
                name: "task_stop".to_owned(),
                arguments: serde_json::json!({
                    "task_id": task.id.as_str(),
                    "mode": "now"
                })
                .to_string(),
            },
            &context,
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("已停下保留的后台命令"),
        "{}",
        result.content
    );
    wait_until("Kind task_stop 杀 retained 进程组", || {
        !pgid_alive(&pid)
    })
    .await;
}
