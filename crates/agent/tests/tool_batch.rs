//! 接缝 1：工具批执行的并行上限、顺序和独占屏障（测试清单第 3 条）。

mod common;

use std::sync::Arc;

use agent::{Agent, AgentConfig, Delivery, Deps};
use common::{
    FINAL_TEXT_SSE, SequenceResponder, ToolEvent, gated, model_config, sse, tool_calls_sse,
};
use model::{DeepSeek, UserMessage};
use session::SessionStore;
use tokio::sync::{Semaphore, mpsc};
use tools::{Tool, ToolConcurrency, ToolSet};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

struct Fixture {
    agent: Agent,
    started: mpsc::UnboundedReceiver<ToolEvent>,
    permits: Vec<(&'static str, Arc<Semaphore>)>,
    session_id: session::SessionId,
    data_dir: tempfile::TempDir,
}

async fn setup(
    names: &[(&'static str, ToolConcurrency)],
    max_parallel_tool_calls: usize,
) -> (Fixture, MockServer) {
    let server = MockServer::start().await;
    let ids: Vec<String> = (0..names.len())
        .map(|index| format!("call_{index}"))
        .collect();
    let calls: Vec<(&str, &str)> = ids
        .iter()
        .map(String::as_str)
        .zip(names.iter().map(|(name, _)| *name))
        .collect();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_sse(&calls)),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let session_id = store.id().clone();
    let (started_tx, started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools: Vec<Box<dyn Tool>> = names
        .iter()
        .map(|(name, mode)| gated(name, *mode, &started_tx, &mut permits))
        .collect();
    let agent = Agent::create(
        AgentConfig {
            max_parallel_tool_calls,
            ..common::isolated_config(data_dir.path())
        },
        Deps {
            model: DeepSeek::new(model_config(&server.uri())).unwrap(),
            tools: ToolSet::new(tools),
            store,
        },
    )
    .await
    .unwrap();
    (
        Fixture {
            agent,
            started,
            permits,
            session_id,
            data_dir,
        },
        server,
    )
}

fn note(event: &ToolEvent, concurrency: &mut i64, max_concurrency: &mut i64) {
    match event {
        ToolEvent::Started(_) => {
            *concurrency += 1;
            *max_concurrency = (*max_concurrency).max(*concurrency);
        }
        ToolEvent::Finished(_) => *concurrency -= 1,
    }
}

fn permit(fixture: &Fixture, name: &str) -> Arc<Semaphore> {
    fixture
        .permits
        .iter()
        .find(|(tool, _)| *tool == name)
        .map(|(_, permit)| Arc::clone(permit))
        .unwrap_or_else(|| panic!("没有名为 {name} 的测试工具"))
}

async fn next_event(fixture: &mut Fixture) -> ToolEvent {
    fixture
        .started
        .recv()
        .await
        .expect("测试工具的事件通道被关闭")
}

fn result_order(events: &[session::Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. } => message.tool_call_id.clone(),
            _ => None,
        })
        .collect()
}

fn call_order(events: &[session::Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn batch_limits_ordering_and_exclusive_barrier() {
    let (mut fixture, _server) = setup(
        &[
            ("p1", ToolConcurrency::Parallel),
            ("p2", ToolConcurrency::Parallel),
            ("e1", ToolConcurrency::Exclusive),
            ("p3", ToolConcurrency::Parallel),
            ("p4", ToolConcurrency::Parallel),
        ],
        2,
    )
    .await;
    let agent = fixture.agent.clone();
    let task = tokio::spawn(async move {
        agent
            .send(UserMessage::text("跑工具"), Delivery::FollowUp)
            .await
            .unwrap();
        agent.idle().await;
    });

    let mut concurrency = 0i64;
    let mut max_concurrency = 0i64;

    // p1、p2 同时在跑：并发到 2，先放 p2（结果顺序不能跟着完成顺序走）。
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Started("p1".to_owned()));
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Started("p2".to_owned()));
    assert_eq!(concurrency, 2, "并发上限 2");
    permit(&fixture, "p2").add_permits(1);
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Finished("p2".to_owned()));
    // p2 结束后，e1 是独占屏障，不能先开；p1 还占着槽位。
    permit(&fixture, "p1").add_permits(1);
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Finished("p1".to_owned()));
    // 屏障：e1 单独跑
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Started("e1".to_owned()));
    assert_eq!(concurrency, 1, "独占调用单独跑");
    permit(&fixture, "e1").add_permits(1);
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Finished("e1".to_owned()));
    // e1 结束之后才轮到 p3、p4，且两个可以同时跑
    let mut later = Vec::new();
    for _ in 0..2 {
        let event = next_event(&mut fixture).await;
        note(&event, &mut concurrency, &mut max_concurrency);
        later.push(event);
    }
    later.sort_by_key(|event| format!("{event:?}"));
    assert_eq!(
        later,
        vec![
            ToolEvent::Started("p3".to_owned()),
            ToolEvent::Started("p4".to_owned())
        ]
    );
    assert_eq!(concurrency, 2);
    permit(&fixture, "p4").add_permits(1);
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Finished("p4".to_owned()));
    permit(&fixture, "p3").add_permits(1);
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Finished("p3".to_owned()));
    assert_eq!(max_concurrency, 2, "同时在跑的工具不超过上限 2");

    task.await.expect("agent 驱动任务失败");
    let reopened = SessionStore::open(fixture.data_dir.path(), &fixture.session_id).unwrap();
    let events = reopened.events();
    assert_eq!(
        call_order(events),
        vec!["call_0", "call_1", "call_2", "call_3", "call_4"],
        "tool/call 按模型顺序记录"
    );
    assert_eq!(
        result_order(events),
        vec!["call_0", "call_1", "call_2", "call_3", "call_4"],
        "tool/result 按模型顺序提交，与完成顺序无关"
    );
}

#[tokio::test]
async fn agent_config_rejects_zero_parallelism() {
    // 协调线加账：非法配置要返回 AgentError，不能 panic 在驱动任务里、把 idle() 挂死。
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let created = Agent::create(
        AgentConfig {
            max_parallel_tool_calls: 0,
            ..common::isolated_config(data_dir.path())
        },
        Deps {
            model: DeepSeek::new(model_config("http://127.0.0.1:1")).unwrap(),
            tools: ToolSet::empty(),
            store,
        },
    )
    .await;
    assert!(
        matches!(created, Err(agent::AgentError::InvalidConfig { .. })),
        "max_parallel_tool_calls 为 0 时要返回 AgentError::InvalidConfig"
    );
}

#[tokio::test]
async fn batch_limit_one_serializes() {
    let (mut fixture, _server) = setup(
        &[
            ("q1", ToolConcurrency::Parallel),
            ("q2", ToolConcurrency::Parallel),
        ],
        1,
    )
    .await;
    let agent = fixture.agent.clone();
    let task = tokio::spawn(async move {
        agent
            .send(UserMessage::text("跑工具"), Delivery::FollowUp)
            .await
            .unwrap();
        agent.idle().await;
    });

    let mut concurrency = 0i64;
    let mut max_concurrency = 0i64;

    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Started("q1".to_owned()));
    permit(&fixture, "q1").add_permits(1);
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Finished("q1".to_owned()));
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Started("q2".to_owned()));
    permit(&fixture, "q2").add_permits(1);
    let event = next_event(&mut fixture).await;
    note(&event, &mut concurrency, &mut max_concurrency);
    assert_eq!(event, ToolEvent::Finished("q2".to_owned()));
    assert_eq!(max_concurrency, 1, "上限设成 1 就是串行");

    task.await.expect("agent 驱动任务失败");
}
