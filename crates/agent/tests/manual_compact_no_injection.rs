//! 接缝 1：手动 /compact 不能顺带注入指令/skill（T006.1c 回归断言）。

mod common;

use agent::Delivery;
use common::{FINAL_TEXT_SSE, SequenceResponder, setup_agent_custom, sse};
use model::{ContentBlock, UserMessage};
use session::{EventKind, SessionStore};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

fn user_message_count(store: &SessionStore, marker: &str) -> usize {
    store
        .events()
        .iter()
        .filter(|event| {
            let EventKind::UserMessage { message, .. } = &event.kind else {
                return false;
            };
            message.content.iter().any(|block| {
                let ContentBlock::Text { text } = block else {
                    return false;
                };
                text.contains(marker)
            })
        })
        .count()
}

#[tokio::test]
async fn manual_compact_does_not_inject_instructions_or_skills() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
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

    let (agent, data_dir, id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let first =
        String::from_utf8(server.received_requests().await.unwrap()[0].body.clone()).unwrap();
    assert!(first.contains("ROOT-INSTRUCTION"));
    assert!(first.contains("Handle PDF files."));

    // 首轮之后文件变化；手动 /compact 的 PreStep 必须跳过指令/skill，不能借此注入。
    std::fs::write(project.path().join("AGENTS.md"), "ROOT-UPDATED\n").unwrap();
    std::fs::write(
        &skill,
        "---\nname: pdf-tools\ndescription: UPDATED-SKILL-DESCRIPTION\n---\n# PDF tools\n",
    )
    .unwrap();
    let done = tokio::time::timeout(std::time::Duration::from_secs(10), agent.compact())
        .await
        .is_ok();
    assert!(done, "/compact 不能挂住");

    let store = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(
        user_message_count(&store, "ROOT-UPDATED"),
        0,
        "手动 /compact 不应注入指令更新"
    );
    assert_eq!(
        user_message_count(&store, "UPDATED-SKILL-DESCRIPTION"),
        0,
        "手动 /compact 不应注入 skill 目录更新"
    );
}
