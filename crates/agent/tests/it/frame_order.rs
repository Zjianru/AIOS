//! 第一轮固定块在用户话前、触发记忆在用户话后；Kind 只带项目规则路径。

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, test_credential,
};
use agent::{Agent, AgentConfig, Delivery, Deps};
use model::UserMessage;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

#[tokio::test]
async fn fixed_blocks_are_before_the_user_message_and_flashback_after() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "PROJECT-INSTRUCTION-FULL").unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        memory_context: Some(std::sync::Arc::new(|| {
            Ok(Some(agent::MemoryContext {
                identity: "frame-v1".to_owned(),
                text: "FIXED-MEMORY".to_owned(),
                ids: vec!["mem-frame".to_owned()],
            }))
        })),
        memory_flashback: Some(std::sync::Arc::new(|message: &str| {
            if !message.contains("触发这句") {
                return Ok(None);
            }
            Ok(Some(agent::MemoryFlashback {
                text: "FLASHBACK-TRIGGER".to_owned(),
                ids: vec!["mem-old".to_owned()],
            }))
        })),
        bundled_skills: vec![tools::BundledSkill {
            name: "demo-frame",
            raw: "---\nname: demo-frame\ndescription: 固定块顺序\n---\n\n正文。",
            requires: None,
            tools: &[],
            unavailable_hint: "",
        }],
        ..isolated_config(data_dir.path())
    };
    // cwd 决定项目规则位置；用同一个 tempdir 作为会话 cwd。
    let body = capture_config_with_cwd(config, project.path()).await;
    let instruction_at = body.find("PROJECT-INSTRUCTION-FULL").expect("项目指令在");
    let skill_at = body.find("demo-frame").expect("skill 目录在");
    let memory_at = body.find("FIXED-MEMORY").expect("常驻记忆在");
    let user_at = body.find("触发这句").expect("用户话在");
    let flashback_at = body.find("FLASHBACK-TRIGGER").expect("触发记忆在");
    assert!(instruction_at < user_at, "固定指令在用户话前：{body}");
    assert!(skill_at < user_at, "skill 目录在用户话前：{body}");
    assert!(memory_at < user_at, "常驻记忆在用户话前：{body}");
    assert!(flashback_at > user_at, "触发记忆在用户话后：{body}");
}

async fn capture_config_with_cwd(config: AgentConfig, cwd: &std::path::Path) -> String {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd).unwrap();
    let mut config = config;
    config.instruction_roots = vec![];
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
        .send(UserMessage::text("触发这句"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "第一轮一条请求");
    String::from_utf8(requests[0].body.clone()).unwrap()
}

#[tokio::test]
async fn kind_project_rules_are_a_path_line_while_thin_gets_the_full_text() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "SECRET-FULL-RULES").unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let path_only = capture_config_with_cwd(
        AgentConfig {
            project_instructions_path_only: true,
            ..isolated_config(data_dir.path())
        },
        project.path(),
    )
    .await;
    assert!(
        path_only.contains("Project rules are at"),
        "只带路径行：{path_only}"
    );
    assert!(
        path_only.contains("AGENTS.md"),
        "带项目规则路径：{path_only}"
    );
    assert!(
        !path_only.contains("SECRET-FULL-RULES"),
        "Kind 不整份注入：{path_only}"
    );

    let data_dir = tempfile::tempdir().unwrap();
    let full = capture_config_with_cwd(
        AgentConfig {
            project_instructions_path_only: false,
            ..isolated_config(data_dir.path())
        },
        project.path(),
    )
    .await;
    assert!(
        full.contains("SECRET-FULL-RULES"),
        "ThinAgent 拿全文：{full}"
    );
}
