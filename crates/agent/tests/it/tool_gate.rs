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
    tool_calls_with_inputs_sse, wait_idle,
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
    ToolSet::new(vec![
        Box::new(SkillTool::new()),
        Box::new(FakeQueryTool::new()),
    ])
}

/// 这一组测试的配置：自带 skill 由产品装配决定，这里显式给它一份，别的不碰用户环境。
fn config(data_dir: &std::path::Path) -> agent::AgentConfig {
    agent::AgentConfig {
        bundled_skills: tools::bundled_skills().to_vec(),
        ..crate::common::isolated_config(data_dir)
    }
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

/// 临时数据目录上的 agent（自带 skill 显式给）。
async fn setup(
    server: &MockServer,
    tools: ToolSet,
) -> (Agent, tempfile::TempDir, tempfile::TempDir, SessionId) {
    setup_with_model(server, tools, model_config(&server.uri())).await
}

/// 同 setup，但模型配置由调用方给（要测声明了能力的路由时用）。
async fn setup_with_model(
    _server: &MockServer,
    tools: ToolSet,
    model: model::DeepSeekConfig,
) -> (Agent, tempfile::TempDir, tempfile::TempDir, SessionId) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(model, test_credential()).expect("模型"),
            tools,
            store,
        },
    )
    .await
    .expect("建 agent");
    (agent, data_dir, cwd, id)
}

/// 声明了 `addition-only` 的模型配置（票面：加载工具不改前缀）。
fn addition_only_config(base_url: &str) -> model::DeepSeekConfig {
    model::DeepSeekConfig {
        tool_update: Some(model::ToolUpdate::AdditionOnly),
        ..model_config(base_url)
    }
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
            EventKind::ToolRegistry {
                step, additions, ..
            } => Some((
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
    let (agent, _data_dir, _cwd, _id) = setup(&server, tool_set()).await;
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
    let (agent, data_dir, cwd, id) = setup(&server, tool_set()).await;
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
        config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config(&server.uri()),
                test_credential(),
            )
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

/// 票面：从日志恢复后发出的请求，和恢复前同一步的请求逐字节相同。
#[tokio::test]
async fn restored_request_is_byte_identical_to_the_same_step_before_restore() {
    let server = MockServer::start().await;
    let mut responses = four_step_responses();
    responses.push(sse(FINAL_TEXT_SSE.to_owned()));
    responses.push(sse(FINAL_TEXT_SSE.to_owned()));
    Mock::given(method("POST"))
        .respond_with(SequenceResponder::new(responses))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, id) =
        setup_with_model(&server, tool_set(), addition_only_config(&server.uri())).await;
    agent
        .send(UserMessage::text("现在用的什么模型？"), Delivery::FollowUp)
        .await
        .expect("send");
    wait_idle(&agent, 10).await;

    // 恢复点 = 这一刻的日志（下一轮还没发出去）。复制到另一个数据目录再续接。
    let restored_dir = tempfile::tempdir().unwrap();
    let log_name = format!("{id}.jsonl");
    let restored_sessions = restored_dir.path().join("sessions");
    std::fs::create_dir_all(&restored_sessions).unwrap();
    std::fs::copy(
        data_dir.path().join("sessions").join(&log_name),
        restored_sessions.join(&log_name),
    )
    .expect("复制会话日志");

    // 恢复前：同一个日志状态发出下一轮第一步的请求。
    agent
        .send(UserMessage::text("接着来"), Delivery::FollowUp)
        .await
        .expect("send");
    wait_idle(&agent, 10).await;

    // 恢复后：用复制出来的同一份日志续接，发同一句话。
    let store = session::SessionStore::open(restored_dir.path(), &id).expect("重开日志");
    let resumed = Agent::resume(
        id.clone(),
        config(restored_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                addition_only_config(&server.uri()),
                test_credential(),
            )
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
    wait_idle(&resumed, 10).await;

    let requests = server.received_requests().await.expect("请求记录");
    assert_eq!(requests.len(), 6, "四步 + 恢复前下一步 + 恢复后同一步");
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    let before_restore: Value = serde_json::from_str(&bodies[4]).expect("恢复前请求");

    // 先确认这一份确实是 addition-only 的投影，不是两边都没投影的空对空相等。
    let query = before_restore["tools"]
        .as_array()
        .expect("tools 数组")
        .iter()
        .find(|tool| tool["name"] == "aios_query")
        .expect("加载过的查询工具在声明里");
    assert_eq!(
        query["defer_loading"], true,
        "加载出来的工具带 defer_loading"
    );
    assert!(
        before_restore.to_string().contains("tool_addition"),
        "历史里有 tool_addition：{before_restore}"
    );
    assert_eq!(
        requests[4]
            .headers
            .get("anthropic-beta")
            .and_then(|value| value.to_str().ok()),
        Some("mid-conversation-tool-changes-2026-07-01"),
        "带工具变更 beta 头"
    );

    assert_eq!(bodies[5], bodies[4], "恢复后同一步的请求与恢复前逐字节相同");
}

/// K1（T014.2 审计 known）：压缩摘要请求与主请求同一工具表/投影；没开出的工具不进摘要请求。
#[tokio::test]
async fn summary_request_uses_the_same_enabled_tool_table() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let agent_config = agent::AgentConfig {
        compaction_headroom_tokens: 1,
        compaction_retain_ratio: 0.000001,
        compaction_retries: 0,
        ..config(data_dir.path())
    };
    let model_config = model::DeepSeekConfig {
        context_window: 1000,
        max_tokens: 100,
        ..addition_only_config(&server.uri())
    };
    let agent = Agent::create(
        agent_config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(model_config, test_credential())
                .expect("模型"),
            tools: tool_set(),
            store,
        },
    )
    .await
    .expect("建 agent");

    agent
        .send(UserMessage::text("EARLY 短消息"), Delivery::FollowUp)
        .await
        .expect("send");
    wait_idle(&agent, 20).await;
    agent
        .send(
            UserMessage::text(format!("LATE {}", "y".repeat(3000))),
            Delivery::FollowUp,
        )
        .await
        .expect("send");
    wait_idle(&agent, 20).await;

    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 3, "两轮请求 + 一次摘要请求");
    let summary = bodies
        .iter()
        .find(|body| body.to_string().contains("compaction engine"))
        .expect("摘要请求");
    assert_eq!(
        tool_names(summary),
        vec!["skill".to_owned()],
        "摘要请求只带本步开出的工具：{summary}"
    );
    assert!(
        !summary.to_string().contains("aios_query"),
        "没开出的查询工具不进摘要请求：{summary}"
    );
    let main = bodies
        .iter()
        .rev()
        .find(|body| !body.to_string().contains("compaction engine"))
        .expect("压缩后的主请求");
    assert_eq!(
        tool_names(summary),
        tool_names(main),
        "摘要请求与主请求同一工具表"
    );
}
