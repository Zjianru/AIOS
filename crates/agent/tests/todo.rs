//! 接缝 1：todo_write 事件与续接恢复（测试清单第 7 条）。

mod common;

use agent::{Agent, Delivery};
use common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse,
    tool_calls_with_inputs_sse,
};
use model::UserMessage;
use serde_json::json;
use session::{EventKind, SessionStore, last_todos};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

fn todo_write_call(content: &str) -> String {
    tool_calls_with_inputs_sse(&[(
        "call_0",
        "todo_write",
        json!({ "todos": [{ "content": content, "status": "in_progress" }] }),
    )])
}

#[tokio::test]
async fn todo_write_appends_event_and_stays_in_history() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(todo_write_call("step one")),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, id) = common::setup_agent(&server.uri(), ToolSet::builtin()).await;

    agent
        .send(UserMessage::text("plan it"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let writes: Vec<&str> = store
        .events()
        .iter()
        .filter(|event| matches!(event.kind, EventKind::TodoWrite { .. }))
        .map(|event| event.kind.type_name())
        .collect();
    assert_eq!(writes, vec!["todo/write"], "应写入一条 todo/write 事件");
    assert_eq!(
        last_todos(store.events()),
        Some(vec![session::TodoItem {
            content: "step one".to_owned(),
            status: session::TodoStatus::InProgress,
        }])
    );

    // 下一步请求里模型能看到自己刚才的 todo_write 调用与计数结果。
    let requests = server.received_requests().await.unwrap();
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(second.contains("step one"), "历史里应带 todo_write 参数");
    assert!(
        second.contains("Updated todo list: 0 pending, 1 in progress, 0 completed."),
        "历史里应带工具结果计数"
    );
}

#[tokio::test]
async fn resume_restores_todo_list_and_replacement_wins() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(todo_write_call("step one")),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(todo_write_call("step two")),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, id) = common::setup_agent(&server.uri(), ToolSet::builtin()).await;
    agent
        .send(UserMessage::text("first"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    drop(agent);

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        last_todos(store.events()),
        Some(vec![session::TodoItem {
            content: "step one".to_owned(),
            status: session::TodoStatus::InProgress,
        }]),
        "首轮之后日志里的当前列表"
    );

    // 续接：从日志恢复，下一轮请求历史里带着原列表。
    let resumed = Agent::resume(
        id.clone(),
        isolated_config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::new(model_config(&server.uri())).unwrap(),
            tools: ToolSet::builtin(),
            store: SessionStore::open(data_dir.path(), &id).unwrap(),
        },
    )
    .await
    .expect("续接应成功");
    resumed
        .send(UserMessage::text("second"), Delivery::FollowUp)
        .await
        .unwrap();
    resumed.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4, "续接后应能继续跑两轮请求");
    let resumed_first = String::from_utf8(requests[2].body.clone()).unwrap();
    assert!(
        resumed_first.contains("step one"),
        "续接后的请求历史里应还有原 todo 列表"
    );

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let writes: Vec<&session::TodoItem> = store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TodoWrite { todos, .. } => todos.first(),
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 2, "两次写入都进日志");
    assert_eq!(
        writes[1].content, "step two",
        "续接后写入的新列表替换旧列表"
    );
    assert_eq!(
        last_todos(store.events()).map(|todos| todos[0].content.clone()),
        Some("step two".to_owned()),
        "回放时后写覆盖先写"
    );
}

/// T006.12：allowParallelInProgress=false 从 AgentConfig 透传到内置 todo_write：
/// 工具描述改成「只留一个」，两个 in_progress 被拒，且不写 todo/write 事件。
#[tokio::test]
async fn single_active_policy_from_config_rejects_two_active() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "todo_write",
                json!({"todos": [
                    {"content": "a", "status": "in_progress"},
                    {"content": "b", "status": "in_progress"}
                ]}),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let mut config = isolated_config(data_dir.path());
    config.allow_parallel_in_progress = false;
    let agent = Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::new(model_config(&server.uri())).unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    agent
        .send(UserMessage::text("plan it"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(
        first.contains("keep exactly one todo"),
        "工具描述应改为单活跃策略：{first}"
    );
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(
        second.contains("at most one task may be in_progress (got 2)"),
        "工具结果应是 DSH 错误：{second}"
    );
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert!(
        !store
            .events()
            .iter()
            .any(|event| matches!(event.kind, EventKind::TodoWrite { .. })),
        "被拒的写入不应落 todo/write"
    );
}
