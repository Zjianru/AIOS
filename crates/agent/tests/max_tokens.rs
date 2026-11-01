//! 接缝 1：输出截断（测试清单第 12 条 + 票面加项 D3）。

mod common;

use agent::Delivery;
use common::{
    FINAL_TEXT_SSE, PhaseScript, PhaseSseServer, ScriptedSseServer, SseScript, ToolEvent, gated,
    setup_agent, wait_idle,
};
use model::{ContentBlock, UserMessage};
use session::{EventKind, SessionStore, TurnEndReason};
use tokio::sync::mpsc;
use tools::{ToolConcurrency, ToolSet};

type ToolPermits = Vec<(&'static str, std::sync::Arc<tokio::sync::Semaphore>)>;

fn gated_tools() -> (ToolSet, mpsc::UnboundedReceiver<ToolEvent>, ToolPermits) {
    let (started_tx, started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    (tools, started, permits)
}

fn count_turns(store: &SessionStore) -> usize {
    store
        .events()
        .iter()
        .filter(|event| matches!(event.kind, EventKind::TurnStart { .. }))
        .count()
}

fn count_steps(store: &SessionStore) -> usize {
    store
        .events()
        .iter()
        .filter(|event| matches!(event.kind, EventKind::StepStart { .. }))
        .count()
}

fn turn_reasons(store: &SessionStore) -> Vec<TurnEndReason> {
    store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn assistant_blocks(store: &SessionStore) -> Vec<Vec<ContentBlock>> {
    store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::AssistantMessage { message, .. } => Some(message.content.clone()),
            _ => None,
        })
        .collect()
}

fn has_tool_events(store: &SessionStore) -> bool {
    store.events().iter().any(|event| {
        matches!(
            event.kind,
            EventKind::ToolCall { .. } | EventKind::ToolResult { .. }
        )
    })
}

fn has_tool_use_block(store: &SessionStore) -> bool {
    assistant_blocks(store)
        .iter()
        .flatten()
        .any(|block| matches!(block, ContentBlock::ToolCall { .. }))
}

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

const TEXT_THEN_TRUNCATED_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_trunc2","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"半截文本"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_0","name":"gate","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#;

#[tokio::test]
async fn max_tokens_does_not_execute_or_log_truncated_tool_call() {
    let server = ScriptedSseServer::start(vec![
        SseScript {
            frames: TRUNCATED_TOOL_SSE.to_owned(),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let (tools, mut started, _permits) = gated_tools();
    let (agent, data_dir, _cwd, id) = setup_agent(&server.base_url(), tools).await;
    agent.send(UserMessage::text("截断一个工具调用"), Delivery::FollowUp);
    wait_idle(&agent, 5).await;

    assert!(started.try_recv().is_err(), "被截断的工具调用不能执行");
    let store = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(count_turns(&store), 1);
    assert_eq!(turn_reasons(&store), vec![TurnEndReason::MaxTokens]);
    assert!(
        !has_tool_events(&store),
        "日志里不能有 tool/call / tool/result"
    );
    assert!(
        !has_tool_use_block(&store),
        "assistant/message 里不能有截断的 tool_use"
    );
    assert!(
        assistant_blocks(&store).is_empty(),
        "裁完为空不写 assistant/message（D1）"
    );
    assert_eq!(server.request_bodies().len(), 1);

    // 下一轮请求里不能出现不配对的 tool_use（协议安全）。
    agent.send(UserMessage::text("继续"), Delivery::FollowUp);
    wait_idle(&agent, 5).await;
    let store = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(count_turns(&store), 2);
    assert_eq!(
        turn_reasons(&store),
        vec![TurnEndReason::MaxTokens, TurnEndReason::Completed]
    );
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2);
    let body = String::from_utf8(bodies[1].clone()).unwrap();
    assert!(
        !body.contains("tool_use"),
        "下一次请求不能带不配对 tool_use：{body}"
    );
}

#[tokio::test]
async fn max_tokens_keeps_text_but_drops_truncated_tool_call() {
    let server = ScriptedSseServer::start(vec![
        SseScript {
            frames: TEXT_THEN_TRUNCATED_SSE.to_owned(),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let (tools, mut started, _permits) = gated_tools();
    let (agent, data_dir, _cwd, id) = setup_agent(&server.base_url(), tools).await;
    agent.send(UserMessage::text("先说半截再截断工具"), Delivery::FollowUp);
    wait_idle(&agent, 5).await;

    assert!(started.try_recv().is_err(), "工具不能执行");
    let store = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(turn_reasons(&store), vec![TurnEndReason::MaxTokens]);
    assert!(!has_tool_events(&store));
    assert_eq!(
        assistant_blocks(&store),
        vec![vec![ContentBlock::Text {
            text: "半截文本".to_owned()
        }]],
        "text 保留、截断的 tool-call 块丢弃"
    );

    agent.send(UserMessage::text("继续"), Delivery::FollowUp);
    wait_idle(&agent, 5).await;
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2);
    let body = String::from_utf8(bodies[1].clone()).unwrap();
    assert!(body.contains("半截文本"), "回放要带截断前的 text");
    assert!(!body.contains("tool_use"), "回放不能带截断的 tool_use");
}

const STEP1_PREFIX: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_step1","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_0","name":"gate","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}

"#;

const STEP1_SUFFIX: &str = r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#;

#[tokio::test]
async fn max_tokens_stays_sticky_across_later_step() {
    let mut server = PhaseSseServer::start(vec![
        PhaseScript {
            prefix: STEP1_PREFIX.to_owned(),
            suffix: STEP1_SUFFIX.to_owned(),
            hold: true,
        },
        PhaseScript {
            prefix: FINAL_TEXT_SSE.to_owned(),
            suffix: String::new(),
            hold: false,
        },
    ]);
    let (tools, mut started, _permits) = gated_tools();
    let (agent, data_dir, _cwd, id) = setup_agent(&server.base_url(), tools).await;
    agent.send(UserMessage::text("截断这一轮"), Delivery::FollowUp);
    // Step1 已经把前缀写出来、还没结算；此时插一句 steer，进 next-step。
    server.wait_ready().await;
    agent.send(UserMessage::text("再补一句"), Delivery::Steer);
    server.release();
    wait_idle(&agent, 10).await;

    assert!(started.try_recv().is_err(), "工具不能执行");
    let store = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(count_turns(&store), 1, "steer 不另开一轮");
    assert_eq!(
        turn_reasons(&store),
        vec![TurnEndReason::MaxTokens],
        "max-tokens 不被后面的 completed step 降级"
    );
    assert_eq!(count_steps(&store), 2, "有第二个 step");
    assert!(
        store.events().iter().any(|event| matches!(
            &event.kind,
            EventKind::UserMessage { message, .. }
                if message
                    .content
                    .iter()
                    .any(|block| block.as_text() == Some("再补一句"))
        )),
        "第二个 step 领取了 steer"
    );
    assert!(!has_tool_events(&store));
    assert!(!has_tool_use_block(&store));
    assert_eq!(server.request_bodies().len(), 2);
}
