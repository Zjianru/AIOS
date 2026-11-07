//! T015.2 接缝 2（agent，假模型回放）：画一版，关掉重开，再用 edits 改，成功（清单 4，故事 14、15）。
//! T015.3（清单 5，故事 15）：压缩时画布旧版只留一句（tool/result 和 html/edits 参数都剪），
//! 最新版保留全文；不压缩时请求前缀逐字稳定；压缩后重开还能改最新版。

use std::path::Path;

use agent::{Agent, AgentConfig, Delivery, Deps};
use model::{DeepSeek, UserMessage};
use serde_json::{Value, json};
use session::{EventKind, SessionId, SessionStore, SurfaceOp};
use tools::{CanvasTool, Tool, ToolCall, ToolContext, ToolFuture, ToolResult, ToolSet};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use crate::common::{
    FINAL_TEXT_SSE, ScriptedSseServer, SequenceResponder, SseScript, isolated_config, model_config,
    sse, test_credential, tool_calls_with_inputs_sse, wait_idle,
};

async fn create_agent(data_dir: &Path, cwd: &Path, base_url: &str) -> (Agent, SessionId) {
    let store = SessionStore::create(data_dir, cwd).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        isolated_config(data_dir),
        Deps {
            model: DeepSeek::with_credentials(model_config(base_url), test_credential()).unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    (agent, id)
}

async fn resume_agent(data_dir: &Path, id: &SessionId, base_url: &str) -> Agent {
    let store = SessionStore::open(data_dir, id).unwrap();
    Agent::resume(
        id.clone(),
        isolated_config(data_dir),
        Deps {
            model: DeepSeek::with_credentials(model_config(base_url), test_credential()).unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap()
}

/// 日志里每个画布版本的 meta.canvas（按出现顺序）。
fn canvases(events: &[session::Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolResult {
                meta: Some(meta), ..
            } => meta.get("canvas").cloned(),
            _ => None,
        })
        .collect()
}

/// 一条 tool/result 给模型的文字（只取 text 块）。
fn tool_text(events: &[session::Event], call_id: &str) -> String {
    for event in events {
        if let EventKind::ToolResult { message, .. } = &event.kind
            && message.tool_call_id.as_deref() == Some(call_id)
        {
            return message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
        }
    }
    String::new()
}

#[tokio::test]
async fn resume_rehydrates_the_canvas_and_the_next_edit_succeeds() {
    // 第 1 轮：画一版（工具调用 + 一句回答）。
    let first = ScriptedSseServer::start(vec![
        SseScript {
            frames: tool_calls_with_inputs_sse(&[(
                "call_create",
                "canvas",
                json!({
                    "title": "每月存钱算一算",
                    "note": "第一版",
                    "html": "<div id=\"v\">1</div>",
                }),
            )]),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &first.base_url()).await;
    agent
        .send(UserMessage::text("画一个存钱的画布"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 5).await;
    drop(agent);

    // 关掉重开之前：日志里只有第 1 版，id 从日志取（真实模型也是从工具结果里拿到它）。
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let created = canvases(store.events());
    drop(store);
    assert_eq!(created.len(), 1, "第 1 轮只该有一个画布结果");
    assert_eq!(created[0]["n"], json!(1));
    let canvas_id = created[0]["id"].as_str().unwrap().to_owned();

    // 重开会话，第 2 轮用 edits 改同一块画布（id 来自日志里的那一版）。
    let second = ScriptedSseServer::start(vec![
        SseScript {
            frames: tool_calls_with_inputs_sse(&[(
                "call_edit",
                "canvas",
                json!({
                    "id": canvas_id,
                    "title": "每月存钱算一算",
                    "note": "第二版：换成折线",
                    "edits": [{"old": "<div id=\"v\">1</div>", "new": "<div id=\"v\">2</div>"}],
                }),
            )]),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let resumed = resume_agent(data_dir.path(), &id, &second.base_url()).await;
    resumed
        .send(UserMessage::text("把柱子换成折线"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 5).await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let events = store.events().to_vec();
    drop(store);
    let done = canvases(&events);
    assert_eq!(done.len(), 2, "重开后这次该出第 2 版");
    assert_eq!(done[1]["id"], json!(canvas_id), "改的是同一个画布");
    assert_eq!(done[1]["n"], json!(2));
    assert_eq!(done[1]["html"], json!("<div id=\"v\">2</div>"));
    assert_eq!(
        tool_text(&events, "call_edit"),
        format!("画布「每月存钱算一算」第 2 版已显示（{canvas_id}）")
    );
}

// ---- T015.3：压缩时旧版只剩一句 ------------------------------------------------

/// 画布标题固定；每个版本的 note 和正文由用例给。
const CANVAS_TITLE: &str = "每月存钱算一算";

fn sentence(n: u64, note: &str) -> String {
    format!("画布「{CANVAS_TITLE}」第 {n} 版：{note}")
}

/// 第 n 版正文（局部改只是把 id 里的数字换掉，所以第 2 版就是 page(1) 改出来的）。
fn page(n: u64) -> String {
    format!(
        "<div id=\"v\">{n}</div>{}",
        "<!-- 余量：柱子和说明 -->".repeat(30)
    )
}

/// 跟画布调用同一批的结果工具：chars 定结果长度。大的撑上下文，小的当同批兄弟结果。
struct BigTool {
    parameters: Value,
}

impl BigTool {
    fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {"chars": {"type": "integer"}},
                "required": ["chars"]
            }),
        }
    }
}

impl Tool for BigTool {
    fn name(&self) -> &str {
        "big"
    }

    fn description(&self) -> &str {
        "test-only big output tool"
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let chars = serde_json::from_str::<Value>(&call.arguments)
                .ok()
                .and_then(|value| value["chars"].as_u64())
                .unwrap_or(0);
            ToolResult::success(call.id, "x".repeat(usize::try_from(chars).unwrap_or(0)))
        })
    }
}

/// 压缩测试用的窗口：够小，走一次真实的自动压缩事务；保留比例要保住最新版和触发这一轮。
const COMPACTION_WINDOW: u64 = 4_750;

fn compaction_config(data_dir: &Path) -> AgentConfig {
    AgentConfig {
        compaction_retain_ratio: 0.43,
        compaction_retries: 0,
        compaction_headroom_tokens: 1,
        ..isolated_config(data_dir)
    }
}

fn compaction_model_config(base_url: &str) -> model::DeepSeekConfig {
    model::DeepSeekConfig {
        context_window: COMPACTION_WINDOW,
        max_tokens: 100,
        ..model_config(base_url)
    }
}

/// 一次「v1 整份写、v2 局部改、v3 再整份写并跑大结果 → 下一步前自动压缩」的会话。
/// 服务脚本还留着第 4 轮（重开后再局部改一次），给 after_compaction_resume_can_edit_the_latest_canvas 用。
struct CompactionScenario {
    data_dir: tempfile::TempDir,
    /// 会话的 cwd 目录；只用来在临时目录被回收前保持路径有效。
    _cwd: tempfile::TempDir,
    id: SessionId,
    server: MockServer,
    requests: Vec<Value>,
    events: Vec<session::Event>,
}

async fn run_compaction_scenario() -> CompactionScenario {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            // 第 1 轮：大背景 + 画布第 1 版；同一步再跑一个小结果（同批兄弟）。
            sse(tool_calls_with_inputs_sse(&[
                (
                    "call_v1",
                    "canvas",
                    json!({"title": CANVAS_TITLE, "note": "第一版", "html": page(1)}),
                ),
                ("call_small", "big", json!({"chars": 200})),
            ])),
            sse(FINAL_TEXT_SSE.to_owned()),
            // 第 2 轮：用 edits 出第 2 版。
            sse(tool_calls_with_inputs_sse(&[(
                "call_v2",
                "canvas",
                json!({
                    "id": "cv_1",
                    "title": CANVAS_TITLE,
                    "note": "第二版",
                    "edits": [{"old": "<div id=\"v\">1</div>", "new": "<div id=\"v\">2</div>"}],
                }),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
            // 第 3 轮：整份重写出第 3 版，并跑一个很大的工具结果。
            sse(tool_calls_with_inputs_sse(&[
                (
                    "call_v3",
                    "canvas",
                    json!({"id": "cv_1", "title": CANVAS_TITLE, "note": "第三版", "html": page(3)}),
                ),
                ("call_big", "big", json!({"chars": 16_000})),
            ])),
            // 第 3 轮下一步之前的压缩摘要；之后才是这一步的请求。
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            // 第 4 轮（重开以后）：用 edits 改最新版。
            sse(tool_calls_with_inputs_sse(&[(
                "call_v4",
                "canvas",
                json!({
                    "id": "cv_1",
                    "title": CANVAS_TITLE,
                    "note": "第四版",
                    "edits": [{"old": "<div id=\"v\">3</div>", "new": "<div id=\"v\">4</div>"}],
                }),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        compaction_config(data_dir.path()),
        Deps {
            model: DeepSeek::with_credentials(
                compaction_model_config(&server.uri()),
                test_credential(),
            )
            .unwrap(),
            tools: ToolSet::new(vec![
                Box::new(CanvasTool::new()) as Box<dyn Tool>,
                Box::new(BigTool::new()),
            ]),
            store,
        },
    )
    .await
    .unwrap();
    // 大背景放在第 1 轮用户消息里：后面压缩掉它，画布那几轮留在 tail。
    let filler = format!("画一个存钱的画布。{}", "背景说明。".repeat(1_000));
    agent
        .send(UserMessage::text(filler), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    agent
        .send(UserMessage::text("用 edits 换成折线"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    agent
        .send(
            UserMessage::text("整份重画第 3 版，再跑个大数据工具"),
            Delivery::FollowUp,
        )
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    let events = {
        let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
        store.events().to_vec()
    };
    let requests = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("请求体是 JSON"))
        .collect();
    drop(agent);
    CompactionScenario {
        data_dir,
        _cwd: cwd,
        id,
        server,
        requests,
        events,
    }
}

/// 一条消息给模型的纯文本（只取 text 块）。
fn message_text(message: &model::Message) -> String {
    message
        .content
        .iter()
        .filter_map(model::ContentBlock::as_text)
        .collect()
}

/// 请求体里所有 canvas 调用的 input（wire 的 tool_use 块）。
fn canvas_inputs(body: &Value) -> Vec<Value> {
    let mut inputs = Vec::new();
    let Some(messages) = body["messages"].as_array() else {
        return inputs;
    };
    for message in messages {
        let Some(content) = message["content"].as_array() else {
            continue;
        };
        for block in content {
            if block["type"] == json!("tool_use") && block["name"] == json!("canvas") {
                inputs.push(block["input"].clone());
            }
        }
    }
    inputs
}

/// 请求体里所有 tool_result 的（tool_use_id，文字）（wire 的 tool_result 块）。
fn request_tool_results_with_ids(body: &Value) -> Vec<(String, String)> {
    let mut results = Vec::new();
    let Some(messages) = body["messages"].as_array() else {
        return results;
    };
    for message in messages {
        let Some(content) = message["content"].as_array() else {
            continue;
        };
        for block in content {
            if block["type"] != json!("tool_result") {
                continue;
            }
            let mut text = String::new();
            if let Some(parts) = block["content"].as_array() {
                for part in parts {
                    if part["type"] == json!("text") {
                        text.push_str(part["text"].as_str().unwrap_or_default());
                    }
                }
            }
            results.push((
                block["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                text,
            ));
        }
    }
    results
}

/// 请求体里所有 tool_result 的文字。
fn request_tool_results(body: &Value) -> Vec<String> {
    request_tool_results_with_ids(body)
        .into_iter()
        .map(|(_, text)| text)
        .collect()
}

/// 日志里某个 seq 的结果替换事件。
fn replacement_of(events: &[session::Event], seq: u64) -> &session::Event {
    events
        .iter()
        .find(|event| {
            event.surface_op
                == Some(SurfaceOp::Replace {
                    start_seq: seq,
                    end_seq: seq,
                })
        })
        .expect("被剪的节点应有 surfaceOp replace 的替换事件")
}

/// 一条 assistant/message 里某个调用的 arguments 原文。
fn call_arguments(event: &session::Event, call_id: &str) -> String {
    let EventKind::AssistantMessage { message, .. } = &event.kind else {
        panic!("不是 assistant/message");
    };
    message
        .content
        .iter()
        .find_map(|block| match block.as_tool_call() {
            Some((id, _, arguments)) if id == call_id => Some(arguments.to_owned()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("消息里没有调用 {call_id}"))
}

#[tokio::test]
async fn compaction_keeps_one_sentence_for_each_old_canvas_version_and_the_latest_full_text() {
    let scenario = run_compaction_scenario().await;
    let bodies = &scenario.requests;
    let summary_index = bodies
        .iter()
        .position(|body| body.to_string().contains("compaction engine"))
        .expect("撑过窗口后应当有一次自动压缩摘要请求");
    let after = bodies
        .get(summary_index + 1)
        .expect("压缩摘要之后要发派生请求");
    let first = sentence(1, "第一版");
    let second = sentence(2, "第二版");
    let latest = format!("画布「{CANVAS_TITLE}」第 3 版已显示（cv_1）");

    assert_eq!(
        bodies.len(),
        7,
        "三轮 + 压缩摘要各一次请求（重开那轮在另一处）"
    );
    let first_count = after.to_string().matches(&first).count();
    assert_eq!(
        first_count, 2,
        "第 1 版结果和调用参数各剩同一句（实际 {first_count} 处）"
    );
    let second_count = after.to_string().matches(&second).count();
    assert_eq!(
        second_count, 2,
        "第 2 版（edits 出的）结果和调用参数各剩同一句（实际 {second_count} 处）"
    );
    let inputs = canvas_inputs(after);
    assert_eq!(inputs.len(), 3, "请求里应有三个 canvas 调用");
    let v1 = inputs
        .iter()
        .find(|input| input.get("id").is_none())
        .expect("第 1 版调用（新建时没有 id）");
    assert_eq!(
        v1["html"].as_str(),
        Some(first.as_str()),
        "旧版 html 参数只剩一句"
    );
    assert_eq!(
        v1["title"],
        json!(CANVAS_TITLE),
        "只剪 html/edits，别的参数不动"
    );
    let v2 = inputs
        .iter()
        .find(|input| input.get("edits").is_some())
        .expect("第 2 版调用用 edits");
    assert_eq!(
        v2["edits"].as_str(),
        Some(second.as_str()),
        "旧版 edits 参数只剩一句"
    );
    assert!(v2.get("html").is_none(), "edits 调用本来就没有 html");
    let v3 = inputs
        .iter()
        .find(|input| input["html"] == json!(page(3)))
        .expect("最新版调用");
    assert_eq!(v3["html"], json!(page(3)), "最新版参数是全文");
    assert!(
        inputs
            .iter()
            .all(|input| input["html"].as_str() != Some(page(1).as_str())
                && input["html"].as_str() != Some(page(2).as_str())),
        "旧版全文不应出现在压缩后的请求里"
    );
    let results = request_tool_results(after);
    assert!(
        results.iter().any(|text| text == &first),
        "第 1 版结果只剩一句：{results:?}"
    );
    assert!(
        results.iter().any(|text| text == &second),
        "第 2 版结果只剩一句：{results:?}"
    );
    assert!(
        results.iter().any(|text| text == &latest),
        "最新版结果保留原文：{results:?}"
    );
    assert!(
        results.iter().any(|text| text == &"x".repeat(200)),
        "同批的小结果原样重落：{results:?}"
    );

    // 同一步里的另一个调用（big）也要跟被剪的 canvas 调用一起按顺序重落，
    // 否则 tool_use 会被结果甩到后面，派生请求配对不上。
    let messages = after["messages"].as_array().expect("messages");
    let position = |kind: &str, id: &str| -> Option<usize> {
        messages.iter().position(|message| {
            message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == json!(kind)
                        && (block["id"] == json!(id) || block["tool_use_id"] == json!(id))
                })
            })
        })
    };
    let small_call = position("tool_use", "call_small").expect("同批别的调用还在");
    let v1_call = position("tool_use", "call_v1").expect("旧版 canvas 调用还在");
    assert_eq!(
        small_call, v1_call,
        "同一条 assistant/message 的两个调用一起重落"
    );
    let small_result =
        position("tool_result", "call_small").expect("同批的结果也要重落，否则没有配对");
    assert!(
        small_call < small_result,
        "tool_use 必须在 tool_result 之前"
    );
    let small_input = messages[small_call]["content"]
        .as_array()
        .expect("content")
        .iter()
        .find(|block| block["id"] == json!("call_small"))
        .map(|block| block["input"].clone())
        .expect("big 的 input");
    assert_eq!(
        small_input,
        json!({"chars": 200}),
        "只剪画布参数，同批别的调用原样"
    );

    let events = &scenario.events;
    let prune_events = events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::CompactionPrune { .. }))
        .count();
    assert!(
        prune_events >= 5,
        "两个旧版调用、三条画布结果、大结果都应记 shadow-price：{prune_events}"
    );
    let originals: Vec<&session::Event> = [1_u64, 2]
        .iter()
        .map(|n| {
            events
                .iter()
                .find(|event| {
                    matches!(
                        &event.kind,
                        EventKind::ToolResult { meta: Some(meta), .. }
                            if meta["canvas"]["n"] == json!(n)
                    )
                })
                .expect("原事件还在日志里")
        })
        .collect();
    for original in &originals {
        assert_eq!(
            original.surface_op, None,
            "原事件本身不能被改写（日志是事实）"
        );
    }
    for (original, text) in originals.iter().zip([first.clone(), second.clone()]) {
        let replaced = replacement_of(events, original.seq);
        let EventKind::ToolResult { message, meta, .. } = &replaced.kind else {
            panic!("替换事件应仍是 tool/result");
        };
        assert_eq!(message_text(message), text, "替换后的结果只剩一句");
        assert!(meta.is_none(), "替换结果不再带展示 meta");
    }
    // 调用参数：第 1 版剪 html，第 2 版剪 edits（同一条 output 视图里的 tool_use）。
    let pruned_v1 = events
        .iter()
        .find(|event| {
            event.surface_op.is_some()
                && matches!(&event.kind, EventKind::AssistantMessage { message, .. }
                    if message.content.iter().any(|block| matches!(block.as_tool_call(), Some((id, _, _)) if id == "call_v1")))
        })
        .expect("第 1 版调用参数也有替换事件");
    let v1_arguments: Value =
        serde_json::from_str(&call_arguments(pruned_v1, "call_v1")).expect("arguments 是 JSON");
    assert_eq!(v1_arguments["html"], json!(first));
    let pruned_v2 = events
        .iter()
        .find(|event| {
            event.surface_op.is_some()
                && matches!(&event.kind, EventKind::AssistantMessage { message, .. }
                    if message.content.iter().any(|block| matches!(block.as_tool_call(), Some((id, _, _)) if id == "call_v2")))
        })
        .expect("第 2 版调用参数也有替换事件");
    let v2_arguments: Value =
        serde_json::from_str(&call_arguments(pruned_v2, "call_v2")).expect("arguments 是 JSON");
    assert_eq!(v2_arguments["edits"], json!(second));
    let folded = session::canvases(events);
    assert_eq!(folded.len(), 1, "折叠照读原事件，画布还是一块");
    assert_eq!(folded[0].n, 3, "折叠出最新版");
    assert_eq!(folded[0].html, page(3), "折叠拿得到最新版全文");
}

#[tokio::test]
async fn after_compaction_resume_can_edit_the_latest_canvas() {
    let scenario = run_compaction_scenario().await;
    assert!(
        scenario.events.iter().any(|event| {
            event.surface_op.is_some()
                && matches!(&event.kind, EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some("call_v1"))
        }),
        "先确认压缩真的剪了旧版（否则后面只是 .2 的折叠在起作用）"
    );
    let resumed = resume_agent_with(
        scenario.data_dir.path(),
        &scenario.id,
        &scenario.server.uri(),
    )
    .await;
    resumed
        .send(UserMessage::text("再改一下那个画布"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 10).await;
    let events = {
        let store = SessionStore::open_read_only(scenario.data_dir.path(), &scenario.id).unwrap();
        store.events().to_vec()
    };
    let fourth = canvases(&events)
        .into_iter()
        .find(|canvas| canvas["n"] == json!(4))
        .expect("重开后还能用 edits 出第 4 版");
    assert_eq!(fourth["id"], json!("cv_1"));
    assert_eq!(fourth["html"], json!(page(4)));
    assert_eq!(
        tool_text(&events, "call_v4"),
        format!("画布「{CANVAS_TITLE}」第 4 版已显示（cv_1）")
    );
}

async fn resume_agent_with(data_dir: &Path, id: &SessionId, base_url: &str) -> Agent {
    let store = SessionStore::open(data_dir, id).unwrap();
    Agent::resume(
        id.clone(),
        compaction_config(data_dir),
        Deps {
            model: DeepSeek::with_credentials(compaction_model_config(base_url), test_credential())
                .unwrap(),
            tools: ToolSet::new(vec![
                Box::new(CanvasTool::new()) as Box<dyn Tool>,
                Box::new(BigTool::new()),
            ]),
            store,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn without_compaction_consecutive_requests_keep_the_full_canvas_history() {
    let scripts = ScriptedSseServer::start(vec![
        SseScript {
            frames: tool_calls_with_inputs_sse(&[(
                "call_v1",
                "canvas",
                json!({"title": CANVAS_TITLE, "note": "第一版", "html": page(1)}),
            )]),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
        SseScript {
            frames: tool_calls_with_inputs_sse(&[(
                "call_v2",
                "canvas",
                json!({"id": "cv_1", "title": CANVAS_TITLE, "note": "第二版", "html": page(2)}),
            )]),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        isolated_config(data_dir.path()),
        Deps {
            model: DeepSeek::with_credentials(model_config(&scripts.base_url()), test_credential())
                .unwrap(),
            tools: ToolSet::new(vec![Box::new(CanvasTool::new())]),
            store,
        },
    )
    .await
    .unwrap();
    agent
        .send(UserMessage::text("画一个存钱的画布"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    agent
        .send(UserMessage::text("把柱子换成折线"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    drop(agent);

    let bodies: Vec<Value> = scripts
        .request_bodies()
        .iter()
        .map(|body| serde_json::from_slice(body).expect("请求体是 JSON"))
        .collect();
    assert_eq!(bodies.len(), 4, "两轮各两次请求");
    for index in 0..3 {
        let before = bodies[index]["messages"].as_array().expect("messages");
        let after = bodies[index + 1]["messages"].as_array().expect("messages");
        assert!(
            before.len() < after.len(),
            "第 {index} 个请求之后要有更长的历史"
        );
        assert_eq!(
            &after[..before.len()],
            before.as_slice(),
            "不压缩时连续两次请求的 messages 前缀必须逐字一样"
        );
    }
    let inputs = canvas_inputs(&bodies[3]);
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0]["html"], json!(page(1)), "没压缩就不剪旧版参数");
    assert_eq!(inputs[1]["html"], json!(page(2)));
    assert!(
        bodies[3].to_string().contains("第 1 版已显示（cv_1）"),
        "旧版结果原样在请求里"
    );

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert!(
        !store
            .events()
            .iter()
            .any(|event| matches!(event.kind, EventKind::CompactionPrune { .. })),
        "没压缩就不写剪枝事件"
    );
    let folded = session::canvases(store.events());
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].n, 2);
    assert_eq!(folded[0].html, page(2));
}

/// T015.3 审计 fix-now：旧版画布所在的 assistant 批里，如果还夹着一条超过阈值
/// （8192 字符）的普通 tool result，它也必须照 T006.4 换 head+marker+tail；
/// 不能因为和旧版同批就整份重落到 tail（压缩摘要通常保留 tail）。
#[tokio::test]
async fn old_canvas_batch_prunes_an_oversized_sibling_result() {
    const BIG_CHARS: usize = 16_000;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            // 一步里三个调用：画布第 1 版、同画布第 2 版（edits）、16k 普通结果。
            // 所以第 1 版是旧版，另外两条是 sentence=None 的同批兄弟结果。
            sse(tool_calls_with_inputs_sse(&[
                (
                    "call_v1",
                    "canvas",
                    json!({"title": CANVAS_TITLE, "note": "第一版", "html": page(1)}),
                ),
                (
                    "call_v2",
                    "canvas",
                    json!({
                        "id": "cv_1",
                        "title": CANVAS_TITLE,
                        "note": "第二版",
                        "edits": [{"old": "<div id=\"v\">1</div>", "new": "<div id=\"v\">2</div>"}],
                    }),
                ),
                ("call_big", "big", json!({"chars": BIG_CHARS})),
            ])),
            sse(FINAL_TEXT_SSE.to_owned()),
            // 修剪没把上下文压回阈值内才会再要一次摘要；正常情况下用不到。
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        compaction_config(data_dir.path()),
        Deps {
            model: DeepSeek::with_credentials(
                compaction_model_config(&server.uri()),
                test_credential(),
            )
            .unwrap(),
            tools: ToolSet::new(vec![
                Box::new(CanvasTool::new()) as Box<dyn Tool>,
                Box::new(BigTool::new()),
            ]),
            store,
        },
    )
    .await
    .unwrap();
    agent
        .send(
            UserMessage::text("画一个存钱的画布，再跑个大结果"),
            Delivery::FollowUp,
        )
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    drop(agent);

    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("请求体是 JSON"))
        .collect();
    let body = bodies.last().expect("压缩/修剪之后还要发下一步请求");
    let results = request_tool_results_with_ids(body);
    let result_of = |call_id: &str| -> String {
        results
            .iter()
            .find(|(id, _)| id == call_id)
            .unwrap_or_else(|| panic!("请求里缺少 {call_id} 的 tool/result"))
            .1
            .clone()
    };

    // 同批的普通大结果被 head+marker+tail 剪过，全文不再进请求。
    let big = result_of("call_big");
    assert!(
        big.contains("[... tool result middle pruned ...]"),
        "同批普通大结果要出现剪枝 marker：{big:?}"
    );
    assert!(
        !body.to_string().contains(&"x".repeat(BIG_CHARS)),
        "16k 全文不应出现在压缩后的请求里"
    );
    assert!(
        big.starts_with(&"x".repeat(4096)) && big.ends_with(&"x".repeat(1024)),
        "保留头 4096 和尾 1024"
    );

    // 旧版结果换一句；最新版（第 2 版）结果原样。
    assert_eq!(result_of("call_v1"), sentence(1, "第一版"));
    assert_eq!(
        result_of("call_v2"),
        format!("画布「{CANVAS_TITLE}」第 2 版已显示（cv_1）")
    );
    let inputs = canvas_inputs(body);
    let v1 = inputs
        .iter()
        .find(|input| input.get("id").is_none())
        .expect("第 1 版调用");
    assert_eq!(
        v1["html"],
        json!(sentence(1, "第一版")),
        "旧版 html 只剩一句"
    );
    assert_eq!(v1["title"], json!(CANVAS_TITLE), "别的参数不动");
    let v2 = inputs
        .iter()
        .find(|input| input.get("edits").is_some())
        .expect("第 2 版调用");
    assert_eq!(
        v2["edits"][0]["new"],
        json!("<div id=\"v\">2</div>"),
        "最新版 edits 原样"
    );
    assert!(
        !body.to_string().contains(&page(1)),
        "旧版 html 全文不应留在请求里"
    );

    // 整批重落之后，tool_use 仍在对应的 tool_result 之前，结果按调用顺序。
    let messages = body["messages"].as_array().expect("messages");
    let position = |kind: &str, id: &str| -> Option<usize> {
        messages.iter().position(|message| {
            message["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == json!(kind)
                        && (block["id"] == json!(id) || block["tool_use_id"] == json!(id))
                })
            })
        })
    };
    let v1_call = position("tool_use", "call_v1").expect("旧版 canvas 调用还在");
    let v2_call = position("tool_use", "call_v2").expect("最新版 canvas 调用还在");
    let big_call = position("tool_use", "call_big").expect("同批普通调用还在");
    assert_eq!(v1_call, v2_call, "同一条 assistant/message 一起重落");
    assert_eq!(v2_call, big_call);
    let v1_result = position("tool_result", "call_v1").expect("第 1 版结果");
    let big_result = position("tool_result", "call_big").expect("大结果");
    assert!(big_call < big_result, "tool_use 必须在 tool_result 之前");
    // 相邻 tool/result 在 wire 上合并成一条 user 消息，顺序要看同一条消息里的块顺序。
    assert_eq!(v1_result, big_result, "三条结果重落在同一批相邻消息里");
    let result_order: Vec<&str> = messages[v1_result]["content"]
        .as_array()
        .expect("content")
        .iter()
        .filter(|block| block["type"] == json!("tool_result"))
        .filter_map(|block| block["tool_use_id"].as_str())
        .collect();
    assert_eq!(
        result_order,
        ["call_v1", "call_v2", "call_big"],
        "结果按调用顺序重落"
    );

    // 日志事实：原事件仍是全文，替换事件才是修剪视图且不带展示 meta。
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let events = store.events().to_vec();
    drop(store);
    let original_big = events
        .iter()
        .find(|event| {
            matches!(&event.kind, EventKind::ToolResult { message, .. }
                if message.tool_call_id.as_deref() == Some("call_big"))
        })
        .expect("原始大结果还在日志里");
    assert_eq!(original_big.surface_op, None, "原事件本身不能被改写");
    let replaced = replacement_of(&events, original_big.seq);
    let EventKind::ToolResult { message, meta, .. } = &replaced.kind else {
        panic!("替换事件应仍是 tool/result");
    };
    assert!(
        message_text(message).contains("[... tool result middle pruned ...]"),
        "替换事件是剪枝视图"
    );
    assert!(meta.is_none(), "修剪后的结果不再带展示 meta");
}
