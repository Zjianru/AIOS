//! T084.2：长 429 进用量等待，到点由泵开新 Attempt（服务端真任务运行时、假模型）。

use std::time::Duration;

use super::*;
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

#[tokio::test]
async fn long_429_enters_usage_wait_and_resumes_at_due() {
    let fixture = Fixture::new().await;
    // 第一个 ThinAgent 请求就吃 11 秒后的 429（超过 T084.1 的 10 秒门限）。
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
    assert!(stored.reports.is_empty(), "用量等待不写报告，不叫醒 Kind");
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
