//! T010.1 清单 1：回放往返——用真适配器跑一段假模型的日志，再拿日志回放同一段对话，
//! 新日志里每条 assistant/message 的 content 和 usage 和原日志相同。

use std::sync::Arc;

use agent::{Agent, Delivery};
use model::{CredentialContext, CredentialSource, ModelError, UserMessage};
use replay::ReplayScript;
use replay::start_replay_server;
use session::SessionStore;
use tools::ToolSet;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const FINAL_TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_final","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":50,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"回放往返的第一段。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

struct TestCredential;

impl CredentialSource for TestCredential {
    fn issue(&self, _purpose: &str, _context: &CredentialContext) -> Result<String, ModelError> {
        Ok("test-key".to_owned())
    }
}

struct TestAgent {
    agent: Agent,
    _data_dir: tempfile::TempDir,
    _cwd: tempfile::TempDir,
    log_path: std::path::PathBuf,
}

async fn setup_agent(base_url: &str) -> TestAgent {
    let data_dir = tempfile::tempdir().expect("数据目录");
    let cwd = tempfile::tempdir().expect("工作目录");
    let store = SessionStore::create(data_dir.path(), cwd.path()).expect("建会话");
    let log_path = data_dir
        .path()
        .join("sessions")
        .join(format!("{}.jsonl", store.id()));
    let config = agent::AgentConfig {
        instruction_roots: vec![data_dir.path().join("agents-home")],
        skill_roots: vec![data_dir.path().join("skills-home")],
        ..agent::AgentConfig::default()
    };
    let config_model = model::DeepSeekConfig {
        base_url: base_url.to_owned(),
        model: "deepseek-flash".to_owned(),
        max_tokens: 2048,
        context_window: model::DEFAULT_CONTEXT_WINDOW,
        effort: model::Effort::new("high"),
    };
    let agent = Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(config_model, Arc::new(TestCredential))
                .expect("模型适配器"),
            tools: ToolSet::empty(),
            store,
        },
    )
    .await
    .expect("建 Agent");
    TestAgent {
        agent,
        _data_dir: data_dir,
        _cwd: cwd,
        log_path,
    }
}

async fn run_one_turn(agent: &Agent) {
    agent
        .send(UserMessage::text("说一句话"), Delivery::FollowUp)
        .await
        .expect("发消息");
    agent.idle().await;
}

fn assistant_messages(
    events: &[session::Event],
) -> Vec<(Vec<model::ContentBlock>, Option<model::Usage>)> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::AssistantMessage { message, usage, .. } => {
                Some((message.content.clone(), usage.clone()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn replay_roundtrip_keeps_content_and_usage() {
    let mock = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(FINAL_TEXT_SSE),
        )
        .mount(&mock)
        .await;

    let first = setup_agent(&mock.uri()).await;
    run_one_turn(&first.agent).await;
    let original = assistant_messages(&first.agent.session_events_from(0));
    assert_eq!(original.len(), 1, "第一次对话应有一条 assistant/message");

    let script = ReplayScript::from_log(&first.log_path).expect("从日志推脚本");
    assert_eq!(script.len(), 1, "日志里应有一次模型调用");
    let server = start_replay_server(script).await.expect("起回放服务");

    let second = setup_agent(&server.url()).await;
    run_one_turn(&second.agent).await;
    let replayed = assistant_messages(&second.agent.session_events_from(0));
    assert_eq!(replayed.len(), 1, "回放后应有一条 assistant/message");

    assert_eq!(
        replayed[0].0, original[0].0,
        "回放后的 content 必须和原日志相同"
    );
    assert_eq!(
        replayed[0].1, original[0].1,
        "回放后的 usage 必须和原日志相同"
    );

    server.shutdown();
    drop(second);
    drop(first);
}
