//! T024.2：Agent 的工具策略（only 名单、no_wait 拒绝）与系统提示追加规则。
//!
//! 机制层照 DSH 的 noWait（`app/sessions.ts:73-87`）与 systemPrompt.section（`:244`）；
//! 测试只走产品的组装入口 Agent::open，模型 HTTP 在边界用 mock 服务替换。

use std::path::Path;
use std::sync::Arc;

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, wait_idle,
};
use agent::{Agent, AgentError, AgentOpenOptions, Delivery, ToolPolicy};
use model::UserMessage;
use tools::{Tool, ToolCall, ToolContext, ToolFuture, ToolResult};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

/// 服务装配才会传的额外工具；用来证明 only 名单覆盖 builtin 与 extra_tools 的并集。
struct ProbeTool {
    parameters: serde_json::Value,
}

impl ProbeTool {
    fn new() -> Self {
        Self {
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
}

impl Tool for ProbeTool {
    fn name(&self) -> &str {
        "probe"
    }

    fn description(&self) -> &str {
        "test-only extra tool"
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { ToolResult::success(call.id, "probe") })
    }
}

async fn mount(server: &MockServer, responses: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            responses.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

fn options(
    data_dir: &Path,
    cwd: &Path,
    base_url: &str,
    resume: Option<session::SessionId>,
    tool_policy: ToolPolicy,
) -> AgentOpenOptions {
    AgentOpenOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        resume,
        model: model_config(base_url),
        config: isolated_config(data_dir),
        credentials: Some(crate::common::test_credential()),
        append_hook_factory: None,
        extra_tools: vec![Arc::new(ProbeTool::new())],
        tool_policy,
        policy_prompt: None,
    }
}

fn tool_names(body: &serde_json::Value) -> Vec<String> {
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

async fn request_body(server: &MockServer, index: usize) -> serde_json::Value {
    let requests = server.received_requests().await.expect("收到过的请求");
    serde_json::from_slice(&requests[index].body).expect("请求 body 是 JSON")
}

#[tokio::test]
async fn only_keeps_builtin_and_extra_tools_and_resume_does_not_revive_them() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![FINAL_TEXT_SSE.to_owned(), FINAL_TEXT_SSE.to_owned()],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let agent = Agent::open(options(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        None,
        ToolPolicy {
            only: Some(vec!["read".to_owned(), "probe".to_owned()]),
            no_wait: false,
        },
    ))
    .await
    .expect("only 名单都在 builtin 与 extra_tools 的并集里");
    let id = agent.id().clone();
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    assert_eq!(
        tool_names(&request_body(&server, 0).await),
        ["read", "probe"],
        "只保留名单内的工具，按注册顺序"
    );
    drop(agent);

    let resumed = Agent::open(options(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        Some(id),
        ToolPolicy {
            only: Some(vec!["probe".to_owned()]),
            no_wait: false,
        },
    ))
    .await
    .expect("续接的 only 名单合法");
    resumed
        .send(UserMessage::text("续接一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 10).await;
    assert_eq!(
        tool_names(&request_body(&server, 1).await),
        ["probe"],
        "续接不把上一轮的工具从日志里复活"
    );
}

#[tokio::test]
async fn unknown_names_in_only_are_invalid_config() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let error = Agent::open(options(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        None,
        ToolPolicy {
            only: Some(vec!["read".to_owned(), "nope".to_owned()]),
            no_wait: false,
        },
    ))
    .await
    .err()
    .expect("名单里的未知名字要拒绝组装");
    match error {
        AgentError::InvalidConfig { field, message } => {
            assert_eq!(field, "tool_policy.only");
            assert!(message.contains("nope"), "报出未知名字：{message}");
        }
        other => panic!("期望 InvalidConfig，得到 {other:?}"),
    }
    assert!(
        !data_dir.path().join("sessions").exists(),
        "组装失败不留下会话文件"
    );
}

#[tokio::test]
async fn policy_prompt_is_appended_at_the_end_of_the_system_prompt() {
    let server = MockServer::start().await;
    mount(&server, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let policy = "Product rule: you route work and never execute it yourself.";
    let mut open = options(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        None,
        ToolPolicy::default(),
    );
    open.policy_prompt = Some(policy.to_owned());

    let agent = Agent::open(open).await.expect("追加规则不影响组装");
    agent
        .send(UserMessage::text("你好"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    let body = request_body(&server, 0).await;
    let system = body["system"].as_str().expect("请求带系统提示");
    assert!(
        system.ends_with(&format!("\n\n{policy}")),
        "产品规则要在系统提示末尾：{system}"
    );
}
