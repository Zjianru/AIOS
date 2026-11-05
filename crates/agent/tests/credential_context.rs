//! T008.F4：经过 Agent 的真实模型请求，Keyring 签发记录必须带 run/turn。

mod common;

use std::sync::Arc;

use agent::{Agent, AgentOpenOptions, Delivery};
use common::{FINAL_TEXT_SSE, isolated_config, model_config, sse, wait_idle};
use credentials::{Keyring, NewConnection};
use model::CredentialContext;
use model::UserMessage;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

struct KeyringSource {
    keyring: Arc<Keyring>,
}

impl model::CredentialSource for KeyringSource {
    fn issue(
        &self,
        purpose: &str,
        context: &CredentialContext,
    ) -> Result<String, model::ModelError> {
        self.keyring
            .issue("deepseek", purpose, context.run.as_deref(), context.turn)
            .map(credentials::IssuedSecret::into_secret)
            .map_err(|error| {
                model::ModelError::new(model::ErrorCode::MissingCredential, error.to_string())
            })
    }
}

#[tokio::test]
async fn agent_turn_records_run_and_turn_in_keyring() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse(FINAL_TEXT_SSE.to_owned()))
        .mount(&server)
        .await;

    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let keyring_dir = tempfile::tempdir().unwrap();
    let keyring = Arc::new(Keyring::open(keyring_dir.path(), None).expect("open keyring"));
    keyring
        .save_connection(NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: "sk-test-context".to_owned(),
        })
        .expect("save connection");

    let agent = Agent::open(AgentOpenOptions {
        data_dir: data_dir.path().to_path_buf(),
        cwd: cwd.path().to_path_buf(),
        resume: None,
        model: model_config(&server.uri()),
        config: isolated_config(data_dir.path()),
        credentials: Some(Arc::new(KeyringSource {
            keyring: Arc::clone(&keyring),
        })),
        append_hook_factory: None,
    })
    .await
    .expect("open agent");

    agent
        .send(UserMessage::text("你好"), Delivery::FollowUp)
        .await
        .expect("send");
    wait_idle(&agent, 10).await;

    let issues = keyring.issues();
    assert!(!issues.is_empty(), "必须至少有一条签发记录");
    let issue = issues
        .iter()
        .find(|issue| issue.purpose == "model-request")
        .expect("应有 model-request 记录");
    assert!(
        issue.run.as_deref().is_some_and(|run| !run.is_empty()),
        "经过 Agent 的签发记录必须带 run：{issue:?}"
    );
    assert!(
        issue.turn.is_some_and(|turn| turn >= 1),
        "经过 Agent 的签发记录必须带 turn：{issue:?}"
    );
}
