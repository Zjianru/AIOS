//! T013.1：tool/result 的 meta 只落会话日志，投给模型的请求体里没有 meta（清单 2 末句）。

use agent::Delivery;
use model::UserMessage;
use serde_json::json;
use tools::ToolSet;

use crate::common::{
    FINAL_TEXT_SSE, ScriptedSseServer, SseScript, setup_agent_custom, tool_calls_with_inputs_sse,
};

#[tokio::test]
async fn model_requests_do_not_carry_tool_result_meta() {
    let server = ScriptedSseServer::start(vec![
        SseScript {
            frames: tool_calls_with_inputs_sse(&[(
                "call_0",
                "write",
                json!({"file_path": "a.txt", "content": "hello\n"}),
            )]),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let cwd = tempfile::tempdir().unwrap();
    let (agent, _data_dir, _id) = setup_agent_custom(
        &server.base_url(),
        ToolSet::builtin_with_patch(),
        cwd.path(),
        Vec::new(),
    )
    .await;
    agent
        .send(UserMessage::text("写一个文件"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let bodies = server.request_bodies();
    assert_eq!(
        bodies.len(),
        2,
        "两轮请求：第一轮带工具调用，第二轮带工具结果"
    );
    let second = String::from_utf8_lossy(&bodies[1]).to_string();
    assert!(
        !second.contains("\"meta\""),
        "投给模型的请求体里不能有 meta：{second}"
    );
    // 工具结果本身在：确认请求体里确实带了 call_0 的结果（否则上面的断言是空转）。
    assert!(
        second.contains("call_0"),
        "第二轮的请求体应包含工具结果：{second}"
    );
}
