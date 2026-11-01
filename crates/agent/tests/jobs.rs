//! 接缝 1：后台任务完成通知与 JobCompleted 事件（测试清单第 4 条 + 票面加项）。

mod common;

use std::fs;
use std::time::Duration;

use agent::Delivery;
use common::{FINAL_TEXT_SSE, SequenceResponder, setup_agent, sse, tool_calls_with_inputs_sse};
use model::UserMessage;
use serde_json::json;
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

#[tokio::test]
async fn background_job_completion_notifies_next_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "bash",
                json!({
                    "command": "while [ ! -f go ]; do :; done; printf 'done\n'",
                    "run_in_background": true
                }),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, _data_dir, cwd, _id) = setup_agent(&server.uri(), ToolSet::builtin()).await;

    agent.send(UserMessage::text("后台跑一下"), Delivery::FollowUp);
    agent.idle().await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "turn1 应有两次请求"
    );

    let mut events = agent.subscribe();
    fs::write(cwd.path().join("go"), "").unwrap();

    let notified = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if server.received_requests().await.unwrap().len() >= 3 {
                return true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or(false);
    assert!(notified, "完成通知没有开启下一轮请求");
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let last = String::from_utf8(requests.last().unwrap().body.clone()).unwrap();
    assert!(last.contains("background job bash-"), "{last}");
    assert!(last.contains("finished"), "{last}");

    let mut saw_job_completed = false;
    while let Ok(event) = events.try_recv() {
        if matches!(event, agent::AgentEvent::JobCompleted { .. }) {
            saw_job_completed = true;
        }
    }
    assert!(saw_job_completed, "缺 JobCompleted 事件");
}
