//! 记忆注入：回合开始/压缩后注入；同一 turn 内 identity 变化先不补，新注入追加为新消息，旧前缀不动。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::common::{
    FINAL_TEXT_SSE, PARTIAL_TEXT_SSE, ScriptedSseServer, SequenceResponder, SseScript,
    isolated_config, model_config, sse, test_credential,
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
                ids: Vec::new(),
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
    assert!(
        first.contains("只读参考") && first.contains("不是用户消息、不是新任务"),
        "注入要带只读声明，模型不能把它当新指令：{first}"
    );
    assert!(
        !first.contains("完整替换"),
        "注入里不能带指令式引言：{first}"
    );
    assert!(
        second.contains("回答先给结论（2）"),
        "identity 变了要换新内容"
    );
    assert!(
        second.contains("只读参考") && second.contains("回答先给结论（2）"),
        "下一轮开始要带新的记忆（只读说明）"
    );
}

#[tokio::test]
async fn memory_context_is_not_reinjected_inside_a_turn_after_identity_changes() {
    let server = MockServer::start().await;
    // step1 先让模型读一个文件（同一 turn 继续到 step2），再收尾；下一轮再回一条。
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(crate::common::tool_calls_sse(&[("call-read", "read")])),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let data_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), project.path()).unwrap();
    let calls = Arc::new(AtomicU64::new(0));
    let provider_calls = Arc::clone(&calls);
    let config = agent::AgentConfig {
        memory_context: Some(Arc::new(move || {
            let call = provider_calls.fetch_add(1, Ordering::SeqCst);
            let version = if call == 0 { 1 } else { 2 };
            Ok(Some(MemoryContext {
                identity: format!("v{version}"),
                text: format!("关于你：回答先给结论（{version}）"),
                ids: Vec::new(),
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
    agent
        .send(UserMessage::text("第二句"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect();
    assert!(
        bodies.len() >= 3,
        "至少：turn1 两步 + turn2 一步，实际 {}",
        bodies.len()
    );
    const MARKER: &str = "以下内容是已保存的记忆，只读参考";
    for body in &bodies {
        assert_eq!(
            body.matches(MARKER).count(),
            1,
            "每个请求里只能有一条记忆注入：{body}"
        );
        assert!(!body.contains("完整替换"), "注入不再带指令式引言：{body}");
    }
    // turn1 的第二步：identity 变了也不在同一 turn 里补注入。
    assert!(bodies[1].contains("回答先给结论（1）"));
    assert!(
        !bodies[1].contains("回答先给结论（2）"),
        "同一 turn 内 identity 变化先不注入：{}",
        bodies[1]
    );
    // 下一个用户回合开始才补新记忆。
    assert!(bodies[2].contains("回答先给结论（2）"));
}

#[tokio::test]
async fn memory_context_replacement_keeps_the_conversation_between_two_injections() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let data_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), project.path()).unwrap();
    let calls = Arc::new(AtomicU64::new(0));
    let provider_calls = Arc::clone(&calls);
    let config = agent::AgentConfig {
        memory_context: Some(Arc::new(move || {
            let call = provider_calls.fetch_add(1, Ordering::SeqCst);
            let version = call + 1;
            Ok(Some(MemoryContext {
                identity: format!("v{version}"),
                text: format!("关于你：回答先给结论（{version}）"),
                ids: Vec::new(),
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

    for text in ["第一句", "第二句", "第三句"] {
        agent
            .send(UserMessage::text(text), Delivery::FollowUp)
            .await
            .unwrap();
        agent.idle().await;
    }

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect();
    assert_eq!(bodies.len(), 3, "三次用户回合三条请求");
    // 追加语义：每次整份新版都是新消息，旧注入留在日志里，不回写前缀。
    let first = &bodies[0];
    assert_eq!(
        first.matches("以下内容是已保存的记忆，只读参考").count(),
        1,
        "第一回合只有首条注入：{first}"
    );
    assert_eq!(
        bodies[1].matches("记忆有新版本，以下是整份最新版").count(),
        1,
        "第二回合追加一条新版：{}",
        bodies[1]
    );
    let last = &bodies[2];
    assert!(last.contains("第一句"), "第一回合的用户话还在：{last}");
    assert!(last.contains("第二句"), "第二回合的用户话还在：{last}");
    assert!(
        last.matches("工具都跑完了").count() >= 2,
        "两回合的助手回答都在：{last}"
    );
    assert!(last.contains("回答先给结论（3）"), "第三条注入生效：{last}");
    assert!(
        last.contains("回答先给结论（1）"),
        "旧注入留在前缀里，不替换：{last}"
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
                ids: Vec::new(),
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

#[tokio::test]
async fn memory_delivery_is_recorded_only_after_a_successful_turn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
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
    let delivered = Arc::new(std::sync::Mutex::new(Vec::<(u64, Vec<String>)>::new()));
    let delivery_log = Arc::clone(&delivered);
    let config = agent::AgentConfig {
        memory_context: Some(Arc::new(move || {
            let version = provider_version.load(Ordering::SeqCst);
            Ok(Some(MemoryContext {
                identity: format!("v{version}"),
                text: format!("关于你：第 {version} 版"),
                ids: vec![format!("mem-{version}")],
            }))
        })),
        memory_delivery: Some(Arc::new(move |delivery| {
            delivery_log
                .lock()
                .unwrap()
                .push((delivery.turn, delivery.ids));
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
    assert_eq!(
        delivered.lock().unwrap().as_slice(),
        [(1, vec!["mem-1".to_owned()])],
        "TurnEnd succeeded 才落 sent"
    );

    // 同一 identity 的下一回合不重复注入，也就不重复落账。
    agent
        .send(UserMessage::text("第二句"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    assert_eq!(delivered.lock().unwrap().len(), 1);

    version.store(2, Ordering::SeqCst);
    agent
        .send(UserMessage::text("第三句"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    assert_eq!(
        delivered.lock().unwrap().last().unwrap(),
        &(3, vec!["mem-2".to_owned()]),
        "版本更新后整份追加并落账"
    );
}

#[tokio::test]
async fn aborted_turn_does_not_record_delivery_and_next_turn_resends() {
    let mut server = ScriptedSseServer::start(vec![
        SseScript {
            frames: PARTIAL_TEXT_SSE.to_owned(),
            hold: true,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let data_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), project.path()).unwrap();
    let delivered = Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let delivery_log = Arc::clone(&delivered);
    let config = agent::AgentConfig {
        memory_context: Some(Arc::new(|| {
            Ok(Some(MemoryContext {
                identity: "v1".to_owned(),
                text: "关于你：取消也要重发".to_owned(),
                ids: vec!["mem-abort".to_owned()],
            }))
        })),
        memory_delivery: Some(Arc::new(move |delivery| {
            delivery_log.lock().unwrap().push(delivery.ids);
        })),
        ..isolated_config(data_dir.path())
    };
    let agent = Agent::create(
        config,
        Deps {
            model: model::DeepSeek::with_credentials(
                model_config(&server.base_url()),
                test_credential(),
            )
            .unwrap(),
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("取消这一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    server.wait_ready().await;
    agent.cancel_with(session::AbortCause::User);
    loop {
        match events.recv().await {
            Ok(agent::AgentEvent::TurnEnded { .. }) => break,
            Ok(_) => {}
            Err(error) => panic!("事件通道关闭: {error}"),
        }
    }
    server.release();
    agent.idle().await;
    assert!(delivered.lock().unwrap().is_empty(), "失败/取消不落送达账");

    agent
        .send(UserMessage::text("下一轮成功"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    assert_eq!(
        delivered.lock().unwrap().as_slice(),
        [vec!["mem-abort".to_owned()]],
        "下一用户回合重发整份并落账"
    );
}

#[tokio::test]
async fn restart_derives_delivery_state_from_the_log_and_does_not_reinject() {
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
    let session_id = store.id().clone();
    let config = || agent::AgentConfig {
        memory_context: Some(Arc::new(|| {
            Ok(Some(MemoryContext {
                identity: "v1".to_owned(),
                text: "关于你：重启后别重发".to_owned(),
                ids: vec!["mem-restart".to_owned()],
            }))
        })),
        ..isolated_config(data_dir.path())
    };
    let agent = Agent::create(
        config(),
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
        .send(UserMessage::text("重启前"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    drop(agent);

    let store = session::SessionStore::open(data_dir.path(), &session_id).unwrap();
    let resumed = Agent::resume(
        session_id,
        config(),
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
    resumed
        .send(UserMessage::text("重启后"), Delivery::FollowUp)
        .await
        .unwrap();
    resumed.idle().await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let after = String::from_utf8(requests[1].body.clone()).unwrap();
    assert_eq!(
        after.matches("关于你：重启后别重发").count(),
        1,
        "重启后从日志派生 identity，不重复注入：{after}"
    );
}

#[tokio::test]
async fn flashback_is_injected_once_with_ids_and_not_repeated() {
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
    let delivered = Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let delivery_log = Arc::clone(&delivered);
    let config = agent::AgentConfig {
        memory_context: Some(Arc::new(|| {
            Ok(Some(MemoryContext {
                identity: "v1".to_owned(),
                text: "关于你：先给结论".to_owned(),
                ids: vec!["mem-context".to_owned()],
            }))
        })),
        memory_flashback: Some(Arc::new(|message: &str| {
            if !message.contains("Linger") {
                return Ok(None);
            }
            Ok(Some(agent::MemoryFlashback {
                text: "<memory_flashback>这让我想起旧事 [mem-old]</memory_flashback>".to_owned(),
                ids: vec!["mem-old".to_owned()],
            }))
        })),
        memory_delivery: Some(Arc::new(move |delivery| {
            delivery_log.lock().unwrap().push(delivery.ids);
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
        .send(UserMessage::text("还记得 Linger 吗"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    agent
        .send(UserMessage::text("Linger 后来呢"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    assert_eq!(
        first.matches("<memory_flashback>").count(),
        1,
        "首问送一次回忆：{first}"
    );
    assert!(first.contains("mem-old"), "回忆段带 id：{first}");
    assert_eq!(
        second.matches("<memory_flashback>").count(),
        1,
        "同一段回忆不重复送：{second}"
    );
    let flash_at = first.find("<memory_flashback>").expect("回忆段");
    let flash_open = first[..flash_at]
        .rfind("<system-reminder>")
        .expect("回忆在框内");
    assert!(
        !first[flash_open..flash_at].contains("</system-reminder>"),
        "回忆必须在同一个框内：{first}"
    );
    assert!(
        delivered
            .lock()
            .unwrap()
            .iter()
            .any(|ids| ids.contains(&"mem-old".to_owned())),
        "回忆 id 也进本轮送达账"
    );
}
