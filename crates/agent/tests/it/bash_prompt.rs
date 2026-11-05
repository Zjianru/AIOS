//! 接缝 1：bash 指导句只随 bash 工具出现（测试清单第 3 条 + 提示词回归）。

use crate::common::{FINAL_TEXT_SSE, SequenceResponder, setup_agent, sse};
use agent::Delivery;
use model::UserMessage;
use tools::{BashTool, ToolSet, WriteTool};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

async fn system_prompt_for(tools: ToolSet) -> String {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(&server)
        .await;
    let (agent, _data_dir, _cwd, _id) = setup_agent(&server.uri(), tools).await;
    agent
        .send(UserMessage::text("hi"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    body["system"].as_str().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn bash_only_system_prompt_mentions_exit_code_only() {
    let system = system_prompt_for(ToolSet::new(vec![Box::new(BashTool::new())])).await;
    assert!(
        system.contains("Check the [exit code: N] marker on every bash result"),
        "bash 指导句缺失：{system}"
    );
    assert!(
        !system.contains("Use the read tool"),
        "read 不可见时不提 read：{system}"
    );
    assert!(
        !system.contains("Read an existing file before overwriting"),
        "write 不可见时不提 write：{system}"
    );
}

#[tokio::test]
async fn bash_guidance_stays_with_coding_tools() {
    let system = system_prompt_for(ToolSet::new(vec![
        Box::new(WriteTool::new()),
        Box::new(BashTool::new()),
    ]))
    .await;
    assert!(
        system.contains("Check the [exit code: N] marker on every bash result"),
        "{system}"
    );
    assert!(
        system.contains("Read an existing file before overwriting it with write"),
        "{system}"
    );
}
