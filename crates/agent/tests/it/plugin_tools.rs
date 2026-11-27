//! T094.2：插件打开/关掉后「下一 step 工具表」变了（全量工具表路由）。

use std::sync::Arc;

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, test_credential,
    wait_idle,
};
use agent::{Agent, AgentOpenOptions, Delivery, ToolPolicy};
use model::UserMessage;
use serde_json::json;
use tools::{PluginId, Tool, ToolCall, ToolContext, ToolFuture, ToolResult};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

struct OfficeRead;

impl Tool for OfficeRead {
    fn name(&self) -> &str {
        "office_read"
    }

    fn description(&self) -> &str {
        "读 Office"
    }

    fn parameters(&self) -> &serde_json::Value {
        static PARAMS: std::sync::LazyLock<serde_json::Value> =
            std::sync::LazyLock::new(|| json!({"type": "object"}));
        &PARAMS
    }

    fn plugin(&self) -> Option<PluginId> {
        Some(PluginId::Office)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { ToolResult::success(call.id, "ok") })
    }
}

fn options(data_dir: &std::path::Path, cwd: &std::path::Path, base_url: &str) -> AgentOpenOptions {
    options_for_route(data_dir, cwd, base_url, false)
}

fn options_for_route(
    data_dir: &std::path::Path,
    cwd: &std::path::Path,
    base_url: &str,
    addition_only: bool,
) -> AgentOpenOptions {
    let mut model = model_config(base_url);
    if addition_only {
        model.tool_update = Some(model::ToolUpdate::AdditionOnly);
    } else {
        // 全量工具表路由：每次请求按当前开关重算。
        model.tool_update = None;
    }
    AgentOpenOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        resume: None,
        model,
        config: isolated_config(data_dir),
        credentials: Some(test_credential()),
        append_hook_factory: None,
        extra_tools: vec![Arc::new(OfficeRead)],
        tool_policy: ToolPolicy {
            only: Some(vec!["read".to_owned(), "office_read".to_owned()]),
            no_wait: false,
        },
        policy_prompt: None,
    }
}

async fn mount(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            vec![FINAL_TEXT_SSE; 4]
                .into_iter()
                .map(|response| sse(response.to_owned()))
                .collect(),
        ))
        .mount(server)
        .await;
}

async fn request_tools(server: &MockServer, index: usize) -> Vec<String> {
    let requests = server.received_requests().await.expect("收到过的请求");
    let body: serde_json::Value = serde_json::from_slice(&requests[index].body).expect("JSON");
    body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool["name"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn next_step_tool_table_follows_plugin_switch() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let switches = tools::plugins::switches_for(data_dir.path());
    let agent = Agent::open(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .expect("插件关着也能组装");
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    assert_eq!(
        request_tools(&server, 0).await,
        ["read"],
        "插件关着时工具表没有 office_read"
    );

    switches.enable(PluginId::Office);
    agent
        .send(UserMessage::text("第二轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    assert_eq!(
        request_tools(&server, 1).await,
        ["read", "office_read"],
        "打开后下一 step 工具表出现 office_read（不重开 Agent）"
    );

    switches.disable(PluginId::Office);
    agent
        .send(UserMessage::text("第三轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    assert_eq!(
        request_tools(&server, 2).await,
        ["read"],
        "关掉后下一 step 工具表移除；历史声明不复活"
    );
}

#[tokio::test]
async fn addition_only_route_adds_office_read_at_next_step() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let switches = tools::plugins::switches_for(data_dir.path());
    let agent = Agent::open(options_for_route(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        true,
    ))
    .await
    .expect("组装");
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    assert_eq!(request_tools(&server, 0).await, ["read"]);

    switches.enable(PluginId::Office);
    agent
        .send(UserMessage::text("第二轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    assert_eq!(
        request_tools(&server, 1).await,
        ["read", "office_read"],
        "addition-only 路由下一 step 追加 office_read，前缀（read）不变"
    );
}
