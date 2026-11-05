//! T006.3：压缩改回 DSH —— compaction/* 事件字段、替换写成日志消息（surfaceOp）、
//! compactionRetries、按模型覆盖、摘要输出上限、摘要请求复用 system+tools 前缀、/compact 计数。
//!
//! DSH 依据：packages/compaction/compaction-basic/src/index.ts:157-213、region.ts:473-509、
//! config.ts:172-193,235；packages/compaction/compaction/src/types.ts:24-70；
//! packages/compaction/command-compact/src/index.ts。



use std::fs;
use std::path::Path;

use agent::{Agent, AgentConfig, CompactionModelPolicy, Delivery};
use crate::common::{FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, wait_idle};
use model::UserMessage;
use serde_json::Value;
use session::{EventKind, SessionId, SessionStore};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

struct Harness {
    agent: Agent,
    data_dir: tempfile::TempDir,
    id: SessionId,
}

async fn setup(
    server: &MockServer,
    window: u64,
    model_max_tokens: u32,
    config: AgentConfig,
    responses: usize,
) -> Harness {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            (0..responses)
                .map(|_| sse(FINAL_TEXT_SSE.to_owned()))
                .collect(),
        ))
        .mount(server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let model_config = model::DeepSeekConfig {
        context_window: window,
        max_tokens: model_max_tokens,
        ..model_config(&server.uri())
    };
    let agent = Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(model_config, crate::common::test_credential())
                .unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    Harness {
        agent,
        data_dir,
        id,
    }
}

fn pressure_config(data_dir: &Path) -> AgentConfig {
    AgentConfig {
        compaction_headroom_tokens: 1,
        compaction_retain_ratio: 0.000001,
        ..isolated_config(data_dir)
    }
}

fn log_rows(data_dir: &Path, id: &SessionId) -> Vec<Value> {
    let path = data_dir.join("sessions").join(format!("{id}.jsonl"));
    let text = fs::read_to_string(path).unwrap();
    text.lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn data_of<'a>(rows: &'a [Value], event_type: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|row| row["type"] == event_type)
        .map(|row| &row["data"])
        .collect()
}

/// 一次压力压缩场景：turn1 小消息；turn2 大消息触发请求前压缩。
async fn run_pressure(server: &MockServer, harness: &Harness) {
    harness
        .agent
        .send(UserMessage::text("TEXT-ONE early"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;
    harness
        .agent
        .send(
            UserMessage::text(format!("TEXT-TWO {}", "y".repeat(3000))),
            Delivery::FollowUp,
        )
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;
    let _ = server;
}

#[tokio::test]
async fn compaction_events_carry_dsh_fields_and_replacement() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        // 只压一次，方便断言一次事务的字段与替换消息（重试行为有单独测试）。
        compaction_retries: 0,
        ..pressure_config(data_dir.path())
    };
    let harness = setup(&server, 1000, 100, config, 3).await;
    run_pressure(&server, &harness).await;

    let rows = log_rows(harness.data_dir.path(), &harness.id);
    let starts = data_of(&rows, "compaction/start");
    let summaries = data_of(&rows, "compaction/summary");
    let ends = data_of(&rows, "compaction/end");
    assert_eq!(starts.len(), 1);
    assert_eq!(summaries.len(), 1);
    assert_eq!(ends.len(), 1);
    let start = starts[0];
    let summary = summaries[0];
    let end = ends[0];
    assert_eq!(start["turn"], 2, "自动压缩在触发它的那个 turn 内进行");
    assert!(start.get("sourceCommandId").is_none());
    assert_eq!(start["compactionId"], summary["compactionId"]);
    assert_eq!(end["compactionId"], summary["compactionId"]);
    assert!(end.get("error").is_none());

    assert_eq!(summary["provider"], "deepseek");
    assert_eq!(summary["model"], "deepseek-flash");
    assert_eq!(summary["llmStreamCall"], true);
    assert!(
        summary["summary"]
            .as_array()
            .is_some_and(|blocks| !blocks.is_empty()),
        "摘要内容块：{summary}"
    );
    assert!(
        summary["rawOutput"]
            .as_array()
            .is_some_and(|blocks| !blocks.is_empty()),
        "rawOutput 保存模型原始输出：{summary}"
    );
    assert!(
        summary["shadowedSeqs"]
            .as_array()
            .is_some_and(|seqs| !seqs.is_empty()),
        "shadowedSeqs 列出被替换 surface 节点：{summary}"
    );
    assert!(
        summary["shadowedTokenCount"].as_u64().unwrap() > 0,
        "shadowedTokenCount 是正数"
    );
    let range_start = summary["shadowedRange"]["start"].as_u64().unwrap();
    let range_end = summary["shadowedRange"]["end"].as_u64().unwrap();
    assert!(range_start <= range_end);

    // 替换写成日志里的 user/message，带 envelope 的 surfaceOp/sourceEventSeqs。
    let replacement = rows
        .iter()
        .find(|row| {
            row["type"] == "user/message"
                && row["data"]["message"]["source"]["kind"] == "compact-checkpoint"
        })
        .expect("压缩替换是一条 user/message（source=compact-checkpoint）");
    assert_eq!(
        replacement["surfaceOp"],
        serde_json::json!({"op": "replace", "startSeq": range_start, "endSeq": range_end})
    );
    assert_eq!(
        replacement["data"]["message"]["source"]["compactionId"],
        summary["compactionId"]
    );
    assert!(
        replacement["sourceEventSeqs"]
            .as_array()
            .is_some_and(|seqs| !seqs.is_empty()),
        "sourceEventSeqs 记录引用的压缩事件：{replacement}"
    );

    // 顺序照 DSH：start → summary → 替换消息 → end。
    let position = |needle: &str| {
        rows.iter()
            .position(|row| row["type"] == needle)
            .unwrap_or_else(|| panic!("日志里没有 {needle}"))
    };
    assert!(position("compaction/start") < position("compaction/summary"));
    let replacement_seq = replacement["seq"].as_u64().unwrap();
    let replacement_compaction_id = &replacement["data"]["message"]["source"]["compactionId"];
    let seq_of = |event_type: &str, compaction_id: Option<&Value>| {
        rows.iter()
            .find(|row| {
                row["type"] == event_type
                    && compaction_id.is_none_or(|id| &row["data"]["compactionId"] == id)
            })
            .map(|row| row["seq"].as_u64().unwrap())
            .unwrap_or_else(|| panic!("日志里没有 {event_type}"))
    };
    let summary_seq = seq_of("compaction/summary", Some(replacement_compaction_id));
    let end_seq = seq_of("compaction/end", Some(replacement_compaction_id));
    assert!(summary_seq < replacement_seq, "summary 先于替换消息");
    assert!(replacement_seq < end_seq, "替换消息先于 end");

    // 派生历史读日志里的 checkpoint，不再自己合成第二条。
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    let derived = store.derive_messages();
    let checkpoints = derived
        .iter()
        .filter(|message| format!("{message:?}").contains("compacted-summary"))
        .count();
    assert_eq!(checkpoints, 1, "只应有一条 checkpoint：{derived:?}");
    assert!(
        !derived
            .iter()
            .any(|message| format!("{message:?}").contains("TEXT-ONE")),
        "被替换的早期消息不能出现在派生视图"
    );
}

#[tokio::test]
async fn summary_request_reuses_system_tools_and_prefix_cap() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        compaction_summary_max_tokens: Some(321),
        compaction_retries: 0,
        ..pressure_config(data_dir.path())
    };
    let harness = setup(&server, 1000, 100, config, 3).await;
    run_pressure(&server, &harness).await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3, "普通请求 + 摘要请求 + 压缩后请求");
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let summary: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert!(
        String::from_utf8_lossy(&requests[1].body).contains("compaction engine"),
        "第二次请求是摘要"
    );

    // 摘要请求复用 system + tools + 被压缩前缀（命中 provider 前缀缓存）。
    assert_eq!(summary["system"], first["system"], "摘要请求复用 system");
    assert!(
        summary["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()),
        "摘要请求带 tools：{summary}"
    );
    assert_eq!(summary["tools"], first["tools"], "tools 与上次请求一致");
    let summary_messages = summary["messages"].as_array().unwrap();
    let first_messages = first["messages"].as_array().unwrap();
    assert_eq!(
        &summary_messages[..first_messages.len()],
        first_messages.as_slice(),
        "摘要请求的消息前缀与上次请求一致"
    );
    assert!(
        summary_messages.last().unwrap()["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("compaction engine")),
        "最后一条是摘要指令"
    );

    // 摘要输出上限照 DSH config.maxTokens（policy）走 request max_tokens 与事件字段。
    assert_eq!(
        summary["max_tokens"], 321,
        "摘要请求 max_tokens 是配置的输出上限"
    );
    let rows = log_rows(harness.data_dir.path(), &harness.id);
    let summaries = data_of(&rows, "compaction/summary");
    assert_eq!(summaries[0]["maxTokens"], 321);
}

#[tokio::test]
async fn compaction_retries_until_below_threshold_or_error() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        compaction_retries: 2,
        ..pressure_config(data_dir.path())
    };
    let harness = setup(&server, 1000, 100, config, 6).await;
    run_pressure(&server, &harness).await;

    let rows = log_rows(harness.data_dir.path(), &harness.id);
    let starts = data_of(&rows, "compaction/start");
    assert_eq!(
        starts.len(),
        3,
        "compactionRetries=2 → 最多 3 次压缩尝试：{starts:?}"
    );
    let ends = data_of(&rows, "compaction/end");
    assert_eq!(ends.len(), 3);
    // 每次压缩事务自身都成功提交（DSH 的压缩循环在循环外抛「仍超阈值」错误，不再追加事件）。
    assert!(
        ends.iter().all(|end| end.get("error").is_none()),
        "单次事务成功：{ends:?}"
    );
    // 自动压缩失败不打断本轮：turn2 仍正常完成。
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    let turn_ends = store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(turn_ends.len(), 2, "两个 turn 都收尾：{turn_ends:?}");
}

#[tokio::test]
async fn manual_compact_counts_and_source_command_id() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        compaction_retain_ratio: 0.000001,
        ..isolated_config(data_dir.path())
    };
    let harness = setup(&server, 1_000_000, 2048, config, 3).await;
    harness
        .agent
        .send(UserMessage::text("FIRST manual"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;

    let outcome = harness
        .agent
        .compact(Some("cmd-1".to_owned()))
        .await
        .expect("手动压缩成功")
        .expect("有可压缩历史");
    let rows = log_rows(harness.data_dir.path(), &harness.id);
    let summaries = data_of(&rows, "compaction/summary");
    assert_eq!(summaries.len(), 1);
    assert_eq!(
        outcome.shadowed_items,
        summaries[0]["shadowedSeqs"].as_array().unwrap().len()
    );
    assert_eq!(
        outcome.shadowed_tokens,
        summaries[0]["shadowedTokenCount"].as_u64().unwrap()
    );
    let starts = data_of(&rows, "compaction/start");
    let ends = data_of(&rows, "compaction/end");
    assert_eq!(starts[0]["turn"], Value::Null, "两次 turn 之间的手动压缩");
    assert_eq!(starts[0]["sourceCommandId"], "cmd-1");
    assert_eq!(summaries[0]["sourceCommandId"], "cmd-1");
    assert_eq!(ends[0]["sourceCommandId"], "cmd-1");
    assert!(ends[0].get("error").is_none());
    assert!(
        rows.iter()
            .any(|row| row["type"] == "user/message" && row["surfaceOp"]["op"] == "replace"),
        "手动压缩同样写成日志消息 + surfaceOp"
    );
}

#[tokio::test]
async fn model_policy_overrides_summary_policy() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        compaction_model_policies: vec![CompactionModelPolicy {
            provider: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            threshold_ratio: None,
            retain_ratio: None,
            headroom_tokens: None,
            summary_max_tokens: Some(123),
            compaction_retries: Some(0),
            max_overflow_retries: None,
        }],
        ..pressure_config(data_dir.path())
    };
    let harness = setup(&server, 1000, 100, config, 3).await;
    run_pressure(&server, &harness).await;

    let requests = server.received_requests().await.unwrap();
    let summary: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(summary["max_tokens"], 123, "按模型策略覆盖摘要输出上限");
    let rows = log_rows(harness.data_dir.path(), &harness.id);
    let summaries = data_of(&rows, "compaction/summary");
    assert_eq!(summaries[0]["maxTokens"], 123);
    let starts = data_of(&rows, "compaction/start");
    assert_eq!(starts.len(), 1, "compactionRetries=0 只压一次");
}

#[tokio::test]
async fn manual_compact_no_history_returns_none() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let config = isolated_config(data_dir.path());
    let harness = setup(&server, 1_000_000, 2048, config, 0).await;
    let outcome = harness.agent.compact(None).await.expect("不应报错");
    assert!(outcome.is_none(), "没有可压缩历史时返回 None");
    let rows = log_rows(harness.data_dir.path(), &harness.id);
    assert!(
        data_of(&rows, "compaction/start").is_empty(),
        "没有可压缩历史不写 compaction 事件"
    );
}
