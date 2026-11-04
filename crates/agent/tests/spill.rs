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
use wiremock::{Mock, MockServer, ResponseTemplate};

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
            model: model::DeepSeek::with_credentials(
                model_config(&server.uri()),
                common::test_credential(),
            )
            .unwrap(),
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
fn tool_result_text(store: &session::SessionSnapshot, call_id: &str) -> String {
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
        &session::SessionStore::open_read_only(data_dir.path(), agent.id()).unwrap(),
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
                .join(tools::spill::session_dir_name(agent.id().as_str()))
        ),
        "会话目录照 DSH spill-local：root/session-<hash>：{locator}"
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
        &session::SessionStore::open_read_only(data_dir.path(), agent.id()).unwrap(),
        "call_0",
    );
    assert_eq!(stored, content, "禁用 spill 时全文留在上下文");
    assert!(
        !data_dir.path().join("spill").exists(),
        "禁用 spill 时不应建 spill 目录"
    );
}
/// T005.2 旧行为是每请求前剪；T006.4 照 DSH 改成只在压缩时剪（压力/溢出两路），
/// 手动 /compact 不剪（DSH compactNow 不调 pruner）。
#[tokio::test]
async fn tool_result_not_pruned_without_compaction_and_manual_compact_does_not_prune() {
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
    // 没压缩就不剪：同一轮第二步、下一轮请求都还是全文。
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        !String::from_utf8(requests[1].body.clone())
            .unwrap()
            .contains("[... tool result middle pruned ...]")
    );
    agent
        .send(UserMessage::text("第二轮：再来"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    let third = String::from_utf8(requests[2].body.clone()).unwrap();
    assert!(
        !third.contains("[... tool result middle pruned ...]"),
        "没压缩就不修剪"
    );
    assert!(third.contains("row0001"), "没压缩时仍是完整结果");

    // 手动 /compact 照 DSH compactNow：不调 pruner，不产生 compaction/prune、不剪 tool/result。
    let outcome = agent
        .compact(Some("cmd-1".to_owned()))
        .await
        .unwrap()
        .expect("有可压缩历史");
    assert!(outcome.shadowed_items > 0);
    let store = SessionStore::open_read_only(data_dir.path(), agent.id()).unwrap();
    let events = store.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, EventKind::CompactionStart { .. })),
        "手动压缩本身要写 compaction/start"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, EventKind::CompactionPrune { .. })),
        "手动 /compact 不剪（DSH compactNow 不调 pruner）"
    );
    // 原始 tool/result 未被替换：日志里只有这一条 tool/result 且内容完整。
    let tool_results: Vec<&session::Event> = events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::ToolResult { .. }))
        .collect();
    assert_eq!(tool_results.len(), 1, "手动压缩不写替换事件");
    let EventKind::ToolResult { message, .. } = &tool_results[0].kind else {
        unreachable!("上面刚筛过 ToolResult");
    };
    let original_text: String = message
        .content
        .iter()
        .filter_map(model::ContentBlock::as_text)
        .collect();
    assert_eq!(original_text, content, "原始 tool/result 内容完整");
    let derived_text: String = store
        .derive_messages()
        .iter()
        .flat_map(|message| {
            message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
        })
        .collect();
    assert!(
        !derived_text.contains("[... tool result middle pruned ...]"),
        "手动 /compact 不剪，派生视图没有修剪占位"
    );

    // 手动压缩后的下一轮请求照常工作，且仍不带修剪占位。
    agent
        .send(UserMessage::text("第三轮：看修剪"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 5);
    let summary = String::from_utf8(requests[3].body.clone()).unwrap();
    assert!(
        summary.contains("compaction engine"),
        "第 4 个请求是 /compact 的摘要请求"
    );
    let after = String::from_utf8(requests[4].body.clone()).unwrap();
    assert!(
        !after.contains("[... tool result middle pruned ...]"),
        "手动 /compact 不剪，后续请求也没有修剪占位"
    );
}

fn overflow_response() -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({
        "error": {"type": "invalid_request_error", "message": "This model's maximum context length is 128000 tokens, you requested 200000"}
    }))
}
#[tokio::test]
async fn auto_pressure_compaction_prunes_tool_results() {
    let server = MockServer::start().await;
    let lines: Vec<String> = (1..=500)
        .map(|number| format!("row{number:04} {}", "z".repeat(10)))
        .collect();
    let content = format!("{}\n", lines.join("\n"));
    // 窗口调小：工具结果一落地就压过阈值，下一步请求前触发压缩事务（含 prune）。
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
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let config = AgentConfig {
        compaction_headroom_tokens: 1,
        compaction_retain_ratio: 0.000001,
        compaction_retries: 0,
        ..common::isolated_config(data_dir.path())
    };
    let model_config = model::DeepSeekConfig {
        context_window: 1000,
        max_tokens: 100,
        ..common::model_config(&server.uri())
    };
    let agent = Agent::create(
        config,
        Deps {
            model: model::DeepSeek::with_credentials(model_config, common::test_credential())
                .unwrap(),
            tools: ToolSet::new(vec![Box::new(BigTool::new(content.clone()))]),
            store,
        },
    )
    .await
    .unwrap();
    agent
        .send(UserMessage::text("第一轮：跑大工具"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let store = SessionStore::open_read_only(data_dir.path(), agent.id()).unwrap();
    let events = store.events();
    let start_index = events
        .iter()
        .position(|event| matches!(event.kind, EventKind::CompactionStart { .. }))
        .expect("自动压力压缩写 compaction/start");
    let prune_index = events
        .iter()
        .position(|event| matches!(event.kind, EventKind::CompactionPrune { .. }))
        .expect("自动压缩事务也应触发 prune");
    assert!(prune_index < start_index, "prune 在压缩事务开头");
    // 压缩后的请求带修剪视图（tail 里仍可见的 tool 结果是替换消息）。
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3, "tool call + 摘要 + 压缩后的请求");
    let body = String::from_utf8(requests[2].body.clone()).unwrap();
    assert!(
        body.contains("[... tool result middle pruned ...]"),
        "压缩后请求带修剪视图"
    );
    assert!(!body.contains(&content), "压缩后请求不带完整旧结果");
}

/// 溢出恢复（RequestError 强制压缩）也触发 prune，并保留原始请求错误语义。
#[tokio::test]
async fn overflow_recovery_prunes_tool_results() {
    let server = MockServer::start().await;
    let lines: Vec<String> = (1..=500)
        .map(|number| format!("row{number:04} {}", "z".repeat(10)))
        .collect();
    let content = format!("{}\n", lines.join("\n"));
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[("call_0", "big", json!({}))])),
            sse(FINAL_TEXT_SSE.to_owned()),
            overflow_response(),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
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
    agent
        .send(UserMessage::text("第二轮：触发溢出"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let store = SessionStore::open_read_only(data_dir.path(), agent.id()).unwrap();
    let events = store.events();
    let start_index = events
        .iter()
        .position(|event| matches!(event.kind, EventKind::CompactionStart { .. }))
        .expect("溢出恢复写 compaction/start");
    let prune_index = events
        .iter()
        .position(|event| matches!(event.kind, EventKind::CompactionPrune { .. }))
        .expect("溢出恢复也触发 prune");
    assert!(prune_index < start_index, "prune 在摘要压缩事务之前");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        5,
        "tool call + 轮内请求 + 溢出 + 摘要 + 重试"
    );
    let summary = String::from_utf8(requests[3].body.clone()).unwrap();
    assert!(summary.contains("compaction engine"));
    // 修剪替换事件 seq 更大 → 在 surface 末尾；重试请求带修剪视图、不带完整旧结果。
    let retry = String::from_utf8(requests[4].body.clone()).unwrap();
    assert!(
        retry.contains("[... tool result middle pruned ...]"),
        "重试请求带修剪视图"
    );
    assert!(retry.contains("row0001"), "修剪保留头部");
    assert!(!retry.contains("row0300"), "中间段已被剪掉");
}

/// T006.7：Agent 启动时照 DSH spill-local cleanupPeriodDays 扫一遍旧 spill 文件。
#[tokio::test]
async fn old_spill_files_are_cleaned_on_agent_start() {
    use std::time::{Duration, SystemTime};

    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let old_dir = data_dir
        .path()
        .join("spill")
        .join(tools::spill::session_dir_name("old-session"));
    std::fs::create_dir_all(&old_dir).unwrap();
    let old_file = old_dir.join("0123456789ab-old.txt");
    std::fs::write(&old_file, "old").unwrap();
    let modified = SystemTime::now() - Duration::from_secs(31 * 24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(&old_file)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let fresh_file = old_dir.join("0123456789ac-fresh.txt");
    std::fs::write(&fresh_file, "fresh").unwrap();

    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let agent = Agent::create(
        common::isolated_config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                common::model_config("http://127.0.0.1:1"),
                common::test_credential(),
            )
            .unwrap(),
            tools: ToolSet::new(Vec::new()),
            store,
        },
    )
    .await
    .unwrap();
    assert!(
        !old_file.exists(),
        "超过 cleanupPeriodDays 的 spill 文件应清掉"
    );
    assert!(fresh_file.exists(), "新 spill 文件保留");
    agent.idle().await;
}
