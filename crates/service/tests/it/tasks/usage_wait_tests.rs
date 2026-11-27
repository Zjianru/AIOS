//! 长 429 进用量等待，到点由泵开新 Attempt（服务端真任务运行时、假模型）。

use std::time::Duration;

use super::*;
use service::{StopMode, TaskContinueRequest, TaskSayRequest, TaskStopRequest};
use tasks::{AttemptTrigger, ReportState};

async fn wait_until(label: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..250 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等不到：{label}");
}

async fn waiting_fixture() -> (Fixture, Task, tasks::Store) {
    let fixture = Fixture::new().await;
    fixture.router.push_thin_error(429, Some("11"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("任务进入 Deferred", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.end == Some(AttemptEnd::Deferred))
    })
    .await;
    (fixture, task, store)
}

async fn stop_waiting(fixture: &Fixture, task: &Task) {
    let response = fixture
        .host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停任务");
    let Response::TaskStop(response) = response else {
        panic!("不是 TaskStop")
    };
    assert!(response.stopped);
}

async fn continue_waiting(fixture: &Fixture, task: &Task) {
    fixture
        .router
        .push_thin_for_title(&task.title, crate::common::text_sse("接着做"));
    fixture
        .host
        .dispatch(Call::TaskContinue(TaskContinueRequest {
            task_id: task.id.as_str().to_owned(),
            note: None,
        }))
        .await
        .expect("接着做");
}

#[tokio::test]
async fn long_429_enters_usage_wait_and_resumes_at_due() {
    let fixture = Fixture::new().await;
    // 第一个 ThinAgent 请求就吃 11 秒后的 429（超过 的 10 秒门限）。
    fixture.router.push_thin_error(429, Some("11"));
    let task = fixture.accept(false).await;
    let kind_requests = fixture.router.bodies(false).len();
    let store = tasks_store(fixture.data());

    wait_until("任务进入 Deferred", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.end == Some(AttemptEnd::Deferred))
    })
    .await;

    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.reports.len(), 1, "保留一条 waiting 报告");
    assert_eq!(stored.reports[0].state, ReportState::Waiting);
    assert_eq!(
        fixture.router.bodies(false).len(),
        kind_requests,
        "没有 relay，Kind 不应为等待报告多跑一轮"
    );
    assert_eq!(
        fixture.router.bodies(false).len(),
        kind_requests,
        "用量等待不能多跑一轮 Kind"
    );
    assert_eq!(
        fixture.host.task_runtime().unwrap().used_seats(),
        0,
        "等待时释放席位"
    );
    let read = fixture
        .host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("读任务");
    let Response::TaskRead(read) = read else {
        panic!("不是 TaskRead")
    };
    let wait = read.task.usage_wait.expect("TaskRead 要带 usageWait");
    assert!(wait.resume_at > 0);
    assert_eq!(wait.connection, "deepseek");

    // 把 due 拨到现在，模拟测试时钟到点；第二次模型回复给一段普通文字。
    fixture
        .router
        .push_thin_for_title(&stored.title, crate::common::text_sse("继续"));
    let runtime = fixture.host.task_runtime().unwrap();
    runtime
        .reschedule_usage_wait_for_test(task.id.as_str())
        .expect("拨到点");
    wait_until("泵开新 Attempt", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.runs.iter().flat_map(|run| run.attempts.iter()).count() > 1)
    })
    .await;
    let stored = store.task(&task.id).unwrap().unwrap();
    let latest = stored.latest_attempt().expect("latest attempt");
    assert_eq!(latest.trigger, AttemptTrigger::UsageResume);
    let conversation = latest.conversation.clone().expect("续跑会话");
    wait_until("续跑说明写进会话", || {
        let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
            return false;
        };
        let Ok(store) = session::SessionStore::open_read_only(fixture.data(), &id) else {
            return false;
        };
        store.events().iter().any(|event| {
            matches!(
                &event.kind,
                session::EventKind::UserMessage { message, .. }
                    if serde_json::to_string(message)
                        .unwrap_or_default()
                        .contains("上一轮因为临时原因中断，从断处继续。")
            )
        })
    })
    .await;
    assert!(
        !stored
            .reports
            .iter()
            .any(|report| report.state == ReportState::Ready),
        "本测试只看续跑 Attempt 起来，不依赖它已经交结果"
    );
}
#[tokio::test]
async fn stopping_a_waiting_task_appends_stops_and_keeps_history_then_continue_reopens() {
    let (fixture, task, store) = waiting_fixture().await;
    let before = store.task(&task.id).unwrap().unwrap();
    let ended_at = before.latest_attempt().unwrap().ended_at;
    let conversation = before.latest_attempt().unwrap().conversation.clone();
    let reports = before.reports.clone();
    let kind_requests = fixture.router.bodies(false).len();

    stop_waiting(&fixture, &task).await;
    assert_eq!(
        fixture.router.bodies(false).len(),
        kind_requests,
        "停止等待任务不叫醒 Kind"
    );
    let stopped = store.task(&task.id).unwrap().unwrap();
    let latest = stopped.latest_attempt().unwrap();
    assert_eq!(latest.end, Some(AttemptEnd::Deferred));
    assert_eq!(latest.ended_at, ended_at, "停止不改结束时间");
    assert_eq!(latest.conversation, conversation, "停止不改会话");
    assert_eq!(latest.stops.len(), 1, "只追加 stops");
    assert_eq!(stopped.reports, reports, "停止不改报告");
    assert!(stopped.is_stopped());
    let key = format!("usage-resume:{}:{}", task.id, latest.id.0);
    assert!(
        store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_none(),
        "停止要删到点效果"
    );
    let read = fixture
        .host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("读任务");
    let Response::TaskRead(read) = read else {
        panic!("不是 TaskRead")
    };
    assert!(read.task.stopped, "页面用同一个停止判断");

    continue_waiting(&fixture, &task).await;
    wait_until("停止后接着做开了新 Attempt", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.runs.iter().flat_map(|run| run.attempts.iter()).count() > 1)
    })
    .await;
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.reports.len(), reports.len(), "不重复写报告");
    assert!(stored.latest_attempt().unwrap().stops.is_empty());
    assert!(matches!(
        stored.latest_attempt().unwrap().trigger,
        AttemptTrigger::KindContinue { .. }
    ));
}

#[tokio::test]
async fn early_continue_deletes_the_usage_effect_without_a_stop_marker() {
    let (fixture, task, store) = waiting_fixture().await;
    let before = store.task(&task.id).unwrap().unwrap();
    let kind_requests = fixture.router.bodies(false).len();
    let key = format!(
        "usage-resume:{}:{}",
        task.id,
        before.latest_attempt().unwrap().id.0
    );
    continue_waiting(&fixture, &task).await;
    wait_until("提前接着做开了新 Attempt", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.runs.iter().flat_map(|run| run.attempts.iter()).count() > 1)
    })
    .await;
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        fixture.router.bodies(false).len(),
        kind_requests,
        "提前继续不叫醒 Kind"
    );
    assert!(after.latest_attempt().unwrap().stops.is_empty());
    assert_eq!(after.reports.len(), before.reports.len());
    assert!(
        store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_none(),
        "提前继续要删掉旧到点效果，避免勾出第二个 Attempt"
    );
}
#[tokio::test]
async fn waiting_survives_one_restart_and_resumes_once() {
    let fixture = Fixture::new().await;
    fixture.router.push_thin_error(429, Some("11"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("任务进入 Deferred", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.end == Some(AttemptEnd::Deferred))
    })
    .await;
    let key = format!(
        "usage-resume:{}:{}",
        task.id,
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .id
            .0
    );
    assert!(
        store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_some()
    );

    let Fixture {
        _server,
        data,
        cwd,
        router,
        host,
    } = fixture;
    host.task_runtime().unwrap().crash();
    drop(host);
    let host = reopen_actions_host(&_server, data.path(), cwd.path()).await;
    assert!(
        store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_some(),
        "重启后等待效果还在"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .runs
            .iter()
            .flat_map(|run| run.attempts.iter())
            .count(),
        1,
        "没到 due 不提前开 Attempt"
    );
    router.push_thin_for_title(&task.title, crate::common::text_sse("重启后继续"));
    host.task_runtime()
        .unwrap()
        .reschedule_usage_wait_for_test(task.id.as_str())
        .expect("拨到点");
    wait_until("重启后到点续跑一次", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.runs.iter().flat_map(|run| run.attempts.iter()).count() == 2)
    })
    .await;
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        stored.latest_attempt().unwrap().trigger,
        AttemptTrigger::UsageResume
    );
    assert_eq!(stored.reports.len(), 1, "重启不重复写报告");
}
#[tokio::test]
async fn task_chat_while_waiting_deletes_the_usage_effect_before_a_new_attempt() {
    let (fixture, task, store) = waiting_fixture().await;
    let before = store.task(&task.id).unwrap().unwrap();
    let key = format!(
        "usage-resume:{}:{}",
        task.id,
        before.latest_attempt().unwrap().id.0
    );
    let kind_requests = fixture.router.bodies(false).len();
    fixture
        .router
        .push_thin_for_title(&task.title, crate::common::text_sse("收到"));
    fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "提前接着做".to_owned(),
            pastes: Vec::new(),
            attachments: Vec::new(),
            object_refs: Vec::new(),
        }))
        .await
        .expect("任务对话说话");
    wait_until("任务对话说话开了新 Attempt", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.runs.iter().flat_map(|run| run.attempts.iter()).count() > 1)
    })
    .await;
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        fixture.router.bodies(false).len(),
        kind_requests,
        "任务对话说话不叫醒 Kind"
    );
    assert!(after.latest_attempt().unwrap().stops.is_empty());
    assert_eq!(after.reports.len(), before.reports.len());
    assert!(
        store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_none(),
        "任务对话说话前要删掉旧到点效果"
    );
}
