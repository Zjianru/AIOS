//! T014.3 接缝 1（模型层）：对话中途加载工具的线上形状（wiremock 收请求体/请求头）。
//!
//! 期望值来源：DSH `projectToolUpdates`（llm-content.ts:382-470）、`deepseek-serialize.ts:95,165`
//! 与 `deepseek-messages-api.ts:7`：声明了 addition-only 的路由上，加载过的工具在声明里带
//! `defer_loading`，加载处是 system 角色的 `tool_addition` 块，请求头带工具变更 beta。

use std::sync::Arc;

use model::{
    CancelToken, ContentBlock, DeepSeek, DeepSeekConfig, Message, ToolAddition, ToolHistory,
    ToolRequest, ToolSchema, ToolUpdate,
};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::common::test_credential;

/// 一次正常说完的 SSE（用例只看请求，流要能正常收尾）。
const FINAL_TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_final","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":5,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

fn cancel() -> CancelToken {
    CancelToken::new(
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(tokio::sync::Notify::new()),
    )
}

fn schema(name: &str) -> ToolSchema {
    ToolSchema {
        name: name.to_owned(),
        description: format!("{name} 的说明"),
        parameters: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
        defer_loading: false,
    }
}

/// 模型层收到的线上形状（不带 AIOS 自己的 defer 标记）。
fn wire(tool: &ToolSchema) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.parameters,
    })
}

fn config(uri: &str, model_name: &str, tool_update: Option<ToolUpdate>) -> DeepSeekConfig {
    DeepSeekConfig {
        provider: "deepseek".to_owned(),
        base_url: uri.to_owned(),
        model: model_name.to_owned(),
        tool_update,
        ..DeepSeekConfig::default()
    }
}

async fn run(deepseek: &DeepSeek, messages: &[Message], tools: &ToolRequest) {
    let mut stream = deepseek
        .stream(messages, tools, &cancel())
        .await
        .expect("请求要发出去");
    while let Some(chunk) = stream.next().await {
        chunk.expect("流事件");
    }
}

async fn requests(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.expect("请求记录")
}

fn beta_header(request: &Request) -> Option<&str> {
    request
        .headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
}

fn has_tool_addition(body: &Value) -> bool {
    body["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_addition"))
        })
    })
}

/// 票面 1：没声明能力的模型发完整工具表，没有 `defer_loading`、`tool_addition`、beta 头。
#[tokio::test]
async fn undeclared_route_sends_the_complete_tool_table_without_updates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(FINAL_TEXT_SSE.to_owned()))
        .mount(&server)
        .await;
    let deepseek = DeepSeek::with_credentials(
        config(&server.uri(), "mystery-model", None),
        test_credential(),
    )
    .expect("构造");

    let messages = vec![Message::user_text("你好")];
    let anchor = messages.last().expect("有一条消息").id.clone();
    let tools = ToolRequest {
        tools: vec![schema("read"), schema("aios_query")],
        history: Some(ToolHistory {
            baseline: vec![schema("read")],
            updates: vec![ToolAddition {
                after: Some(anchor),
                additions: vec![schema("aios_query")],
            }],
        }),
    };
    run(&deepseek, &messages, &tools).await;

    let requests = requests(&server).await;
    assert_eq!(requests.len(), 1, "一次请求");
    let body: Value = serde_json::from_slice(&requests[0].body).expect("请求体是 JSON");
    let expected = [schema("read"), schema("aios_query")];
    assert_eq!(
        body["tools"],
        json!([wire(&expected[0]), wire(&expected[1])]),
        "完整工具表逐项正确：{body}"
    );
    for tool in body["tools"].as_array().expect("tools 数组") {
        assert!(
            tool.get("defer_loading").is_none(),
            "没声明的路由不带 defer_loading：{tool}"
        );
    }
    assert!(
        !has_tool_addition(&body),
        "没声明的路由不发 tool_addition：{body}"
    );
    assert_eq!(beta_header(&requests[0]), None, "没声明的路由不加 beta 头");
}

/// 票面 2：addition-only 的模型，加载前最后一次请求和加载后第一次请求，前面的消息和工具
/// 声明逐字节相同；后一次的新工具带 `defer_loading`、历史有 `tool_addition`、带 beta 头。
#[tokio::test]
async fn addition_only_route_defers_loaded_tools_and_keeps_the_prefix() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(FINAL_TEXT_SSE.to_owned()))
        .mount(&server)
        .await;
    let deepseek = DeepSeek::with_credentials(
        config(
            &server.uri(),
            "deepseek-flash",
            Some(ToolUpdate::AdditionOnly),
        ),
        test_credential(),
    )
    .expect("构造");

    // 加载前：只有基线工具。
    let first_messages = vec![
        Message::system_prompt("你是 Kind"),
        Message::user_text("开始"),
    ];
    let first_tools = ToolRequest {
        tools: vec![schema("read")],
        history: Some(ToolHistory {
            baseline: vec![schema("read")],
            updates: Vec::new(),
        }),
    };
    run(&deepseek, &first_messages, &first_tools).await;

    // 加载后：历史多了 assistant 的 skill 调用和它的结果，工具表多了 aios_query。
    let mut second_messages = first_messages.clone();
    second_messages.push(Message::assistant(
        vec![ContentBlock::ToolCall {
            id: "call_load".to_owned(),
            name: "skill".to_owned(),
            arguments: "{}".to_owned(),
        }],
        "deepseek",
        "deepseek-flash",
    ));
    let result = Message::tool_result("call_load", "loaded", false);
    let anchor = result.id.clone();
    second_messages.push(result);
    let second_tools = ToolRequest {
        tools: vec![schema("read"), schema("aios_query")],
        history: Some(ToolHistory {
            baseline: vec![schema("read")],
            updates: vec![ToolAddition {
                after: Some(anchor),
                additions: vec![schema("aios_query")],
            }],
        }),
    };
    run(&deepseek, &second_messages, &second_tools).await;

    let requests = requests(&server).await;
    assert_eq!(requests.len(), 2, "加载前后各一次请求");
    let first: Value = serde_json::from_slice(&requests[0].body).expect("第一次请求体");
    let second: Value = serde_json::from_slice(&requests[1].body).expect("第二次请求体");

    let first_wire_messages = first["messages"].as_array().expect("第一次 messages");
    let second_wire_messages = second["messages"].as_array().expect("第二次 messages");
    assert!(
        first_wire_messages.len() < second_wire_messages.len(),
        "加载后历史更长"
    );
    assert_eq!(
        serde_json::to_vec(&second_wire_messages[..first_wire_messages.len()]).expect("序列化"),
        serde_json::to_vec(first_wire_messages).expect("序列化"),
        "前面的消息逐字节相同（前缀缓存）"
    );

    let first_declarations = first["tools"].as_array().expect("第一次 tools");
    let second_declarations = second["tools"].as_array().expect("第二次 tools");
    assert_eq!(
        second_declarations.len(),
        first_declarations.len() + 1,
        "只多出加载出来的那一个工具"
    );
    assert_eq!(
        serde_json::to_vec(&second_declarations[..first_declarations.len()]).expect("序列化"),
        serde_json::to_vec(first_declarations).expect("序列化"),
        "前面的工具声明逐字节相同"
    );
    assert!(
        second_declarations[0].get("defer_loading").is_none(),
        "第一次请求就声明的工具不延迟：{:?}",
        second_declarations[0]
    );
    assert_eq!(second_declarations[1]["name"], "aios_query");
    assert_eq!(
        second_declarations[1]["defer_loading"], true,
        "加载出来的工具带 defer_loading：{:?}",
        second_declarations[1]
    );

    let addition_index = second_wire_messages
        .iter()
        .position(|message| {
            message["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_addition"))
        })
        .expect("历史里有 tool_addition");
    assert_eq!(second_wire_messages[addition_index]["role"], "system");
    assert_eq!(
        second_wire_messages[addition_index]["content"][0],
        json!({"type": "tool_addition", "tool": {"type": "tool_reference", "name": "aios_query"}}),
        "加载处是一条 system 角色的 tool_addition"
    );
    assert_eq!(
        second_wire_messages[addition_index - 1]["content"][0]["type"],
        "tool_result",
        "tool_addition 紧跟在加载它的 tool_result 之后"
    );
    assert!(!has_tool_addition(&first), "加载前没有 tool_addition");

    assert_eq!(
        beta_header(&requests[0]),
        None,
        "加载前（没有 tool_addition 块）不加 beta 头"
    );
    assert_eq!(
        beta_header(&requests[1]),
        Some("mid-conversation-tool-changes-2026-07-01"),
        "有 tool_addition 块时加工具变更 beta 头"
    );
}

fn sse(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}
