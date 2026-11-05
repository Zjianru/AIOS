//! 接缝 1：手动 compact、运行中 busy 与续接一致性（测试清单第 10、11 条；T006.F1 起不排队）。

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, ToolEvent, gated, isolated_config, model_config, sse,
    tool_calls_sse, wait_idle,
};
use agent::{
    Agent, AgentConfig, AgentError, AgentEvent, CompactionTrigger, Delivery,
    ManualCompactionErrorCode,
};
use model::UserMessage;
use session::SessionStore;
use tools::{ToolConcurrency, ToolSet};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Harness {
    agent: Agent,
    data_dir: tempfile::TempDir,
    id: session::SessionId,
}

async fn setup(server: &MockServer, tools: ToolSet, retain_ratio: f64) -> Harness {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let config = AgentConfig {
        compaction_retain_ratio: retain_ratio,
        ..isolated_config(data_dir.path())
    };
    let model_config = model::DeepSeekConfig {
        context_window: 1_000_000,
        max_tokens: 2048,
        ..model_config(&server.uri())
    };
    let agent = Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config,
                crate::common::test_credential(),
            )
            .unwrap(),
            tools,
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

fn bodies(requests: &[wiremock::Request]) -> Vec<String> {
    requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect()
}

async fn mount(server: &MockServer, responses: Vec<ResponseTemplate>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(responses))
        .mount(server)
        .await;
}

fn compact_events(store: &session::SessionSnapshot) -> Vec<(String, &'static str)> {
    store
        .events()
        .iter()
        .filter(|event| event.kind.type_name().starts_with("compaction/"))
        .map(|event| {
            let id = match &event.kind {
                session::EventKind::CompactionStart { compaction_id, .. }
                | session::EventKind::CompactionSummary { compaction_id, .. }
                | session::EventKind::CompactionEnd { compaction_id, .. } => compaction_id.clone(),
                _ => String::new(),
            };
            (id, event.kind.type_name())
        })
        .collect()
}

#[tokio::test]
async fn manual_compact_while_idle_compresses_immediately() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let harness = setup(&server, ToolSet::builtin(), 0.000001).await;
    let mut events = harness.agent.subscribe();
    harness
        .agent
        .send(UserMessage::text("TEXT-OLD manual"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;

    let done = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        harness.agent.compact(None),
    )
    .await
    .is_ok();
    assert!(done, "/compact 不能挂住");

    let requests = server.received_requests().await.unwrap();
    let bodies = bodies(&requests);
    assert_eq!(bodies.len(), 2, "空闲 compact 应立刻发一次摘要请求");
    assert!(
        bodies[1].contains("compaction engine"),
        "第二次请求是摘要请求"
    );
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    let events_in_log = compact_events(&store);
    assert_eq!(
        events_in_log
            .iter()
            .map(|(_, name)| *name)
            .collect::<Vec<_>>(),
        vec!["compaction/start", "compaction/summary", "compaction/end"]
    );
    assert!(
        !events_in_log[0].0.is_empty(),
        "compaction_id 应存在于事件里"
    );

    let mut started_manual = false;
    let mut ended = false;
    while let Ok(event) = events.try_recv() {
        match event {
            AgentEvent::CompactionStarted {
                trigger: CompactionTrigger::Manual,
                ..
            } => started_manual = true,
            AgentEvent::CompactionEnded { error: None, .. } => ended = true,
            _ => {}
        }
    }
    assert!(started_manual, "手动压缩应发 Manual 的 CompactionStarted");
    assert!(ended, "手动压缩应发 CompactionEnded");
}

#[tokio::test]
async fn manual_compact_busy_while_turn_running() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            sse(tool_calls_sse(&[("call_0", "gated")])),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut permits: Vec<(&'static str, std::sync::Arc<tokio::sync::Semaphore>)> = Vec::new();
    let tool = gated(
        "gated",
        ToolConcurrency::Exclusive,
        &started_tx,
        &mut permits,
    );
    let harness = setup(&server, ToolSet::new(vec![tool]), 0.000001).await;
    let mut events = harness.agent.subscribe();

    harness
        .agent
        .send(UserMessage::text("run gated"), Delivery::FollowUp)
        .await
        .unwrap();
    let started = started_rx.recv().await.expect("工具应收到调用");
    assert!(matches!(started, ToolEvent::Started(name) if name == "gated"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    // 轮次还在跑时请求 compact：照来源回 busy，不排队、不发摘要请求。
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.agent.compact(None),
    )
    .await
    .expect("运行中压缩必须立刻回 busy，不能排队挂住");
    let error = outcome.expect_err("运行中压缩必须返回 busy");
    assert!(
        matches!(
            error,
            AgentError::ManualCompaction {
                code: ManualCompactionErrorCode::Busy,
                ..
            }
        ),
        "应为 busy：{error:?}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    permits[0].1.add_permits(1);
    wait_idle(&harness.agent, 20).await;
    let requests = server.received_requests().await.unwrap();
    let bodies = bodies(&requests);
    assert_eq!(bodies.len(), 2, "只有工具调用 + 本轮结束，不发压缩摘要");
    assert!(
        bodies
            .iter()
            .all(|body| !body.contains("compaction engine")),
        "运行中的 compact 不能发出摘要请求"
    );
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::CompactionStart { .. }))
            .count(),
        0,
        "busy 的 compact 不能落压缩事件"
    );
    let mut manual_started = false;
    while let Ok(event) = events.try_recv() {
        if matches!(
            event,
            AgentEvent::CompactionStarted {
                trigger: CompactionTrigger::Manual,
                ..
            }
        ) {
            manual_started = true;
        }
    }
    assert!(!manual_started, "busy 的 compact 不能开始压缩");
}
#[tokio::test]
async fn resume_after_compaction_derives_same_messages() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let harness = setup(&server, ToolSet::builtin(), 0.000001).await;
    harness
        .agent
        .send(UserMessage::text("TEXT-OLD resume"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;
    let _ = harness.agent.compact(None).await;
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    let before = store.derive_messages();
    assert!(
        before
            .iter()
            .any(|message| format!("{message:?}").contains("compacted-summary")),
        "压缩后派生应含 checkpoint"
    );
    drop(harness.agent);

    let config = AgentConfig {
        compaction_retain_ratio: 0.000001,
        ..isolated_config(harness.data_dir.path())
    };
    let model_config = model::DeepSeekConfig {
        context_window: 1_000_000,
        max_tokens: 2048,
        ..model_config(&server.uri())
    };
    let resumed = Agent::resume(
        harness.id.clone(),
        config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config,
                crate::common::test_credential(),
            )
            .unwrap(),
            tools: ToolSet::builtin(),
            store: SessionStore::open(harness.data_dir.path(), &harness.id).unwrap(),
        },
    )
    .await
    .expect("续接应成功");
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    assert_eq!(
        store.derive_messages(),
        before,
        "续接后的派生请求应与退出前一致"
    );

    resumed
        .send(UserMessage::text("TEXT-NEW resume"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 20).await;
    let requests = server.received_requests().await.unwrap();
    let bodies = bodies(&requests);
    assert_eq!(bodies.len(), 3);
    assert!(
        bodies[2].contains("<compacted-summary>"),
        "续接请求带 checkpoint"
    );
    assert!(bodies[2].contains("TEXT-NEW"));
    assert!(!bodies[2].contains("TEXT-OLD"), "较早历史被摘要替换");
}

#[tokio::test]
async fn compaction_ids_are_unique_across_resume() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let harness = setup(&server, ToolSet::builtin(), 0.000001).await;
    harness
        .agent
        .send(UserMessage::text("ID-ONE history"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;
    let _ = harness.agent.compact(None).await;
    drop(harness.agent);

    let config = AgentConfig {
        compaction_retain_ratio: 0.000001,
        ..isolated_config(harness.data_dir.path())
    };
    let model_config = model::DeepSeekConfig {
        context_window: 1_000_000,
        max_tokens: 2048,
        ..model_config(&server.uri())
    };
    let resumed = Agent::resume(
        harness.id.clone(),
        config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config,
                crate::common::test_credential(),
            )
            .unwrap(),
            tools: ToolSet::builtin(),
            store: SessionStore::open(harness.data_dir.path(), &harness.id).unwrap(),
        },
    )
    .await
    .expect("续接应成功");
    let _ = resumed.compact(None).await;

    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    let ids: Vec<String> = store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::CompactionStart { compaction_id, .. } => {
                Some(compaction_id.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2, "两次压缩都进日志：{ids:?}");
    assert_ne!(ids[0], ids[1], "续接后的 compaction_id 不能和旧 id 重复");
}

fn delayed_sse(body: String) -> ResponseTemplate {
    sse(body).set_delay(std::time::Duration::from_millis(500))
}

async fn compact_and_get_id() -> String {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let harness = setup(&server, ToolSet::builtin(), 0.000001).await;
    harness
        .agent
        .send(UserMessage::text("TEXT-PROCESS id"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;
    let _ = harness.agent.compact(None).await;
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    store
        .events()
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::CompactionStart { compaction_id, .. } => {
                Some(compaction_id.clone())
            }
            _ => None,
        })
        .expect("应有 compaction/start")
}

const ID_CHILD_ENV: &str = "AIOS_TEST_COMPACTION_ID_CHILD";
const ID_FILE_ENV: &str = "AIOS_TEST_COMPACTION_ID_FILE";

#[tokio::test]
async fn compaction_ids_differ_across_processes() {
    let id = compact_and_get_id().await;
    if std::env::var_os(ID_CHILD_ENV).is_some() {
        std::fs::write(std::env::var(ID_FILE_ENV).unwrap(), id).unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("child-id.txt");
    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(&exe)
        .arg("compact_manual::compaction_ids_differ_across_processes")
        .arg("--exact")
        .arg("--nocapture")
        .env(ID_CHILD_ENV, "1")
        .env(ID_FILE_ENV, &file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "子进程失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let child_id = std::fs::read_to_string(&file).unwrap();
    assert_ne!(
        id.trim(),
        child_id.trim(),
        "新进程的 compaction_id 不能和父进程重复"
    );
}

#[tokio::test]
async fn idle_compact_does_not_create_empty_turn() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let harness = setup(&server, ToolSet::builtin(), 0.000001).await;
    harness
        .agent
        .send(UserMessage::text("TEXT-EMPTY turn"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;
    let turn_starts = |data_dir: &std::path::Path| {
        SessionStore::open_read_only(data_dir, &harness.id)
            .unwrap()
            .events()
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::TurnStart { .. }))
            .count()
    };
    let before = turn_starts(harness.data_dir.path());
    assert_eq!(before, 1);
    let done = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        harness.agent.compact(None),
    )
    .await
    .is_ok();
    assert!(done, "空闲 compact 不能丢等待者");
    let after = turn_starts(harness.data_dir.path());
    assert_eq!(after, before, "空闲 compact 不应为空 turn 写 turn/start");
}

#[tokio::test]
async fn concurrent_manual_compacts_second_is_busy() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            delayed_sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let harness = setup(&server, ToolSet::builtin(), 0.000001).await;
    harness
        .agent
        .send(UserMessage::text("TEXT-BATCH batch"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;

    let (first, second) = tokio::join!(harness.agent.compact(None), harness.agent.compact(None));
    let outcomes = [&first, &second];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "并发两个 compact 只有一个能开始：{first:?} / {second:?}"
    );
    assert!(
        outcomes.iter().any(|outcome| matches!(
            outcome,
            Err(AgentError::ManualCompaction {
                code: ManualCompactionErrorCode::Busy,
                ..
            })
        )),
        "另一个必须 busy：{first:?} / {second:?}"
    );

    let requests = server.received_requests().await.unwrap();
    let bodies = bodies(&requests);
    let summaries = bodies
        .iter()
        .filter(|body| body.contains("compaction engine"))
        .count();
    assert_eq!(summaries, 1, "只有第一个 compact 发摘要请求");
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::CompactionStart { .. }))
            .count(),
        1,
        "只有一次压缩开始"
    );
}
#[tokio::test]
async fn second_compact_while_compacting_is_busy() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            delayed_sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let harness = setup(&server, ToolSet::builtin(), 0.000001).await;
    harness
        .agent
        .send(UserMessage::text("TEXT-DURING during"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;
    let mut events = harness.agent.subscribe();

    let agent = harness.agent.clone();
    let first = tokio::spawn(async move { agent.compact(None).await });
    loop {
        match events.recv().await.expect("事件流不应关闭") {
            AgentEvent::CompactionStarted { .. } => break,
            _ => continue,
        }
    }
    let error = harness
        .agent
        .compact(None)
        .await
        .expect_err("压缩进行中的第二个 compact 必须 busy");
    assert!(
        matches!(
            error,
            AgentError::ManualCompaction {
                code: ManualCompactionErrorCode::Busy,
                ..
            }
        ),
        "应为 busy：{error:?}"
    );
    let first = tokio::time::timeout(std::time::Duration::from_secs(15), first)
        .await
        .expect("第一个 compact 不能挂住")
        .unwrap();
    assert!(first.is_ok(), "第一个 compact 应成功：{first:?}");
    let store = SessionStore::open_read_only(harness.data_dir.path(), &harness.id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::CompactionStart { .. }))
            .count(),
        1,
        "第二个请求 busy，不能再压一次"
    );
}
#[tokio::test]
async fn manual_compact_summary_failure_is_classified() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![sse(FINAL_TEXT_SSE.to_owned()), ResponseTemplate::new(500)],
    )
    .await;
    let harness = setup(&server, ToolSet::builtin(), 0.000001).await;
    harness
        .agent
        .send(UserMessage::text("TEXT-OLD manual"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&harness.agent, 20).await;
    let outcome = harness.agent.compact(Some("cmd-classify".to_owned())).await;
    match outcome {
        Err(agent::AgentError::ManualCompaction { code, message }) => {
            assert_eq!(
                code,
                agent::ManualCompactionErrorCode::Summary,
                "摘要请求失败归 summary 码：{message}"
            );
        }
        other => panic!("期望 ManualCompaction(Summary)，实际：{other:?}"),
    }
}
