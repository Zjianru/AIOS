//! T014.2：按条件开出的工具（每步算一次）和加载记录（tool/registry）。
//!
//! 条件、加载记录都是内核的事：工具由谁实现无所谓，这里用只在测试里注册的替身，
//! 产品的查询工具在服务里，由服务自己的测试盖。

use std::sync::LazyLock;

use agent::{Agent, Delivery};
use model::UserMessage;
use serde_json::{Value, json};
use session::{EventKind, SessionId};
use tools::{
    SkillTool, Tool, ToolCall, ToolCondition, ToolContext, ToolFuture, ToolResult, ToolSet,
};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer};

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, model_config, sse, test_credential,
    tool_calls_with_inputs_sse,
};

/// 替身查询工具：名字和开出条件跟产品里的 `aios_query` 一样，执行永远成功。
struct FakeQueryTool {
    parameters: LazyLock<Value>,
}

impl FakeQueryTool {
    fn new() -> Self {
        Self {
            parameters: LazyLock::new(|| json!({"type": "object", "properties": {}})),
        }
    }
}

impl Tool for FakeQueryTool {
    fn name(&self) -> &str {
        "aios_query"
    }

    fn description(&self) -> &str {
        "read AIOS settings (test double)"
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkill(&["aios-settings"])
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { ToolResult::success(call.id, "queried") })
    }
}

fn tool_set() -> ToolSet {
    ToolSet::new(vec![Box::new(SkillTool::new()), Box::new(FakeQueryTool::new())])
}

/// 四步脚本：没加载时调查询 → 加载 skill → 再调查询 → 收尾回答。
fn four_step_responses() -> Vec<wiremock::ResponseTemplate> {
    vec![
        sse(tool_calls_with_inputs_sse(&[(
            "call_closed",
            "aios_query",
            json!({"topic": "settings"}),
        )])),
        sse(tool_calls_with_inputs_sse(&[(
            "call_skill",
            "skill",
            json!({"name": "aios-settings"}),
        )])),
        sse(tool_calls_with_inputs_sse(&[(
            "call_open",
            "aios_query",
            json!({"topic": "settings"}),
        )])),
        sse(FINAL_TEXT_SSE.to_owned()),
    ]
}

async fn bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .expect("收到过的请求")
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("请求 body 是 JSON"))
        .collect()
}

/// 请求里的工具名，按声明顺序。
fn tool_names(body: &Value) -> Vec<String> {
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

/// 某个调用的工具结果正文（在后续请求的 messages 里找）。
fn tool_result_text(body: &Value, call_id: &str) -> Option<String> {
    for message in body["messages"].as_array()? {
        for block in message["content"].as_array()? {
            if block["type"] == "tool_result" && block["tool_use_id"] == call_id {
                return Some(block["content"][0]["text"].as_str()?.to_owned());
            }
        }
    }
    None
}

fn logged_events(data_dir: &std::path::Path, id: &SessionId) -> Vec<session::Event> {
    session::SessionStore::open_read_only(data_dir, id)
        .expect("读会话日志")
        .events()
        .to_vec()
}

/// 加载记录：每条的（step、新加的工具名）。
fn registry_records(events: &[session::Event]) -> Vec<(u64, Vec<String>)> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolRegistry { step, additions, .. } => Some((
                *step,
                additions.iter().map(|tool| tool.name.clone()).collect(),
            )),
            _ => None,
        })
        .collect()
}

/// 测试决定 3：没加载 `aios-settings` 时本步工具表里没有查询工具，调它得到「未知工具」；
/// 加载以后的下一步才有。
#[tokio::test]
async fn query_tool_stays_closed_until_the_skill_loads() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SequenceResponder::new(four_step_responses()))
        .mount(&server)
        .await;
    let (agent, _data_dir, _cwd, _id) = crate::common::setup_agent(&server.uri(), tool_set()).await;
    agent
        .send(UserMessage::text("现在用的什么模型？"), Delivery::FollowUp)
        .await
        .expect("send");
    crate::common::wait_idle(&agent, 10).await;

    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 4, "四步各发一次请求");
    assert_eq!(
        tool_names(&bodies[0]),
        vec!["skill".to_owned()],
        "没加载 skill 时本步工具表里没有查询工具"
    );
    assert_eq!(
        tool_names(&bodies[1]),
        vec!["skill".to_owned()],
        "加载发生在这一步里，本步的工具表还是旧的"
    );
    assert_eq!(
        tool_names(&bodies[2]),
        vec!["skill".to_owned(), "aios_query".to_owned()],
        "加载以后的下一步才有查询工具"
    );
    let closed = tool_result_text(&bodies[1], "call_closed").expect("没开出的调用也有结果");
    assert!(
        closed.contains("tool \"aios_query\" is not available"),
        "没开出的工具要报「未知工具」：{closed}"
    );
    assert_eq!(
        tool_result_text(&bodies[3], "call_open").as_deref(),
        Some("queried"),
        "加载以后这个工具真的能执行"
    );
}

/// 测试决定 4（工具表部分）：加载记录只在新增时记一条、带完整定义；从日志恢复以后
/// 工具表和恢复前一样，也不再重复记。
#[tokio::test]
async fn tool_registry_records_additions_once_and_restores() {
    let server = MockServer::start().await;
    let mut responses = four_step_responses();
    responses.push(sse(FINAL_TEXT_SSE.to_owned()));
    Mock::given(method("POST"))
        .respond_with(SequenceResponder::new(responses))
        .mount(&server)
        .await;
    let (agent, data_dir, cwd, id) = crate::common::setup_agent(&server.uri(), tool_set()).await;
    agent
        .send(UserMessage::text("现在用的什么模型？"), Delivery::FollowUp)
        .await
        .expect("send");
    crate::common::wait_idle(&agent, 10).await;
    drop(agent);

    let events = logged_events(data_dir.path(), &id);
    let records = registry_records(&events);
    assert_eq!(
        records,
        vec![
            (1, vec!["skill".to_owned()]),
            (3, vec!["aios_query".to_owned()]),
        ],
        "第 1、2、4 步都没有新工具，只在第 1 步和第 3 步各记一条"
    );
    let additions: Vec<&model::ToolSchema> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolRegistry { additions, .. } => Some(additions.iter()),
            _ => None,
        })
        .flatten()
        .collect();
    for tool in &additions {
        assert!(
            !tool.description.is_empty() && tool.parameters.is_object(),
            "加载记录要带完整定义"
        );
    }

    let before = bodies(&server).await;
    let last_tools = before[3]["tools"].clone();

    let store = session::SessionStore::open(data_dir.path(), &id).expect("重开日志");
    let resumed = Agent::resume(
        id.clone(),
        crate::common::isolated_config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(model_config(&server.uri()), test_credential())
                .expect("模型"),
            tools: tool_set(),
            store,
        },
    )
    .await
    .expect("续接");
    resumed
        .send(UserMessage::text("接着来"), Delivery::FollowUp)
        .await
        .expect("send");
    crate::common::wait_idle(&resumed, 10).await;
    drop(resumed);

    let after = bodies(&server).await;
    assert_eq!(after.len(), 5, "续接的那一轮也发一次请求");
    assert_eq!(
        after[4]["tools"], last_tools,
        "恢复以后的工具表要和恢复前逐项相同"
    );
    assert_eq!(
        registry_records(&logged_events(data_dir.path(), &id)),
        records,
        "恢复不会重复记加载记录"
    );
    let _ = cwd;
}
