//! T006.5c2：assistant/message 的 DSH stream/usage/interrupted，以及无可见内容 attempt 的
//! assistant/attempt 事件（DSH packages/core/session/src/types.ts:349-363；
//! packages/core/agent-loop/src/agent.ts:480-590；packages/core/agent-loop/src/assistant-stream.ts）。



use std::fs;
use std::path::Path;

use agent::Delivery;
use crate::common::{
    FINAL_TEXT_SSE, PARTIAL_TEXT_SSE, ScriptedSseServer, SequenceResponder, SseScript, setup_agent,
    sse, tool_calls_with_inputs_sse, wait_idle,
};
use model::{AssistantStreamRecord, UserMessage, expand_assistant_stream};
use serde_json::{Value, json};
use session::{SessionId, SessionStore, TurnEndReason};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

/// 两个 text delta 的 SSE：验证 stream 记录保留 delta 边界（不合并成一个文本）。
const TWO_DELTA_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_two","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":7,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#;

/// 第一个 attempt 已经开了 text 块但没流出可见内容就报可重试服务端错误。
const RETRY_ERROR_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_retry","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":3,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: error
data: {"type":"error","error":{"type":"overloaded_error","message":"overloaded"}}

"#;

/// 只写 message_start、没有可见内容，用来测取消时的无表面 attempt。
const MESSAGE_START_ONLY_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_cancel","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":4,"output_tokens":1}}}

"#;

/// 整轮被截断：唯一的块是没写完的 tool_use，裁完为空（T004 K9：DSH 会写空消息保住 usage）。
const TRUNCATED_TOOL_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_trunc","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_0","name":"gate","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#;

fn log_rows(data_dir: &Path, id: &SessionId) -> Vec<Value> {
    let path = data_dir.join("sessions").join(format!("{id}.jsonl"));
    let text = fs::read_to_string(path).unwrap();
    text.lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn event_data<'a>(rows: &'a [Value], event_type: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|row| row["type"] == event_type)
        .map(|row| &row["data"])
        .collect()
}

fn text_of(message: &Value) -> String {
    message["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn reconstructed_delta_text(records: &[AssistantStreamRecord]) -> String {
    expand_assistant_stream(records)
        .into_iter()
        .filter_map(|timed| match timed.chunk {
            model::StreamChunk::TextDelta { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn assistant_message_records_dsh_stream_frames() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(TWO_DELTA_SSE.to_owned())]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    agent
        .send(UserMessage::text("说你好"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 5).await;

    let rows = log_rows(data_dir.path(), &id);
    let messages = event_data(&rows, "assistant/message");
    assert_eq!(messages.len(), 1, "一次成功尝试一条 assistant/message");
    let data = messages[0];
    let stream = data["stream"]
        .as_array()
        .expect("assistant/message 带 stream 数组");

    // 逐帧记录：block-start 原始记录 → text delta 合成一条 text-chunks run（边界不合并）
    // → block-end/usage/finish 原始记录（DSH assistant-stream.ts:20-46）。
    assert_eq!(stream[0]["type"], "chunk");
    assert_eq!(stream[0]["chunk"]["type"], "block-start");
    assert_eq!(stream[0]["chunk"]["blockType"], "text");
    let run = &stream[1];
    assert_eq!(
        run["type"], "text-chunks",
        "text delta 记成 time0/dt/texts 的 run：{run}"
    );
    assert_eq!(run["index"], 0);
    assert_eq!(
        run["texts"],
        json!(["你", "好"]),
        "delta 边界保留，不合并成 \"你好\""
    );
    assert!(
        run["dt"].as_array().is_some_and(|gaps| gaps.len() == 1),
        "两帧之间一个时间间隔：{run}"
    );
    let block_end = stream
        .iter()
        .find(|record| record["chunk"]["type"] == "block-end")
        .expect("block-end 记录");
    assert_eq!(
        block_end["chunk"]["block"],
        json!({"type": "text", "text": "你好"}),
        "block-end 的 block 照 DSH 块 JSON"
    );
    let usage = stream
        .iter()
        .find(|record| record["chunk"]["type"] == "usage")
        .expect("usage 记录");
    assert_eq!(
        usage["chunk"]["usage"],
        json!({"inputTokens": 7, "outputTokens": 2}),
        "usage 记录字段照 DSH TokenUsage"
    );
    let finish = stream
        .iter()
        .find(|record| record["chunk"]["type"] == "finish")
        .expect("finish 记录");
    assert_eq!(finish["chunk"]["reason"], json!({"kind": "stop"}));

    // 记录展开后和消息内容一致：同一批帧组装出消息文本。
    let records: Vec<AssistantStreamRecord> =
        serde_json::from_value(data["stream"].clone()).expect("stream 可反序列化");
    assert_eq!(reconstructed_delta_text(&records), "你好");
    assert_eq!(text_of(&data["message"]), "你好");
    assert!(
        event_data(&rows, "assistant/attempt").is_empty(),
        "成功尝试不写 assistant/attempt"
    );
}

#[tokio::test]
async fn assistant_message_usage_and_interrupted() {
    // 成功：usage 透传、interrupted 省略。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    agent
        .send(UserMessage::text("普通回答"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 5).await;
    let rows = log_rows(data_dir.path(), &id);
    let data = event_data(&rows, "assistant/message")[0];
    assert_eq!(
        data["usage"],
        json!({"inputTokens": 50, "outputTokens": 3}),
        "assistant/message 带 usage"
    );
    assert!(
        data.get("interrupted").is_none(),
        "正常完成不写 interrupted"
    );
    assert!(
        data["stream"]
            .as_array()
            .is_some_and(|stream| !stream.is_empty()),
        "正常完成也带 stream"
    );

    // 取消中途：保留的文本前缀写 interrupted=true 的消息，stream 是已收到的帧。
    let mut server = ScriptedSseServer::start(vec![SseScript {
        frames: PARTIAL_TEXT_SSE.to_owned(),
        hold: true,
    }]);
    let (agent, data_dir, _cwd, id) = setup_agent(&server.base_url(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("开始流"), Delivery::FollowUp)
        .await
        .unwrap();
    server.wait_ready().await;
    let mut streamed = String::new();
    while streamed != "已经流出来的部分文字。" {
        match events.recv().await {
            Ok(agent::AgentEvent::AssistantTextDelta { text }) => streamed.push_str(&text),
            Ok(_) => {}
            Err(error) => panic!("事件通道关闭: {error}"),
        }
    }
    agent.cancel().await.unwrap();
    agent.idle().await;

    let rows = log_rows(data_dir.path(), &id);
    let interrupted = event_data(&rows, "assistant/message")
        .into_iter()
        .find(|data| data["interrupted"] == true)
        .expect("取消中途要写 interrupted=true 的 assistant/message");
    assert_eq!(interrupted["interrupted"], true);
    assert_eq!(text_of(&interrupted["message"]), "已经流出来的部分文字。");
    // usage 只在流结束时由适配器报一次（DSH deepseek/translate.ts:160）；取消发生在
    // 结束帧之前，所以中断消息没有 usage（DSH types.ts:349-352 的「适配器没报就没有」）。
    assert!(
        interrupted.get("usage").is_none(),
        "取消早于结束帧时不写 usage"
    );
    let stream = interrupted["stream"]
        .as_array()
        .expect("interrupted 消息带 stream");
    assert!(!stream.is_empty(), "中断前缀的尝试帧要保留：{stream:?}");
}

#[tokio::test]
async fn attempt_without_surface_goes_to_assistant_attempt() {
    // 可重试的流错误：第一次尝试没有可见内容 → assistant/attempt；重试成功后才有 message。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(RETRY_ERROR_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    agent
        .send(UserMessage::text("重试一下"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let rows = log_rows(data_dir.path(), &id);
    let attempts = event_data(&rows, "assistant/attempt");
    assert_eq!(attempts.len(), 1, "失败的一次尝试落一条 assistant/attempt");
    assert_eq!(attempts[0]["turn"], 1);
    assert_eq!(attempts[0]["step"], 1);
    let attempt_stream = attempts[0]["stream"]
        .as_array()
        .expect("assistant/attempt 带 stream");
    assert!(
        attempt_stream
            .iter()
            .any(|record| record["chunk"]["type"] == "block-start"),
        "attempt 保留已收到的帧：{attempt_stream:?}"
    );
    assert!(
        attempt_stream
            .iter()
            .all(|record| record["chunk"]["type"] != "text-delta"),
        "没有可见文本的 attempt 不该出现 text 帧"
    );
    let messages = event_data(&rows, "assistant/message");
    assert_eq!(messages.len(), 1, "重试成功后只有一条 assistant/message");
    assert_eq!(text_of(&messages[0]["message"]), "工具都跑完了。");
    // attempt 不进派生历史：第二次请求的 messages 里没有任何 assistant 消息。
    let bodies = server.received_requests().await.unwrap();
    assert_eq!(bodies.len(), 2, "失败一次后重试一次");
    let body: Value = serde_json::from_slice(&bodies[1].body).unwrap();
    assert!(
        body["messages"].as_array().is_some_and(|messages| messages
            .iter()
            .all(|message| message["role"] != "assistant")),
        "attempt 不能进派生历史：{body}"
    );

    // 取消且没有可见内容：也落 assistant/attempt（不是空 assistant/message）。
    let mut server = ScriptedSseServer::start(vec![SseScript {
        frames: MESSAGE_START_ONLY_SSE.to_owned(),
        hold: true,
    }]);
    let (agent, data_dir, _cwd, id) = setup_agent(&server.base_url(), ToolSet::empty()).await;
    agent
        .send(UserMessage::text("取消没有输出"), Delivery::FollowUp)
        .await
        .unwrap();
    server.wait_ready().await;
    // 等 driver 真的从流里读第一轮（和上面 cancel_mid_stream 用事件同步不同，这里
    // 没有可见 delta 可等；hold 住连接后 driver 会停在 select 上等下一帧）。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    agent.cancel().await.unwrap();
    agent.idle().await;
    let rows = log_rows(data_dir.path(), &id);
    assert_eq!(
        event_data(&rows, "assistant/attempt").len(),
        1,
        "取消且无可见内容要落 assistant/attempt"
    );
    assert!(
        event_data(&rows, "assistant/message").is_empty(),
        "没有可见内容不能写空 assistant/message"
    );
}

#[tokio::test]
async fn truncated_turn_keeps_usage() {
    let server = ScriptedSseServer::start(vec![SseScript {
        frames: TRUNCATED_TOOL_SSE.to_owned(),
        hold: false,
    }]);
    let (agent, data_dir, _cwd, id) = setup_agent(&server.base_url(), ToolSet::empty()).await;
    agent
        .send(UserMessage::text("截断一个工具调用"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 5).await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![TurnEndReason::MaxTokens],
        "整轮被截断"
    );
    let rows = log_rows(data_dir.path(), &id);
    let messages = event_data(&rows, "assistant/message");
    assert_eq!(
        messages.len(),
        1,
        "截断也要写一条（空 content 的）assistant/message（T004 K9，DSH agent.ts:580-584）"
    );
    assert_eq!(
        messages[0]["message"]["content"],
        json!([]),
        "被截断的 tool_use 裁掉，content 为空"
    );
    assert_eq!(
        messages[0]["usage"],
        json!({"inputTokens": 10, "outputTokens": 2}),
        "usage 不能因为截断而丢"
    );
    assert!(
        messages[0]["stream"]
            .as_array()
            .is_some_and(|stream| !stream.is_empty()),
        "截断尝试的流帧也要留下"
    );
}

/// T006.14（DSH assistant-stream.ts:35-43,129-166；deepseek/translate.ts:82-85,130）：
/// 工具参数增量记成 tool-call-chunks run（id/name 从 block-start 带下来），展开后逐帧还原。
#[tokio::test]
async fn assistant_message_records_tool_call_delta_chunks() {
    let server = MockServer::start().await;
    let tool_call = tool_calls_with_inputs_sse(&[(
        "call_0",
        "read",
        json!({"file_path": "/nonexistent/t006-14"}),
    )]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_call),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    agent
        .send(UserMessage::text("读一个文件"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let rows = log_rows(data_dir.path(), &id);
    let first = event_data(&rows, "assistant/message")
        .into_iter()
        .find(|data| {
            data["message"]["content"]
                .as_array()
                .is_some_and(|content| {
                    content
                        .iter()
                        .any(|block| block["type"] == "tool-call" && block["id"] == "call_0")
                })
        })
        .expect("第一次尝试的 assistant/message 带 tool_use");
    let stream = first["stream"].as_array().expect("stream 数组");
    let runs: Vec<&Value> = stream
        .iter()
        .filter(|record| record["type"] == "tool-call-chunks")
        .collect();
    assert!(
        !runs.is_empty(),
        "工具参数增量要有 tool-call-chunks 记录：{stream:?}"
    );
    assert!(
        runs.iter().all(|run| run["id"] == "call_0"),
        "run 的 id 是 tool_use 的 id：{runs:?}"
    );
    assert_eq!(runs[0]["name"], "read", "首帧带 name（DSH 的起始帧）");
    let args: String = runs
        .iter()
        .flat_map(|run| {
            run["args"]
                .as_array()
                .expect("args 数组")
                .iter()
                .map(|arg| arg.as_str().expect("args 是字符串"))
        })
        .collect();
    assert_eq!(
        args, "{\"file_path\":\"/nonexistent/t006-14\"}",
        "参数增量拼起来就是 block-end 的完整参数"
    );

    // 展开记录后逐帧还原 tool-call-delta（首帧 name + 空参数，之后纯增量）。
    let records: Vec<AssistantStreamRecord> =
        serde_json::from_value(first["stream"].clone()).expect("stream 可反序列化");
    let deltas: Vec<(String, Option<String>, String)> = expand_assistant_stream(&records)
        .into_iter()
        .filter_map(|timed| match timed.chunk {
            model::StreamChunk::ToolCallDelta {
                id,
                name,
                arguments_delta,
                ..
            } => Some((id, name, arguments_delta)),
            _ => None,
        })
        .collect();
    assert!(!deltas.is_empty(), "展开要有 tool-call-delta 帧");
    assert_eq!(deltas[0].1.as_deref(), Some("read"), "起始帧带 name");
    assert_eq!(
        deltas
            .iter()
            .map(|(_, _, arguments)| arguments.as_str())
            .collect::<String>(),
        "{\"file_path\":\"/nonexistent/t006-14\"}"
    );
}
