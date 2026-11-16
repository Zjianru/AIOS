//! 记忆注入：identity 不变不重复，identity 变了追加完整替换消息。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, test_credential,
};
use agent::{Agent, Delivery, Deps, MemoryContext};
use model::UserMessage;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

#[tokio::test]
async fn memory_context_is_injected_once_then_replaced_when_identity_changes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let data_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), project.path()).unwrap();
    let version = Arc::new(AtomicU64::new(1));
    let provider_version = Arc::clone(&version);
    let config = agent::AgentConfig {
        memory_context: Some(Arc::new(move || {
            let version = provider_version.load(Ordering::SeqCst);
            Ok(Some(MemoryContext {
                identity: format!("v{version}"),
                text: format!("关于你：回答先给结论（{version}）"),
            }))
        })),
        ..isolated_config(data_dir.path())
    };
    let agent = Agent::create(
        config,
        Deps {
            model: model::DeepSeek::with_credentials(
                model_config(&server.uri()),
                test_credential(),
            )
            .unwrap(),
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();

    agent
        .send(UserMessage::text("第一句"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    version.fetch_add(1, Ordering::SeqCst);
    agent
        .send(UserMessage::text("第二句"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert!(requests.len() >= 2, "至少两次请求");
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(first.contains("回答先给结论（1）"));
    assert!(first.contains("完整替换之前的记忆"));
    assert!(
        first.contains("只读参考") && first.contains("不是用户的新要求"),
        "注入要带只读声明，模型不能把它当新指令：{first}"
    );
    assert!(
        second.contains("回答先给结论（2）"),
        "identity 变了要完整替换"
    );
    assert!(
        second.contains("完整替换之前的记忆") && second.contains("回答先给结论（2）"),
        "第二次请求要带新的完整替换内容"
    );
}

#[tokio::test]
async fn pre_compaction_diary_uses_model_and_reports_text_to_hook() {
    let server = MockServer::start().await;
    // 压缩前记一笔 → 摘要 → 主请求；三者都回同一段文本即可。
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let data_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), project.path()).unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let hook_seen = Arc::clone(&seen);
    let mut model = model_config(&server.uri());
    model.context_window = 200;
    let config = agent::AgentConfig {
        compaction_threshold_ratio: 0.01,
        compaction_retain_ratio: 0.0,
        compaction_headroom_tokens: 1,
        pre_compaction_instruction: Some("写一条简短日记".to_owned()),
        pre_compaction: Some(Arc::new(move |diary: agent::PreCompactionDiary| {
            hook_seen.lock().unwrap().push(diary.text);
            Box::pin(async { Ok(()) })
        })),
        ..isolated_config(data_dir.path())
    };
    let agent = Agent::create(
        config,
        Deps {
            model: model::DeepSeek::with_credentials(model, test_credential()).unwrap(),
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    for index in 0..3 {
        agent
            .send(
                UserMessage::text(format!("第 {index} 段足够长的历史，应该触发压缩前记一笔。")),
                Delivery::FollowUp,
            )
            .await
            .unwrap();
        agent.idle().await;
    }
    let calls = seen.lock().unwrap().clone();
    assert!(!calls.is_empty(), "压缩前钩子应被调用");
    assert!(calls.iter().all(|text| !text.trim().is_empty()));
}

#[tokio::test]
async fn pre_compaction_failure_does_not_block_compaction_or_turn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), project.path()).unwrap();
    let mut model = model_config(&server.uri());
    model.context_window = 200;
    let config = agent::AgentConfig {
        compaction_threshold_ratio: 0.01,
        compaction_retain_ratio: 0.0,
        compaction_headroom_tokens: 1,
        pre_compaction_instruction: Some("写一条简短日记".to_owned()),
        pre_compaction: Some(Arc::new(|_: agent::PreCompactionDiary| {
            Box::pin(async { Err("日记写不进去".to_owned()) })
        })),
        ..isolated_config(data_dir.path())
    };
    let agent = Agent::create(
        config,
        Deps {
            model: model::DeepSeek::with_credentials(model, test_credential()).unwrap(),
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    for index in 0..3 {
        agent
            .send(
                UserMessage::text(format!("第 {index} 段足够长的历史，应该触发压缩前记一笔。")),
                Delivery::FollowUp,
            )
            .await
            .unwrap();
        agent.idle().await;
    }
    // 失败只记录，不改变压缩/回合的结局；日志里仍有正常 turn/end。
    let log = std::fs::read_to_string(agent.log_path()).unwrap();
    assert!(log.contains("turn/end"), "失败后回合仍要正常收尾");
}

#[tokio::test]
async fn memory_is_reinjected_after_compaction() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), project.path()).unwrap();
    let mut model = model_config(&server.uri());
    model.context_window = 200;
    let config = agent::AgentConfig {
        compaction_threshold_ratio: 0.01,
        compaction_retain_ratio: 0.0,
        compaction_headroom_tokens: 1,
        memory_context: Some(Arc::new(|| {
            Ok(Some(MemoryContext {
                identity: "v-fixed".to_owned(),
                text: "关于你：稳定记忆".to_owned(),
            }))
        })),
        ..isolated_config(data_dir.path())
    };
    let agent = Agent::create(
        config,
        Deps {
            model: model::DeepSeek::with_credentials(model, test_credential()).unwrap(),
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    for index in 0..3 {
        agent
            .send(
                UserMessage::text(format!("第 {index} 段足够长的历史，测试压缩后重新注入。")),
                Delivery::FollowUp,
            )
            .await
            .unwrap();
        agent.idle().await;
    }
    let requests = server.received_requests().await.unwrap();
    let injected = requests
        .iter()
        .filter(|request| String::from_utf8_lossy(&request.body).contains("关于你：稳定记忆"))
        .count();
    assert!(
        injected >= 2,
        "压缩后下一步必须重新注入，实际 {injected} 次"
    );
}
