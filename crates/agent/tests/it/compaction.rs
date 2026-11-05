//! 接缝 1：自动压缩与溢出恢复（测试清单第 9 条）。

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, wait_idle,
};
use agent::{Agent, AgentConfig, AgentEvent, CompactionTrigger, Delivery};
use model::UserMessage;
use serde_json::json;
use session::{EventKind, SessionStore};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Harness {
    agent: Agent,
    data_dir: tempfile::TempDir,
    id: session::SessionId,
}

async fn setup(
    server: &MockServer,
    window: u64,
    max_tokens: u32,
    headroom: u64,
    retain_ratio: f64,
) -> Harness {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let config = AgentConfig {
        compaction_headroom_tokens: headroom,
        compaction_retain_ratio: retain_ratio,
        ..isolated_config(data_dir.path())
    };
    let model_config = model::DeepSeekConfig {
        context_window: window,
        max_tokens,
        ..model_config(&server.uri())
    };
    let agent = Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config,
                crate::common::test_credential(),
            )
            .unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    Harness {
        agent,
        data_dir,
        id,
    }
}

fn body(requests: &[wiremock::Request], index: usize) -> String {
    String::from_utf8_lossy(&requests[index].body).into_owned()
}

fn overflow_response() -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({
        "error": {"type": "invalid_request_error", "message": "This model's maximum context length is 128000 tokens, you requested 200000"}
    }))
}

#[tokio::test]
async fn pressure_triggers_compaction_and_next_request_uses_summary() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let harness = setup(&server, 1000, 100, 1, 0.16).await;
    let mut events = harness.agent.subscribe();

    harness
        .agent
        .send(
            UserMessage::text("TEXT-ONE early request"),
            Delivery::FollowUp,
        )
        .await
        .unwrap();
    harness.agent.idle().await;
    harness
        .agent
        .send(
            UserMessage::text(format!("TEXT-TWO {}", "y".repeat(3000))),
            Delivery::FollowUp,
        )
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    assert!(bodies.len() >= 3, "请求数应含摘要与压缩后请求");
    let summary_request = bodies
        .iter()
        .find(|body| body.contains("compaction engine"))
        .expect("应有一次摘要请求");
    assert!(summary_request.contains("TEXT-ONE"));
    let compacted_request = bodies
        .iter()
        .find(|body| body.contains("TEXT-TWO"))
        .expect("压缩后的请求应带近期历史");
    assert!(
        compacted_request.contains("<compacted-summary>"),
        "压缩后的请求应由摘要 + 近期历史组成"
    );
    assert!(
        !compacted_request.contains("TEXT-ONE"),
        "较早历史应被摘要替换"
    );

    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    let kinds: Vec<&str> = store
        .events()
        .iter()
        .map(|event| event.kind.type_name())
        .filter(|name| name.starts_with("compaction/"))
        .collect();
    // T006.3 照 DSH：compactionRetries 默认 1——这次压缩后估算仍超阈值，会再压一次，
    // 每次都是一组 start/summary/end。
    assert_eq!(
        kinds,
        vec![
            "compaction/start",
            "compaction/summary",
            "compaction/end",
            "compaction/start",
            "compaction/summary",
            "compaction/end"
        ],
        "压缩事件按顺序落日志（重试一次）"
    );
    let seqs: Vec<u64> = store.events().iter().map(|event| event.seq).collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "日志只追加（seq 递增）"
    );

    let mut started = false;
    let mut ended = false;
    while let Ok(event) = events.try_recv() {
        match event {
            AgentEvent::CompactionStarted {
                trigger: CompactionTrigger::Auto,
                ..
            } => started = true,
            AgentEvent::CompactionEnded { error: None, .. } => ended = true,
            _ => {}
        }
    }
    assert!(started, "应发 CompactionStarted 事件");
    assert!(ended, "应发 CompactionEnded 事件");
}

#[tokio::test]
async fn overflow_compacts_then_retries_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            overflow_response(),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let harness = setup(&server, 1_000_000, 2048, 65_536, 0.000001).await;
    harness
        .agent
        .send(UserMessage::text("OVERFLOW-ONE early"), Delivery::FollowUp)
        .await
        .unwrap();
    harness.agent.idle().await;
    harness
        .agent
        .send(UserMessage::text("OVERFLOW-TWO big"), Delivery::FollowUp)
        .await
        .unwrap();
    harness.agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        4,
        "原请求 + 摘要请求 + 重试请求 = 4（含 turn1）"
    );
    assert!(
        body(&requests, 2).contains("compaction engine"),
        "溢出后先发摘要请求"
    );
    assert!(
        body(&requests, 3).contains("<compacted-summary>"),
        "重试请求用压缩后的派生视图"
    );
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    assert!(
        store
            .events()
            .iter()
            .any(|event| matches!(event.kind, EventKind::CompactionEnd { error: None, .. })),
        "溢出恢复也要写完整的压缩事件"
    );
}

#[tokio::test]
async fn second_overflow_after_compaction_is_fatal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            overflow_response(),
            sse(FINAL_TEXT_SSE.to_owned()),
            overflow_response(),
        ]))
        .mount(&server)
        .await;
    let harness = setup(&server, 1_000_000, 2048, 65_536, 0.000001).await;
    let mut events = harness.agent.subscribe();
    harness
        .agent
        .send(UserMessage::text("FATAL-ONE early"), Delivery::FollowUp)
        .await
        .unwrap();
    harness.agent.idle().await;
    harness
        .agent
        .send(UserMessage::text("FATAL-TWO big"), Delivery::FollowUp)
        .await
        .unwrap();
    harness.agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4, "第二次溢出不再重试（不无限循环）");
    let mut errored = false;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded {
            outcome: agent::TurnOutcome::Error { .. },
            ..
        } = event
        {
            errored = true;
        }
    }
    assert!(errored, "第二次溢出应按正常错误收尾");
}
