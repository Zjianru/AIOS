//! T006.15：唯一的产品组装入口 Agent::open（票面交付：组装只在一处、CLI chat/run 共用）。
//!
//! DSH 对应物：packages/bundle/base/cordis.patch.yml:516-517 的 agent-loop 组合、
//! packages/core/agent-loop/src/index.ts 的 create/resume 装配（:557/:668/:717/:754）与初始化
//! 失败不唤醒（:601-647）。行为不变部分是 characterization；「配置非法要先于会话物化失败」是
//! 本票的可观测行为改动（DSH docs/user/develop/basic/config.md 的「Fail loudly on invalid
//! configuration」是同一个失败时机口径）。

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, wait_idle,
};
use agent::{Agent, AgentError, AgentOpenOptions, Delivery};
use model::UserMessage;
use session::SessionStore;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

async fn mount(server: &MockServer, responses: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            responses.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

fn options(
    data_dir: &std::path::Path,
    cwd: &std::path::Path,
    base_url: &str,
    resume: Option<session::SessionId>,
) -> AgentOpenOptions {
    AgentOpenOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        resume,
        model: model_config(base_url),
        config: isolated_config(data_dir),
        credentials: Some(crate::common::test_credential()),
        append_hook_factory: None,
        extra_tools: Vec::new(),
    }
}

fn count_turns(data_dir: &std::path::Path, id: &session::SessionId) -> usize {
    SessionStore::open_read_only(data_dir, id)
        .unwrap()
        .events()
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::TurnStart { .. }))
        .count()
}

#[tokio::test]
async fn open_assembles_session_then_resumes_through_same_entry() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![FINAL_TEXT_SSE.to_owned(), FINAL_TEXT_SSE.to_owned()],
    )
    .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    // 新建：一处组装（model/tools/store → create），返回后不唤醒任何 turn。
    let agent = Agent::open(options(data_dir.path(), cwd.path(), &server.uri(), None))
        .await
        .expect("合法配置应组装成功");
    let id = agent.id().clone();
    assert!(
        data_dir
            .path()
            .join("sessions")
            .join(format!("{id}.jsonl"))
            .exists(),
        "组装要把会话日志建在 data-dir/sessions/ 下"
    );
    assert_eq!(
        count_turns(data_dir.path(), &id),
        0,
        "open 只组装，不启动 turn"
    );
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    assert_eq!(
        count_turns(data_dir.path(), &id),
        1,
        "组装后的 agent 可正常跑一轮"
    );
    drop(agent);

    // 续接：同一个入口（resume 分支）打开同一会话，日志与 id 都保持。
    let resumed = Agent::open(options(
        data_dir.path(),
        cwd.path(),
        &server.uri(),
        Some(id.clone()),
    ))
    .await
    .expect("续接应组装成功");
    assert_eq!(resumed.id(), &id);
    resumed
        .send(UserMessage::text("续接一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 10).await;
    assert_eq!(
        count_turns(data_dir.path(), &id),
        2,
        "续接后的 agent 可正常跑"
    );

    // 组装只喂一次模型：两个 turn 各一次请求。
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn open_rejects_invalid_config_before_creating_session() {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut config = isolated_config(data_dir.path());
    config.max_parallel_tool_calls = 0;
    let error = match Agent::open(AgentOpenOptions {
        data_dir: data_dir.path().to_path_buf(),
        cwd: cwd.path().to_path_buf(),
        resume: None,
        model: model_config("http://127.0.0.1:1"),
        config,
        credentials: Some(crate::common::test_credential()),
        append_hook_factory: None,
        extra_tools: Vec::new(),
    })
    .await
    {
        Ok(_) => panic!("非法配置必须返回 Err"),
        Err(error) => error,
    };
    assert!(
        matches!(&error, AgentError::InvalidConfig { field, .. } if *field == "max_parallel_tool_calls"),
        "错误要是 InvalidConfig：{error:?}"
    );
    let sessions = data_dir.path().join("sessions");
    let created = std::fs::read_dir(&sessions)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    assert!(
        !created,
        "配置非法时组装不能先物化会话文件（DSH：配置在装载时失败，创建失败不留下半装配状态）"
    );
    // 文件系统里也没有残留的空日志。
    let leftover = std::fs::read_dir(data_dir.path())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(
        leftover.is_empty(),
        "data-dir 里不应有失败组装的残留：{leftover:?}"
    );
}

#[tokio::test]
async fn open_resume_missing_session_returns_store_error() {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let missing = session::SessionId::generate();
    let error = match Agent::open(options(
        data_dir.path(),
        cwd.path(),
        "http://127.0.0.1:1",
        Some(missing),
    ))
    .await
    {
        Ok(_) => panic!("续接不存在的会话必须 Err"),
        Err(error) => error,
    };
    assert!(
        matches!(error, AgentError::Store(_)),
        "应是存储错误而不是 panic：{error:?}"
    );
}
