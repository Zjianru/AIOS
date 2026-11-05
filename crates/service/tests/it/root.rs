//! T006.16b：Kind 根会话四态/状态文件原子写、重启 pending inbox 自动领取（DoD6）、
//! durable rpcId 跨重启幂等。
//!
//! 需求来源：docs/02-Kind与协作.md 第 1 节（一个用户一条根会话；缺失/损坏报告、不静默新建）、
//! 06 篇恢复链、strategy §4；DSH 对照：session-controller 的 prompt requestId → user/message
//! source.rpcId（types.ts:397/589、commands.ts:540-556 hasPromptRequest）。

use agent::{AgentConfig, Delivery};
use model::{DeepSeekConfig, Effort};
use service::{
    DeliveryMode, RootError, RootSessionOptions, RootStatus, SendRequest, ServiceHost,
    open_root_session,
};
use session::SessionStore;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FINAL_TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_final","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":9,"output_tokens":2}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"完成。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

fn sse() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(FINAL_TEXT_SSE.to_owned())
}

async fn mount(server: &MockServer) {
    let template = sse();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |_: &wiremock::Request| template.clone())
        .mount(server)
        .await;
}

/// T008.F9：测试凭证来源（产品不再有固定 key 构造入口，测试显式注入）。
struct TestCredential;

impl model::CredentialSource for TestCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok("test-key".to_owned())
    }
}

fn test_credential() -> std::sync::Arc<dyn model::CredentialSource> {
    std::sync::Arc::new(TestCredential)
}

fn options(
    data_dir: &std::path::Path,
    cwd: &std::path::Path,
    base_url: &str,
) -> RootSessionOptions {
    let config = AgentConfig {
        instruction_roots: vec![data_dir.join("agents-home")],
        skill_roots: vec![data_dir.join("skills-home")],
        ..AgentConfig::default()
    };
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        model: DeepSeekConfig {
            base_url: base_url.to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: model::DEFAULT_CONTEXT_WINDOW,
            effort: Effort::High,
        },
        config,
        credentials: Some(test_credential()),
        identity: None,
        model_factory: None,
        append_hook_factory: None,
    }
}

fn sessions_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("sessions")
}

fn session_file_count(data_dir: &std::path::Path) -> usize {
    std::fs::read_dir(sessions_dir(data_dir))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
                .count()
        })
        .unwrap_or(0)
}

fn turn_count(data_dir: &std::path::Path, id: &session::SessionId) -> usize {
    SessionStore::open_read_only(data_dir, id)
        .unwrap()
        .events()
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::TurnStart { .. }))
        .count()
}

#[tokio::test]
async fn root_created_on_first_start_and_recorded_atomically() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let root = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .expect("首次启动应建根会话");
    assert_eq!(root.status, RootStatus::Created);
    assert!(!root.claimed_pending, "新建时没有 pending inbox 可领");
    let state_path = service::RootSession::state_path(data_dir.path());
    assert!(state_path.exists(), "状态文件要落在数据目录");
    let record = std::fs::read_to_string(&state_path).unwrap();
    let record: serde_json::Value = serde_json::from_str(&record).unwrap();
    assert_eq!(record["sessionId"], root.session_id.as_str());
    assert!(
        !std::path::Path::new(&format!("{}.tmp", state_path.display())).exists(),
        "原子写完成后不应留下临时文件"
    );
    assert_eq!(session_file_count(data_dir.path()), 1);
}

#[tokio::test]
async fn root_restored_when_record_and_log_exist() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let first = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .unwrap();
    let id = first.session_id.clone();
    drop(first);

    let second = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .expect("记过且日志在，应找回同一条根会话");
    assert_eq!(second.status, RootStatus::Restored);
    assert_eq!(second.session_id, id);
    assert_eq!(session_file_count(data_dir.path()), 1, "不能新建第二条会话");
}

#[tokio::test]
async fn root_recorded_but_missing_log_reports_and_does_not_create() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let root = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .unwrap();
    let id = root.session_id.clone();
    drop(root);
    // 记过 id，但日志被删掉（恢复链：缺失要报告，不静默新建）。
    let log = data_dir.path().join("sessions").join(format!("{id}.jsonl"));
    std::fs::remove_file(&log).unwrap();
    let before = session_file_count(data_dir.path());

    let error = match open_root_session(options(data_dir.path(), cwd.path(), &server.uri())).await {
        Ok(_) => panic!("记过但日志缺失必须报错"),
        Err(error) => error,
    };
    assert!(
        matches!(&error, RootError::RecordedSessionMissing { session_id, .. } if session_id == id.as_str()),
        "应是 RecordedSessionMissing：{error:?}"
    );
    assert_eq!(
        session_file_count(data_dir.path()),
        before,
        "报错时不能新建根会话"
    );
}

#[tokio::test]
async fn root_corrupt_state_file_reports_and_does_not_create() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        service::RootSession::state_path(data_dir.path()),
        b"{ not valid json ",
    )
    .unwrap();

    let error = match open_root_session(options(data_dir.path(), cwd.path(), &server.uri())).await {
        Ok(_) => panic!("状态文件损坏必须报错"),
        Err(error) => error,
    };
    assert!(
        matches!(error, RootError::StateCorrupt { .. }),
        "应是 StateCorrupt：{error:?}"
    );
    assert_eq!(session_file_count(data_dir.path()), 0, "损坏时不能静默新建");
}

#[tokio::test]
async fn root_half_written_tmp_does_not_break_existing_state() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let first = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .unwrap();
    let id = first.session_id.clone();
    drop(first);
    // 模拟上一次写状态文件时崩溃在 rename 之前：半个临时文件留着，目标文件完好。
    std::fs::write(
        std::path::Path::new(&format!(
            "{}.tmp",
            service::RootSession::state_path(data_dir.path()).display()
        )),
        b"{ half",
    )
    .unwrap();

    let second = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .expect("半写的临时文件不能破坏已提交的状态");
    assert_eq!(second.status, RootStatus::Restored);
    assert_eq!(second.session_id, id);
}

#[tokio::test]
async fn restart_claims_pending_inbox_exactly_once() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    // 第一次启动：建根会话；投一条 Inject（进下一步队列但不唤醒，模拟重启前没跑完的输入）。
    let first = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .unwrap();
    let id = first.session_id.clone();
    assert_eq!(first.status, RootStatus::Created);
    first
        .agent
        .send(model::UserMessage::text("重启后要做的事"), Delivery::Inject)
        .await
        .unwrap();
    assert!(!first.agent.is_running(), "Inject 不唤醒空闲 loop");
    drop(first);

    // 第二次启动（重启）：找回根会话，自动领取 pending inbox 并转成一轮，恰好一次。
    let second = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .expect("重启应找回根会话");
    assert_eq!(second.status, RootStatus::Restored);
    assert_eq!(second.session_id, id);
    assert!(second.claimed_pending, "重启应自动领取 pending inbox");
    second.agent.idle().await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "领取后只跑一次模型请求"
    );
    assert_eq!(turn_count(data_dir.path(), &id), 1, "重启后续跑恰好一轮");
    let bodies: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect();
    assert!(
        bodies[0].contains("重启后要做的事"),
        "领取的输入要进到这一轮"
    );
    drop(second);

    // 第三次启动：队列已消费，不再领取、不再跑。
    let third = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .unwrap();
    assert!(!third.claimed_pending, "没有 pending inbox 时不领取");
    third.agent.idle().await;
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(turn_count(data_dir.path(), &id), 1);
}

#[tokio::test]
async fn durable_rpc_id_is_idempotent_across_restart() {
    let server = MockServer::start().await;
    mount(&server).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let first = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .unwrap();
    let id = first.session_id.clone();
    let service = ServiceHost::new(first.agent.clone());
    let sent = service
        .send(SendRequest {
            request_id: "req-dup-restart".to_owned(),
            content: "同一条请求".to_owned(),
            delivery: DeliveryMode::FollowUp,
        })
        .await;
    assert!(sent.is_ok(), "发送应成功：{sent:?}");
    let sent = sent.unwrap();
    assert!(sent.accepted && !sent.duplicate);
    first.agent.idle().await;
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    drop(service);
    drop(first);

    // 重启后同一个 requestId 再发：durable rpcId 应判重，不产生第二条消息/第二次请求。
    let second = open_root_session(options(data_dir.path(), cwd.path(), &server.uri()))
        .await
        .unwrap();
    let service = ServiceHost::new(second.agent.clone());
    let duplicate = service
        .send(SendRequest {
            request_id: "req-dup-restart".to_owned(),
            content: "同一条请求".to_owned(),
            delivery: DeliveryMode::FollowUp,
        })
        .await;
    assert!(duplicate.is_ok(), "重复发送应返回受理回执：{duplicate:?}");
    let duplicate = duplicate.unwrap();
    assert!(duplicate.duplicate, "重启后同 requestId 要标 duplicate");
    second.agent.idle().await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "重复 id 不能触发第二次模型请求"
    );
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let user_messages = store
        .events()
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::UserMessage { .. }))
        .count();
    assert_eq!(user_messages, 1, "日志里只能有一条这条请求");

    // 换一个 id 正常进。
    let fresh = service
        .send(SendRequest {
            request_id: "req-new".to_owned(),
            content: "新请求".to_owned(),
            delivery: DeliveryMode::FollowUp,
        })
        .await
        .unwrap();
    assert!(!fresh.duplicate);
    second.agent.idle().await;
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert_eq!(turn_count(data_dir.path(), &id), 2);
}
