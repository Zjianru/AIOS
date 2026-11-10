//! Agent 的工具策略：only 名单过滤、no_wait 拒绝，以及追加在系统提示末尾的产品规则。
//!
//! 机制层照 DSH 的 noWait（`app/sessions.ts:73-87`）与 systemPrompt.section（`:244`）；
//! 测试只走产品的组装入口 Agent::open，模型 HTTP 在边界用 mock 服务替换。

use std::path::Path;
use std::sync::Arc;

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse,
    tool_calls_with_inputs_sse, wait_idle,
};
use agent::{
    Agent, AgentError, AgentOpenOptions, Delivery, NO_WAIT, STOP_AFTER_REPORT, ToolPolicy,
};
use model::{ContentBlock, UserMessage};
use session::{EventKind, SessionStore};
use tools::{Tool, ToolCall, ToolContext, ToolFuture, ToolResult};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

/// 服务装配才会传的额外工具；用来证明 only 名单覆盖 builtin 与 extra_tools 的并集。
struct ProbeTool {
    name: &'static str,
    parameters: serde_json::Value,
}

impl ProbeTool {
    fn new() -> Self {
        Self::named("probe")
    }

    /// 同名替身：收尾停的放行名单按名字判，测试要一个 task_report。
    fn named(name: &'static str) -> Self {
        Self {
            name,
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
}

impl Tool for ProbeTool {
    fn name(&self) -> &str {
        self.name
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
        Box::pin(async move { ToolResult::success(call.id, call.name) })
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
    options_with(
        data_dir,
        cwd,
        base_url,
        resume,
        tool_policy,
        vec![Arc::new(ProbeTool::new())],
    )
}

fn options_with(
    data_dir: &Path,
    cwd: &Path,
    base_url: &str,
    resume: Option<session::SessionId>,
    tool_policy: ToolPolicy,
    extra_tools: Vec<Arc<dyn Tool>>,
) -> AgentOpenOptions {
    AgentOpenOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        resume,
        model: model_config(base_url),
        config: isolated_config(data_dir),
        credentials: Some(crate::common::test_credential()),
        append_hook_factory: None,
        extra_tools,
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

#[tokio::test]
async fn no_wait_records_the_call_then_returns_an_error_result_without_running_it() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[
                (
                    "call_sleep",
                    "bash",
                    serde_json::json!({"description": "wait", "command": "sleep 1"}),
                ),
                (
                    "call_echo",
                    "bash",
                    serde_json::json!({
                        "description": "quick",
                        "command": "echo policy-pass",
                        "timeoutMs": 20000
                    }),
                ),
            ]),
            FINAL_TEXT_SSE.to_owned(),
        ],
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
            only: None,
            no_wait: true,
        },
    ))
    .await
    .expect("no_wait 打开不影响组装");
    let id = agent.id().clone();
    agent
        .send(UserMessage::text("跑两个命令"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    drop(agent);

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let events = store.events();
    let call_position = |call_id: &str| {
        events
            .iter()
            .position(|event| {
                matches!(
                    &event.kind,
                    EventKind::ToolCall { call_id: id, .. } if id == call_id
                )
            })
            .unwrap_or_else(|| panic!("日志里有 {call_id} 的 tool/call"))
    };
    let result_position = |call_id: &str| {
        events
            .iter()
            .position(|event| {
                matches!(
                    &event.kind,
                    EventKind::ToolResult { message, .. }
                        if message.tool_call_id.as_deref() == Some(call_id)
                )
            })
            .unwrap_or_else(|| panic!("日志里有 {call_id} 的 tool/result"))
    };

    let sleep_call = call_position("call_sleep");
    let echo_call = call_position("call_echo");
    let sleep_result = result_position("call_sleep");
    let echo_result = result_position("call_echo");
    assert!(
        sleep_call < sleep_result && echo_call < echo_result,
        "先记 tool/call，再给 tool/result"
    );
    assert!(
        sleep_call < echo_result && echo_call < sleep_result,
        "同一步的所有 tool/call 都在结果之前"
    );

    let (sleep_message, sleep_error) = match &events[sleep_result].kind {
        EventKind::ToolResult { message, error, .. } => (message, error),
        other => panic!("期望 tool/result，得到 {}", other.type_name()),
    };
    assert_eq!(sleep_message.is_error, Some(true), "命中的调用是错误结果");
    let sleep_text = sleep_message
        .content
        .first()
        .and_then(ContentBlock::as_text)
        .expect("tool/result 里是文本");
    assert_eq!(
        sleep_text,
        format!("Error: {NO_WAIT}"),
        "拒绝语照 NO_WAIT 常量"
    );
    assert_eq!(sleep_error, &None, "拒绝不是工具的结构化错误码");

    let (echo_message, _) = match &events[echo_result].kind {
        EventKind::ToolResult { message, error, .. } => (message, error),
        other => panic!("期望 tool/result，得到 {}", other.type_name()),
    };
    assert_eq!(
        echo_message.is_error, None,
        "timeoutMs 20000 的调用照常执行"
    );
    let echo_text = echo_message
        .content
        .first()
        .and_then(ContentBlock::as_text)
        .expect("tool/result 里是文本");
    assert!(echo_text.contains("policy-pass"), "{echo_text}");
}

#[tokio::test]
async fn stop_after_report_records_each_call_then_denies_everything_but_task_report() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[
                (
                    "call_read",
                    "read",
                    serde_json::json!({"file_path": "README.md"}),
                ),
                ("call_probe", "probe", serde_json::json!({})),
                (
                    "call_report",
                    "task_report",
                    serde_json::json!({"state": "ready", "summary": "查完了"}),
                ),
            ]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let agent = Agent::open(options_with(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        None,
        ToolPolicy::default(),
        vec![
            Arc::new(ProbeTool::new()),
            Arc::new(ProbeTool::named("task_report")),
        ],
    ))
    .await
    .expect("收尾标记不影响组装");
    agent.stop_after_report();
    let id = agent.id().clone();
    agent
        .send(UserMessage::text("收尾停之前的一步"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    drop(agent);

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let events = store.events();
    let position = |call_id: &str, call: bool| {
        events
            .iter()
            .position(|event| match &event.kind {
                EventKind::ToolCall { call_id: id, .. } if call => id == call_id,
                EventKind::ToolResult { message, .. } if !call => {
                    message.tool_call_id.as_deref() == Some(call_id)
                }
                _ => false,
            })
            .unwrap_or_else(|| {
                panic!(
                    "日志里要有 {call_id} 的 tool/{}",
                    if call { "call" } else { "result" }
                )
            })
    };
    for call_id in ["call_read", "call_probe", "call_report"] {
        assert!(
            position(call_id, true) < position(call_id, false),
            "{call_id}：先记 tool/call，再给 tool/result"
        );
    }
    let result_text = |call_id: &str| {
        let index = position(call_id, false);
        match &events[index].kind {
            EventKind::ToolResult { message, .. } => (
                message
                    .content
                    .first()
                    .and_then(ContentBlock::as_text)
                    .map(str::to_owned),
                message.is_error,
            ),
            other => panic!("期望 tool/result，得到 {}", other.type_name()),
        }
    };
    for call_id in ["call_read", "call_probe"] {
        let (text, is_error) = result_text(call_id);
        assert_eq!(is_error, Some(true), "{call_id} 要被拒绝");
        assert_eq!(text, Some(format!("Error: {STOP_AFTER_REPORT}")));
    }
    let (text, is_error) = result_text("call_report");
    assert_eq!(is_error, None, "task_report 放行");
    assert_eq!(text.as_deref(), Some("task_report"));
}

#[tokio::test]
async fn a_new_session_does_not_inherit_the_after_report_stop() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[(
                "call_read",
                "read",
                serde_json::json!({"file_path": "README.md"}),
            )]),
            FINAL_TEXT_SSE.to_owned(),
            tool_calls_with_inputs_sse(&[(
                "call_read",
                "read",
                serde_json::json!({"file_path": "README.md"}),
            )]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("README.md"), "readable").unwrap();

    let stopped = Agent::open(options(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        None,
        ToolPolicy::default(),
    ))
    .await
    .expect("组装");
    stopped.stop_after_report();
    stopped
        .send(UserMessage::text("收尾停的一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&stopped, 10).await;
    drop(stopped);

    // 下一次 Attempt 是另一个 Agent/会话：收尾标记不跟着走。
    let next = Agent::open(options(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        None,
        ToolPolicy::default(),
    ))
    .await
    .expect("组装下一个会话");
    let id = next.id().clone();
    next.send(UserMessage::text("下一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&next, 10).await;
    drop(next);

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let read_result = store.events().iter().find_map(|event| match &event.kind {
        EventKind::ToolResult { message, .. }
            if message.tool_call_id.as_deref() == Some("call_read") =>
        {
            Some((
                message
                    .content
                    .first()
                    .and_then(ContentBlock::as_text)
                    .map(str::to_owned),
                message.is_error,
            ))
        }
        _ => None,
    });
    let (text, is_error) = read_result.expect("下一会话有 read 的结果");
    assert_eq!(is_error, None, "新会话不继承收尾标记");
    assert!(
        text.as_deref()
            .is_some_and(|text| text.contains("readable")),
        "{text:?}"
    );
}
