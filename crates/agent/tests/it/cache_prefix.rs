//! T006.14：派生请求前缀稳定（DSH 靠请求前缀命中 DeepSeek 的前缀缓存；strategy §2 缓存行）。
//!
//! 前缀 = system / tools / 先前的 messages。同一轮工具批之后、下一轮新输入之后，后一次请求的
//! messages 必须以先前请求为前缀、其余顶层字段逐字相同；历史很短时也不能出现额外的摘要请求
//! （摘要请求的前缀复用由 T006.3 的测试覆盖）。

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, setup_agent, sse, tool_calls_with_inputs_sse, wait_idle,
};
use agent::Delivery;
use model::UserMessage;
use serde_json::{Value, json};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

/// 后一次请求以先前请求为前缀：messages 前缀逐字相同，其余顶层字段不变。
fn assert_prefix(earlier: &Value, later: &Value, label: &str) {
    let before = earlier["messages"].as_array().expect("messages 数组");
    let after = later["messages"].as_array().expect("messages 数组");
    assert!(
        before.len() < after.len(),
        "{label}：后一请求要有更长的历史（{} -> {}）",
        before.len(),
        after.len()
    );
    assert_eq!(
        &after[..before.len()],
        before,
        "{label}：messages 前缀必须逐字相同（前缀缓存）"
    );
    for field in [
        "model",
        "stream",
        "max_tokens",
        "thinking",
        "output_config",
        "system",
        "tools",
    ] {
        assert_eq!(
            earlier[field], later[field],
            "{label}：顶层字段 {field} 必须不变"
        );
    }
}

#[tokio::test]
async fn derived_request_prefix_is_stable_across_steps_and_turns() {
    let server = MockServer::start().await;
    let tool_call = tool_calls_with_inputs_sse(&[(
        "call_0",
        "todo_write",
        json!({"todos": [{"content": "step", "status": "pending"}]}),
    )]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_call),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, _data_dir, _cwd, _id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    agent
        .send(UserMessage::text("第一句"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    agent
        .send(UserMessage::text("第二句"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        3,
        "两轮 + 一个工具批 = 3 次普通请求；历史很短时不能有额外摘要请求"
    );
    let bodies: Vec<Value> = requests
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("请求体是 JSON"))
        .collect();
    assert_prefix(&bodies[0], &bodies[1], "同一轮工具批之后");
    assert_prefix(&bodies[1], &bodies[2], "下一轮新输入之后");

    let len0 = bodies[0]["messages"].as_array().unwrap().len();
    let len1 = bodies[1]["messages"].as_array().unwrap().len();
    let len2 = bodies[2]["messages"].as_array().unwrap().len();
    assert_eq!(
        len1,
        len0 + 2,
        "工具批之后正好多 assistant tool_use + tool result 两条"
    );
    assert_eq!(
        len2,
        len1 + 2,
        "下一轮正好多 assistant 回答 + user 新消息两条"
    );
}
