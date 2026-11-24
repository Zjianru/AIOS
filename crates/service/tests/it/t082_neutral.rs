//! D117：Kind 没有工作目录——系统提示确定性、旧会话升级、skill 根隔离。

use std::path::Path;

use agent::AgentConfig;
use service::{Call, DeliveryMode, RootHost, RootSessionOptions, SendRequest};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FINAL_TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_final","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":9,"output_tokens":2}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

async fn mount(server: &MockServer, expected: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(FINAL_TEXT_SSE.to_owned()),
        )
        .expect(expected)
        .mount(server)
        .await;
}

fn options(data_dir: &Path, cwd: &Path, base_url: &str) -> RootSessionOptions {
    let mut options = crate::common::options(data_dir);
    options.cwd = cwd.to_path_buf();
    options.model.base_url = base_url.to_owned();
    options.config = AgentConfig {
        instruction_roots: Vec::new(),
        skill_roots: Vec::new(),
        ..AgentConfig::default()
    };
    options
}

async fn send(host: &RootHost, data_dir: &Path, request_id: &str) {
    host.dispatch(Call::Send(SendRequest {
        request_id: request_id.to_owned(),
        content: "hello".to_owned(),
        delivery: DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
    }))
    .await
    .expect("发送");
    crate::common::wait_turns(host, data_dir, 1).await;
}

fn system(body: &[u8]) -> String {
    let value: serde_json::Value = serde_json::from_slice(body).expect("请求体 JSON");
    value["system"].as_str().expect("system").to_owned()
}

#[tokio::test]
async fn kind_prompt_has_no_workspace_and_is_byte_identical_across_cwds() {
    let server = MockServer::start().await;
    mount(&server, 3).await;
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let cwds = [repo.path(), home.path(), root.path()];
    let mut systems = Vec::new();
    for (index, cwd) in cwds.iter().enumerate() {
        let data = tempfile::tempdir().unwrap();
        let host = crate::common::open(options(data.path(), cwd, &server.uri())).await;
        send(&host, data.path(), &format!("neutral-{index}")).await;
        let bodies = server.received_requests().await.unwrap();
        let body = &bodies[index].body;
        let text = String::from_utf8(body.clone()).unwrap();
        let system = system(body);
        for path in [repo.path(), home.path(), root.path()] {
            assert!(
                !text.contains(&path.display().to_string()),
                "请求体不能带 cwd：{text}"
            );
        }
        assert!(!system.contains("Your working directory is"), "{system}");
        assert!(
            system.starts_with("You are Kind, the user's connector in AIOS."),
            "{system}"
        );
        systems.push(system);
        drop(host);
    }
    assert_eq!(systems[0], systems[1], "三种 cwd 的系统提示要逐字节一致");
    assert_eq!(systems[1], systems[2], "三种 cwd 的系统提示要逐字节一致");
}

#[tokio::test]
async fn old_root_session_with_repo_cwd_upgrades_to_neutral_prompt() {
    let server = MockServer::start().await;
    mount(&server, 1).await;
    let data = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    // 旧根会话：根头 cwd 已是仓库，但还没有系统提示（升级后首句才生成）。
    let old = crate::common::open(options(data.path(), repo.path(), &server.uri())).await;
    drop(old);
    std::fs::write(repo.path().join("AGENTS.md"), "OLD-REPO-INSTRUCTIONS\n").unwrap();
    let host = crate::common::open(options(data.path(), repo.path(), &server.uri())).await;
    send(&host, data.path(), "old-root").await;
    let bodies = server.received_requests().await.unwrap();
    assert_eq!(bodies.len(), 1);
    let text = String::from_utf8(bodies[0].body.clone()).unwrap();
    assert!(
        !text.contains("OLD-REPO-INSTRUCTIONS"),
        "Kind 不按 cwd 读 AGENTS：{text}"
    );
    assert!(
        !text.contains(&repo.path().display().to_string()),
        "旧仓库目录不进提示：{text}"
    );
    assert!(!system(&bodies[0].body).contains("Your working directory is"));
}

#[tokio::test]
async fn kind_skill_roots_exclude_project_agents_but_keep_data_dir() {
    let server = MockServer::start().await;
    mount(&server, 1).await;
    let data = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".agents/skills/p082only")).unwrap();
    std::fs::write(
        home.path().join(".agents/skills/p082only/SKILL.md"),
        "---\nname: p082only\ndescription: PROJECT-SKILL-MARKER\n---\nbody",
    )
    .unwrap();
    std::fs::create_dir_all(data.path().join("skills/datadirskill")).unwrap();
    std::fs::write(
        data.path().join("skills/datadirskill/SKILL.md"),
        "---\nname: datadirskill\ndescription: DATA-DIR-SKILL-MARKER\n---\nbody",
    )
    .unwrap();
    // ThinAgent 的发现路径仍会看到项目根 .agents/skills。
    let thin = tools::discover_skills(home.path(), data.path(), &[], &[]);
    assert!(
        thin.iter().any(|skill| skill.name == "p082only"),
        "ThinAgent 路径应看到 p082only"
    );
    let host = crate::common::open(options(data.path(), home.path(), &server.uri())).await;
    send(&host, data.path(), "skill-roots").await;
    let bodies = server.received_requests().await.unwrap();
    let text = String::from_utf8(bodies[0].body.clone()).unwrap();
    assert!(
        !text.contains("p082only"),
        "Kind 不读项目 .agents/skills：{text}"
    );
    assert!(
        !text.contains("PROJECT-SKILL-MARKER"),
        "Kind 不读项目 skill 正文：{text}"
    );
    assert!(
        text.contains("DATA-DIR-SKILL-MARKER"),
        "Kind 保留 data-dir skill：{text}"
    );
}

/// /local-file 只放通 Kind 根会话说过的绝对路径；旧 cwd 里的文件不会因为 cwd 自动放通。
#[tokio::test]
async fn local_file_allows_only_paths_said_in_the_root_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let host = crate::common::open(crate::common::options(dir.path())).await;
    let said = dir.path().join("said.txt");
    let silent = dir.path().join("silent.txt");
    std::fs::write(&said, "said").unwrap();
    std::fs::write(&silent, "silent").unwrap();
    host.append_user_message_for_test(&format!("看下 {}", said.display()))
        .await
        .unwrap();
    assert!(
        host.local_file_allowed(
            said.to_str().unwrap(),
            &std::fs::canonicalize(&said).unwrap()
        )
        .await
    );
    assert!(
        !host
            .local_file_allowed(
                silent.to_str().unwrap(),
                &std::fs::canonicalize(&silent).unwrap()
            )
            .await
    );
    host.shutdown().await;
}
