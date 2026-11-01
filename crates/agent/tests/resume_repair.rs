//! 接缝 1：续接与中断修复（测试清单第 7、8 条）。

mod common;

use std::path::{Path, PathBuf};

use agent::{Agent, Delivery, Deps};
use common::{
    FINAL_TEXT_SSE, SequenceResponder, ToolEvent, gated, model_config, sse, tool_calls_sse,
    wait_idle,
};
use model::{ContentBlock, DeepSeek, UserMessage};
use session::{EventKind, SessionId, SessionStore, TurnEndReason};
use tokio::sync::mpsc;
use tools::{ToolConcurrency, ToolSet};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

fn log_path(data_dir: &Path, id: &SessionId) -> PathBuf {
    data_dir.join("sessions").join(format!("{id}.jsonl"))
}

async fn mount_sequence(server: &MockServer, responses: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            responses.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

async fn create_agent(
    data_dir: &Path,
    cwd: &Path,
    base_url: &str,
    tools: ToolSet,
) -> (Agent, SessionId) {
    let store = SessionStore::create(data_dir, cwd).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        common::isolated_config(data_dir),
        Deps {
            model: DeepSeek::new(model_config(base_url)).unwrap(),
            tools,
            store,
        },
    )
    .await
    .unwrap();
    (agent, id)
}

async fn resume_agent(data_dir: &Path, id: &SessionId, base_url: &str, tools: ToolSet) -> Agent {
    let store = SessionStore::open(data_dir, id).unwrap();
    Agent::resume(
        id.clone(),
        common::isolated_config(data_dir),
        Deps {
            model: DeepSeek::new(model_config(base_url)).unwrap(),
            tools,
            store,
        },
    )
    .await
    .unwrap()
}

fn count_turns(events: &[session::Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::TurnStart { .. }))
        .count()
}

fn request_body(request: &wiremock::Request) -> serde_json::Value {
    serde_json::from_slice(&request.body).expect("请求体是 JSON")
}

/// 请求里所有 user/tool 消息上的 text 块内容。
fn user_texts(body: &serde_json::Value) -> Vec<String> {
    let mut texts = Vec::new();
    if let Some(messages) = body["messages"].as_array() {
        for message in messages {
            if message["role"] != "user" {
                continue;
            }
            if let Some(content) = message["content"].as_array() {
                for block in content {
                    if block["type"] == "text"
                        && let Some(text) = block["text"].as_str()
                    {
                        texts.push(text.to_owned());
                    }
                }
            }
        }
    }
    texts
}

/// 请求里是否带指定 id 的工具结果，且内容包含 needled。
fn has_tool_result(body: &serde_json::Value, call_id: &str, needle: &str) -> bool {
    let Some(messages) = body["messages"].as_array() else {
        return false;
    };
    messages.iter().any(|message| {
        message["content"].as_array().is_some_and(|content| {
            content.iter().any(|block| {
                block["type"] == "tool_result"
                    && block["tool_use_id"] == call_id
                    && wire_text(&block["content"]).is_some_and(|text| text.contains(needle))
            })
        })
    })
}

/// wire 上的 tool_result content 是文本块数组（有时也可能直接是字符串）。
fn wire_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(blocks) => Some(
            blocks
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect(),
        ),
        _ => None,
    }
}

/// 日志里的中断修复收尾：synthetic tool/result、step/end、turn/end。
fn repair_tail(events: &[session::Event]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event.kind.type_name())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn find_tool_result<'a>(
    events: &'a [session::Event],
    call_id: &str,
) -> Option<(&'a str, Option<&'a session::ToolError>)> {
    for event in events {
        if let EventKind::ToolResult { message, error, .. } = &event.kind {
            for block in &message.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                    && tool_use_id == call_id
                {
                    return Some((content, error.as_ref()));
                }
            }
        }
    }
    None
}

#[tokio::test]
async fn resume_after_balanced_turn_sees_full_history() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            tool_calls_sse(&[("call_0", "gate")]),
            FINAL_TEXT_SSE.to_owned(),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri(), tools).await;
    agent
        .send(UserMessage::text("跑一个工具"), Delivery::FollowUp)
        .await
        .unwrap();
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned()))
    );
    permits[0].1.add_permits(1);
    wait_idle(&agent, 3).await;
    drop(agent);

    // 平衡日志：续接不应追加任何修复事件。
    let balanced = SessionStore::open(data_dir.path(), &id).unwrap();
    let balanced_events = balanced.events().len();
    assert!(
        balanced
            .events()
            .iter()
            .any(|event| matches!(event.kind, EventKind::TurnEnd { .. })),
        "正常跑完的一轮应当有 turn/end"
    );
    assert_eq!(
        balanced
            .events()
            .iter()
            .filter(|event| matches!(
                event.kind,
                EventKind::TurnEnd {
                    reason: TurnEndReason::Interrupted,
                    ..
                }
            ))
            .count(),
        0,
        "平衡日志不该有 interrupted 收尾"
    );
    drop(balanced);

    let resumed = resume_agent(data_dir.path(), &id, &server.uri(), ToolSet::empty()).await;
    resumed
        .send(UserMessage::text("续接后第一句"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 3).await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3, "续接后只该发起一次新请求");
    let body = request_body(&requests[2]);
    assert!(
        body["system"]
            .as_str()
            .is_some_and(|system| system.contains("You are AIOS")),
        "续接后的请求还带系统提示：{body}"
    );
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.first().unwrap()["role"], "user");
    assert!(
        messages.iter().any(|message| message["role"] == "assistant"
            && message["content"].as_array().is_some_and(|content| content
                .iter()
                .any(|block| block["type"] == "tool_use" && block["id"] == "call_0"))),
        "续接后的请求要带之前的 assistant tool_use：{body}"
    );
    assert!(
        has_tool_result(&body, "call_0", "ok:gate"),
        "续接后的请求要带之前的工具结果：{body}"
    );
    assert!(
        user_texts(&body).contains(&"续接后第一句".to_owned()),
        "续接后的请求要带新输入"
    );

    let reopened = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(count_turns(reopened.events()), 2);
    assert_eq!(
        reopened.events().len(),
        balanced_events + 6,
        "平衡日志续接只多一轮的 6 个事件（step/start、user/message、assistant/message、step/end、turn/end + turn/start）"
    );
}

#[tokio::test]
async fn resume_repairs_unknown_call_from_torn_tail() {
    // 原始会话：mock 返回一次工具调用，门控工具卡住；崩溃点 = tool/call 已写、tool/result 未写。
    let server = MockServer::start().await;
    mount_sequence(&server, vec![tool_calls_sse(&[("call_0", "gate")])]).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri(), tools).await;
    agent
        .send(UserMessage::text("跑工具"), Delivery::FollowUp)
        .await
        .unwrap();
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned()))
    );
    let crashed_prefix = std::fs::read(log_path(data_dir.path(), &id)).unwrap();
    drop(agent);

    // 把崩溃前缀抄到新 data-dir，并追加半行（模拟掉电时最后一行只写了一半）。
    let crashed_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(crashed_dir.path().join("sessions")).unwrap();
    let mut torn = crashed_prefix;
    torn.extend_from_slice(b"{\"seq\":99,\"time\":1,\"type\":\"tool/");
    std::fs::write(log_path(crashed_dir.path(), &id), &torn).unwrap();

    // 续接用的 mock：修复后的新请求拿一条正常回答。
    let resumed_server = MockServer::start().await;
    mount_sequence(&resumed_server, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let resumed = resume_agent(
        crashed_dir.path(),
        &id,
        &resumed_server.uri(),
        ToolSet::empty(),
    )
    .await;

    // 修复事件：tool/result（UNKNOWN）+ step/end + turn/end（interrupted）。
    let repaired = SessionStore::open(crashed_dir.path(), &id).unwrap();
    let repaired_events = repaired.events();
    assert_eq!(
        repair_tail(repaired_events),
        vec!["tool/result", "step/end", "turn/end"]
    );
    // 合成事件复用最后一个真实事件的时间（DSH repair.ts:88-90）。
    let tail_len = repair_tail(repaired_events).len();
    let last_real = &repaired_events[repaired_events.len() - tail_len - 1];
    assert!(
        repaired_events[repaired_events.len() - tail_len..]
            .iter()
            .all(|event| event.time == last_real.time),
        "修复事件要复用最后一个真实事件的时间"
    );
    let (text, code) = find_tool_result(repaired_events, "call_0").expect("要补出 call_0 的结果");
    assert_eq!(
        code.map(|error| (error.name.as_str(), error.code.as_str())),
        Some(("ToolOutcomeUnknownError", "TOOL_OUTCOME_UNKNOWN"))
    );
    assert!(
        text.contains("outcome is unknown"),
        "模型可见文案要说明结果未知：{text}"
    );
    assert_eq!(
        repaired
            .events()
            .iter()
            .filter(|event| matches!(
                event.kind,
                EventKind::TurnEnd {
                    reason: TurnEndReason::Interrupted,
                    ..
                }
            ))
            .count(),
        1,
        "崩溃收尾的 turn/end 用 interrupted（DSH repair.ts:96）"
    );
    assert_eq!(count_turns(repaired.events()), 1, "修复不应重开一轮");
    drop(repaired);

    // 工具没有被重跑：原工具通道没有第二个 Started。
    assert!(started.try_recv().is_err(), "工具不能被重跑");

    // 修复后能继续，请求里带合成结果和新输入。
    resumed
        .send(UserMessage::text("修复后继续"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 3).await;
    let requests = resumed_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = request_body(&requests[0]);
    assert!(
        has_tool_result(&body, "call_0", "outcome is unknown"),
        "修复后的请求要带合成的工具结果：{body}"
    );
    assert!(user_texts(&body).contains(&"修复后继续".to_owned()));
}

#[tokio::test]
async fn resume_repairs_not_started_call_after_assistant_message() {
    // 原始会话完整跑完一轮（工具开始且有结果），用于拿到合法的平衡日志。
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            tool_calls_sse(&[("call_0", "gate")]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri(), tools).await;
    agent
        .send(UserMessage::text("跑工具"), Delivery::FollowUp)
        .await
        .unwrap();
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned()))
    );
    permits[0].1.add_permits(1);
    wait_idle(&agent, 3).await;
    drop(agent);

    // 崩溃点 = assistant/message 写完、tool/call 还没写；截到 assistant 那一行为止，再补半行。
    let log = std::fs::read_to_string(log_path(data_dir.path(), &id)).unwrap();
    let lines: Vec<&str> = log.trim_end_matches('\n').split('\n').collect();
    let assistant_line = lines
        .iter()
        .position(|line| line.contains("\"type\":\"assistant/message\""))
        .expect("日志里要有 assistant/message");
    let mut torn = lines[..=assistant_line].join("\n").into_bytes();
    torn.push(b'\n');
    torn.extend_from_slice(b"{\"seq\":99,\"time\":1,\"type\":\"tool/");

    let crashed_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(crashed_dir.path().join("sessions")).unwrap();
    std::fs::write(log_path(crashed_dir.path(), &id), &torn).unwrap();

    let resumed_server = MockServer::start().await;
    mount_sequence(&resumed_server, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let resumed = resume_agent(
        crashed_dir.path(),
        &id,
        &resumed_server.uri(),
        ToolSet::empty(),
    )
    .await;

    let repaired = SessionStore::open(crashed_dir.path(), &id).unwrap();
    assert_eq!(
        repair_tail(repaired.events()),
        vec!["tool/result", "step/end", "turn/end"]
    );
    let repaired_events = repaired.events();
    let tail_len = repair_tail(repaired_events).len();
    let last_real = &repaired_events[repaired_events.len() - tail_len - 1];
    assert!(
        repaired_events[repaired_events.len() - tail_len..]
            .iter()
            .all(|event| event.time == last_real.time),
        "修复事件要复用最后一个真实事件的时间"
    );
    let (text, code) = find_tool_result(repaired_events, "call_0").expect("要补出 call_0 的结果");
    assert_eq!(
        code.map(|error| (error.name.as_str(), error.code.as_str())),
        Some(("ToolNotStartedError", "TOOL_NOT_STARTED"))
    );
    assert!(
        text.contains("Retry it if it is still needed."),
        "模型可见文案要说明可以重试：{text}"
    );
    assert_eq!(
        repaired
            .events()
            .iter()
            .filter(|event| matches!(
                event.kind,
                EventKind::TurnEnd {
                    reason: TurnEndReason::Interrupted,
                    ..
                }
            ))
            .count(),
        1,
        "崩溃收尾的 turn/end 用 interrupted（DSH repair.ts:96）"
    );
    drop(repaired);

    resumed
        .send(UserMessage::text("修复后继续"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 3).await;
    let requests = resumed_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(has_tool_result(
        &request_body(&requests[0]),
        "call_0",
        "Retry it if it is still needed."
    ));
}
