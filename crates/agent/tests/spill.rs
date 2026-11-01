//! 接缝 1：大工具结果 spill 与大结果修剪（测试清单第 8 条）。

mod common;

use std::fs;
use std::path::PathBuf;

use agent::{Agent, AgentConfig, Delivery, Deps};
use common::{FINAL_TEXT_SSE, SequenceResponder, model_config, sse, tool_calls_with_inputs_sse};
use model::UserMessage;
use serde_json::{Value, json};
use session::{EventKind, SessionStore};
use tools::{Tool, ToolCall, ToolContext, ToolFuture, ToolResult, ToolSet, estimate_text_tokens};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

/// 每次调用返回同一段大文本的测试工具（模拟 bash / 抓取的超大输出）。
struct BigTool {
    content: String,
    parameters: Value,
}

impl BigTool {
    fn new(content: String) -> Self {
        Self {
            content,
            parameters: json!({"type": "object", "properties": {}}),
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
        Box::pin(async move { ToolResult::success(call.id, self.content.clone()) })
    }
}

async fn setup(
    data_dir: &std::path::Path,
    cwd: &std::path::Path,
    server: &MockServer,
    tools: ToolSet,
    config: AgentConfig,
) -> Agent {
    let store = SessionStore::create(data_dir, cwd).unwrap();
    Agent::create(
        config,
        Deps {
            model: model::DeepSeek::new(model_config(&server.uri())).unwrap(),
            tools,
            store,
        },
    )
    .await
    .unwrap()
}

async fn mount(server: &MockServer, bodies: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            bodies.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

/// 会话日志里某个 call_id 的 tool/result 文本。
fn tool_result_text(store: &SessionStore, call_id: &str) -> String {
    for event in store.events() {
        let EventKind::ToolResult { message, .. } = &event.kind else {
            continue;
        };
        if message.tool_call_id.as_deref() == Some(call_id) {
            return message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
        }
    }
    panic!("日志里没有 call_id {call_id} 的 tool/result");
}

/// 用 read 工具按 locator 把全文读回来（分两页，read 有 2000 行/50KB 上限）。
async fn read_all(cwd: &std::path::Path, locator: &str, total_lines: usize) -> String {
    let context = ToolContext::new(cwd);
    let read = tools::ReadTool::new();
    let mut lines: Vec<String> = Vec::new();
    let mut offset = 1;
    while offset <= total_lines {
        let limit = 2000;
        let result = read
            .execute(
                ToolCall {
                    id: format!("read_{offset}"),
                    name: "read".to_owned(),
                    arguments: json!({"file_path": locator, "offset": offset, "limit": limit})
                        .to_string(),
                },
                &context,
            )
            .await;
        assert!(!result.is_error, "{}", result.content);
        let body_start = result.content.find("<content>\n").unwrap() + "<content>\n".len();
        let body_end = result.content.rfind("\n\n(").unwrap();
        for line in result.content[body_start..body_end].split('\n') {
            let (_, text) = line.split_once(": ").expect("read 行号格式");
            lines.push(text.to_owned());
        }
        offset += limit;
    }
    let mut joined = lines.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

fn big_content() -> (String, usize) {
    let lines: Vec<String> = (1..=3000)
        .map(|number| format!("line{number:04} {}", "abcdefghij"))
        .collect();
    (format!("{}\n", lines.join("\n")), 3000)
}

#[tokio::test]
async fn oversized_tool_result_becomes_preview_with_locator() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[("call_0", "big", json!({}))]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (content, total_lines) = big_content();
    assert!(estimate_text_tokens(&content) > 12_500);
    let agent = setup(
        data_dir.path(),
        cwd.path(),
        &server,
        ToolSet::new(vec![Box::new(BigTool::new(content.clone()))]),
        common::isolated_config(data_dir.path()),
    )
    .await;
    agent
        .send(UserMessage::text("跑个大工具"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let stored = tool_result_text(
        &SessionStore::open(data_dir.path(), agent.id()).unwrap(),
        "call_0",
    );
    assert!(
        stored.contains("Full formatted result stored at: "),
        "{stored:.300}"
    );
    assert!(!stored.contains(&content), "完整结果不应留在上下文里");
    let locator = {
        let start = stored.find("stored at: ").unwrap() + "stored at: ".len();
        let rest = &stored[start..];
        let end = rest.find(". Use ").unwrap();
        rest[..end].to_owned()
    };
    assert!(
        PathBuf::from(&locator).starts_with(
            data_dir
                .path()
                .join("spill")
                .join(format!("session-{}", agent.id()))
        ),
        "{locator}"
    );
    assert_eq!(fs::read_to_string(&locator).unwrap(), content);

    // 下一次请求里只有 preview，没有全文。
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(
        second.contains("Full formatted result stored at: "),
        "第二次请求应带 preview"
    );
    assert!(!second.contains(&content), "第二次请求不应带全文");

    // 模型按 locator 取回全文。
    let retrieved = read_all(cwd.path(), &locator, total_lines).await;
    assert_eq!(retrieved, content);
}

#[tokio::test]
async fn zero_spill_threshold_disables_spill() {
    // T005.0 known-weakness 补验：spill_max_inline_tokens=0 时真的不落盘、全文留在上下文。
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[("call_0", "big", json!({}))]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (content, _) = big_content();
    let config = AgentConfig {
        spill_max_inline_tokens: 0,
        ..common::isolated_config(data_dir.path())
    };
    let agent = setup(
        data_dir.path(),
        cwd.path(),
        &server,
        ToolSet::new(vec![Box::new(BigTool::new(content.clone()))]),
        config,
    )
    .await;
    agent
        .send(UserMessage::text("跑个大工具"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let stored = tool_result_text(
        &SessionStore::open(data_dir.path(), agent.id()).unwrap(),
        "call_0",
    );
    assert_eq!(stored, content, "禁用 spill 时全文留在上下文");
    assert!(
        !data_dir.path().join("spill").exists(),
        "禁用 spill 时不应建 spill 目录"
    );
}

#[tokio::test]
async fn tool_result_pruned_before_next_request() {
    let server = MockServer::start().await;
    // 10,000 个码点：超过修剪阈值 8192，但不到 spill 阈值 12500 token。
    let lines: Vec<String> = (1..=500)
        .map(|number| format!("row{number:04} {}", "z".repeat(10)))
        .collect();
    let content = format!("{}\n", lines.join("\n"));
    assert!(content.chars().count() > 8192);
    assert!(estimate_text_tokens(&content) < 12_500);
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[("call_0", "big", json!({}))]),
            FINAL_TEXT_SSE.to_owned(),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let agent = setup(
        data_dir.path(),
        cwd.path(),
        &server,
        ToolSet::new(vec![Box::new(BigTool::new(content.clone()))]),
        common::isolated_config(data_dir.path()),
    )
    .await;
    agent
        .send(UserMessage::text("第一轮：跑大工具"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    // 同一轮第二步的请求里还是全文（只修剪「旧」结果）。
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(!second.contains("[... tool result middle pruned ...]"));

    agent
        .send(UserMessage::text("第二轮：再来"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    let third = String::from_utf8(requests[2].body.clone()).unwrap();
    assert!(
        third.contains("[... tool result middle pruned ...]"),
        "下一请求应带修剪后的占位"
    );
    assert!(!third.contains(&content), "下一请求不应带完整旧结果");

    // 原始 tool/result 仍在日志里（可回放），另有 compaction/prune 事件记录替换。
    let content_expected = content.clone();
    let store = SessionStore::open(data_dir.path(), agent.id()).unwrap();
    // 原始 tool/result 的 content 仍是完整文本（修剪只写替换事件，不改原事件）。
    assert!(store.events().iter().any(|event| match &event.kind {
        EventKind::ToolResult { message, .. } => {
            message.tool_call_id.as_deref() == Some("call_0")
                && message
                    .content
                    .iter()
                    .any(|block| block.as_text() == Some(content_expected.as_str()))
        }
        _ => false,
    }));
    assert!(store.events().iter().any(|event| matches!(&event.kind, EventKind::CompactionPrune { shadowed_seqs, .. } if !shadowed_seqs.is_empty())));
}
