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

    agent
        .send(UserMessage::text("后台跑一下"), Delivery::FollowUp)
        .await
        .unwrap();
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

/// 动态 mock：第 1 次回 bash run_in_background，第 2 次从请求体里取出 job id 回
/// job_output(wait)，之后回 FINAL。这样「忙碌→NextStep」的测试不依赖固定 id。
struct BusyScriptResponder {
    step: std::sync::Mutex<usize>,
}

impl wiremock::Respond for BusyScriptResponder {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let mut step = self.step.lock().expect("BusyScript 锁被毒化");
        let current = *step;
        *step += 1;
        drop(step);
        match current {
            0 => sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "bash",
                json!({
                    "command": "while [ ! -f go ]; do :; done; printf 'job done\n'",
                    "run_in_background": true
                }),
            )])),
            1 => {
                let body = String::from_utf8_lossy(&request.body);
                let marker = "started background job ";
                let Some(start) = body.find(marker) else {
                    return sse(FINAL_TEXT_SSE.to_owned());
                };
                let rest = &body[start + marker.len()..];
                let end = rest
                    .find(|character: char| {
                        !(character.is_ascii_alphanumeric() || character == '-')
                    })
                    .unwrap_or(rest.len());
                let job_id = &rest[..end];
                sse(tool_calls_with_inputs_sse(&[(
                    "call_1",
                    "job_output",
                    json!({"job_id": job_id, "wait": true, "timeout_ms": 5000}),
                )]))
            }
            _ => sse(FINAL_TEXT_SSE.to_owned()),
        }
    }
}

#[tokio::test]
async fn job_completion_during_turn_arrives_as_next_step() {
    // 忙碌路径：turn 内 job 完成 → 通知进 NextStep（同一轮的后续请求带上），不另开一轮。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(BusyScriptResponder {
            step: std::sync::Mutex::new(0),
        })
        .mount(&server)
        .await;
    let (agent, data_dir, cwd, session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;

    let cwd_path = cwd.path().to_path_buf();
    let notifier = tokio::spawn(async move {
        // 等第一次请求已经在处理（turn 已开始）再放行后台任务。
        for _ in 0..5000 {
            if cwd_path.join("go").exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        std::fs::write(cwd_path.join("go"), "").unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    std::fs::write(cwd.path().join("go"), "").unwrap();
    notifier.abort();

    agent
        .send(UserMessage::text("一边跑后台一边干活"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert!(requests.len() >= 3, "至少 3 次请求：{}", requests.len());
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    let notified_step = bodies[1..]
        .iter()
        .position(|body| body.contains("background job bash-"));
    assert!(
        notified_step.is_some(),
        "忙碌路径的通知应在同一轮的后续请求（NextStep）"
    );
    assert!(bodies[1..].iter().any(|body| body.contains("finished")));

    // 同一轮内送达：日志里只有 1 个 turn/start，没有因通知另开一轮。
    let store = session::SessionStore::open(data_dir.path(), &session_id).unwrap();
    let turns = store
        .events()
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::TurnStart { .. }))
        .count();
    assert_eq!(turns, 1, "忙碌路径不应另开一轮");
}
