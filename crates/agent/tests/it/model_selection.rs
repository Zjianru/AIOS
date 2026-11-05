//! T012.7：会话模型选择（session/model 事件；下一轮生效，本轮冻结）。
//!
//! 接缝：agent 入口（Agent::select/send）+ wiremock 的 HTTP 接缝（测试决定 (b)）。

use serde_json::Value;
use session::SessionStore;
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, test_credential,
    wait_idle,
};

async fn create_agent(
    server: &MockServer,
    responses: usize,
) -> (agent::Agent, tempfile::TempDir, session::SessionId) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            (0..responses)
                .map(|_| sse(FINAL_TEXT_SSE.to_owned()))
                .collect(),
        ))
        .mount(server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = agent::Agent::create(
        isolated_config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config(&server.uri()),
                test_credential(),
            )
            .unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, id)
}

fn log_rows(data_dir: &std::path::Path, id: &session::SessionId) -> Vec<Value> {
    let text = std::fs::read_to_string(data_dir.join("sessions").join(format!("{id}.jsonl")))
        .expect("session log");
    text.lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).expect("event line"))
        .collect()
}

/// T012.7 故事 2/8：选了以后正在跑的轮用旧模型、下一轮用新的；日志记 session/model。
#[tokio::test]
async fn 选择写session_model事件且下一轮请求用新模型和档() {
    let server = MockServer::start().await;
    let (agent, data_dir, id) = create_agent(&server, 2).await;
    agent
        .send(
            model::UserMessage::text("第一个问题"),
            agent::Delivery::FollowUp,
        )
        .await
        .unwrap();
    wait_idle(&agent, 20).await;

    // 用户换到 MiniMax 连接/模型/低档：从下一轮开始生效。
    agent
        .select(&session::ModelSelection {
            connection: "minimax-cn".to_owned(),
            model: "MiniMax-M3.1-Flash".to_owned(),
            effort_wanted: "low".to_owned(),
        })
        .await
        .unwrap();

    let rows = log_rows(data_dir.path(), &id);
    let event = rows
        .iter()
        .find(|row| row["type"] == "session/model")
        .expect("session/model 事件");
    assert_eq!(event["data"]["connection"], "minimax-cn");
    assert_eq!(event["data"]["model"], "MiniMax-M3.1-Flash");
    assert_eq!(event["data"]["effortWanted"], "low");

    agent
        .send(
            model::UserMessage::text("第二个问题"),
            agent::Delivery::FollowUp,
        )
        .await
        .unwrap();
    wait_idle(&agent, 20).await;

    let requests = server.received_requests().await.expect("请求记录");
    assert_eq!(requests.len(), 2, "两轮各一次请求");
    let first: Value = serde_json::from_slice(&requests[0].body).expect("body");
    assert_eq!(first["model"], "deepseek-flash", "本轮先用旧模型");
    let second: Value = serde_json::from_slice(&requests[1].body).expect("body");
    assert_eq!(
        second["model"], "MiniMax-M3.1-Flash",
        "下一轮用新模型：{second}"
    );
    assert_eq!(second["thinking"]["type"], "enabled");
    assert_eq!(
        second["output_config"]["effort"], "low",
        "下一轮用新档：{second}"
    );
}
