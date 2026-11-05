//! 接缝 1：同一 turn 内「注入 → prune → 压缩」的 PreStep 两阶段顺序（T006.1c 回归断言）。

mod common;

use agent::{Agent, AgentConfig, Delivery};
use common::{FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse};
use model::{ContentBlock, UserMessage};
use session::{EventKind, SessionStore, TurnEndReason};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

fn user_text_contains(kind: &EventKind, marker: &str) -> bool {
    let EventKind::UserMessage { message, .. } = kind else {
        return false;
    };
    message.content.iter().any(|block| {
        let ContentBlock::Text { text } = block else {
            return false;
        };
        text.contains(marker)
    })
}

#[tokio::test]
async fn injection_happens_before_compaction_in_same_turn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .expect(3)
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "ROOT-INSTRUCTION\n").unwrap();
    let skill = project.path().join(".agents/skills/pdf-tools/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(
        &skill,
        "---\nname: pdf-tools\ndescription: Handle PDF files.\n---\n# PDF tools\n",
    )
    .unwrap();

    let data_dir = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), project.path()).unwrap();
    let id = store.id().clone();
    let config = AgentConfig {
        compaction_threshold_ratio: 0.000001,
        compaction_retain_ratio: 0.0000001,
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
            model: model::DeepSeek::with_credentials(model_config, common::test_credential())
                .unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let events = store.events();
    let instr = events
        .iter()
        .position(|event| user_text_contains(&event.kind, "Instructions from:"))
        .expect("应注入指令");
    let skill = events
        .iter()
        .position(|event| user_text_contains(&event.kind, "<available_skills>"))
        .expect("应注入 skill 目录");
    let compaction = events
        .iter()
        .position(|event| matches!(event.kind, EventKind::CompactionStart { .. }))
        .expect("同一 turn 应触发压缩");
    assert!(instr < skill, "指令应先于 skill 注入");
    assert!(
        instr < compaction && skill < compaction,
        "注入必须发生在 compaction/start 之前：instr={instr}, skill={skill}, compaction={compaction}"
    );
    let end = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::TurnEnd { reason, .. } => Some(reason),
            _ => None,
        })
        .expect("这一轮应有 turn/end");
    assert!(
        matches!(end, TurnEndReason::Completed),
        "这一轮应做完（不是被 mock 耗尽拖成错误）：{end:?}"
    );
}
