//! T077.2：旧会话固定块变化时，下一轮请求末尾补程序消息；内容不变不重复补。

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, test_credential,
};
use agent::{Agent, Delivery, Deps};
use model::UserMessage;
use session::SessionStore;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

#[tokio::test]
async fn old_session_gets_changed_fixed_block_at_tail_then_stays() {
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
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data.path(), project.path()).unwrap();
    let id = store.id().clone();
    drop(store);
    let deps = || Deps {
        model: model::DeepSeek::with_credentials(model_config(&server.uri()), test_credential())
            .unwrap(),
        tools: tools::ToolSet::builtin(),
        store: SessionStore::open(data.path(), &id).unwrap(),
    };
    let first = Agent::create(isolated_config(data.path()), deps())
        .await
        .unwrap();
    first
        .send(UserMessage::text("第一句"), Delivery::FollowUp)
        .await
        .unwrap();
    first.idle().await;
    drop(first);

    let mut config = isolated_config(data.path());
    config.fixed_block = Some(agent::FixedBlock {
        text: "页面清单甲：现在、Kind、工作".to_owned(),
    });
    let second = Agent::resume(id.clone(), config, deps()).await.unwrap();
    second
        .send(UserMessage::text("第二句"), Delivery::FollowUp)
        .await
        .unwrap();
    second.idle().await;
    drop(second);

    let mut config = isolated_config(data.path());
    config.fixed_block = Some(agent::FixedBlock {
        text: "页面清单乙：现在、Kind、工作、监控".to_owned(),
    });
    let third = Agent::resume(id.clone(), config, deps()).await.unwrap();
    third
        .send(UserMessage::text("第三句"), Delivery::FollowUp)
        .await
        .unwrap();
    third.idle().await;

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect();
    let second_body = &bodies[1];
    assert!(
        second_body.contains("页面清单甲"),
        "旧会话下一轮要看到新固定块"
    );
    assert_eq!(second_body.matches("页面清单甲").count(), 1, "只补一次");
    let third_body = &bodies[2];
    assert!(third_body.contains("页面清单乙"), "内容再变要补新版本");
    assert!(
        third_body.find("页面清单乙") > third_body.find("页面清单甲"),
        "新固定块在末尾"
    );
}

#[tokio::test]
async fn old_session_gets_a_new_bundled_skill_directory_at_tail() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data.path(), project.path()).unwrap();
    let id = store.id().clone();
    drop(store);
    let deps = || Deps {
        model: model::DeepSeek::with_credentials(model_config(&server.uri()), test_credential())
            .unwrap(),
        tools: tools::ToolSet::builtin(),
        store: SessionStore::open(data.path(), &id).unwrap(),
    };
    let first = Agent::create(isolated_config(data.path()), deps())
        .await
        .unwrap();
    first
        .send(UserMessage::text("第一句"), Delivery::FollowUp)
        .await
        .unwrap();
    first.idle().await;
    drop(first);

    let skill = tools::BundledSkill {
        name: "new-guide",
        raw: "---\nname: new-guide\ndescription: 新说明书\n---\n\n说明正文。\n",
        requires: None,
        tools: &["read"],
        read_only: false,
        unavailable_hint: "",
    };
    let mut config = isolated_config(data.path());
    config.bundled_skills = vec![skill];
    let second = Agent::resume(id.clone(), config, deps()).await.unwrap();
    second
        .send(UserMessage::text("第二句"), Delivery::FollowUp)
        .await
        .unwrap();
    second.idle().await;

    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(
        body.contains("new-guide"),
        "新进 skill 目录要在下一轮请求末尾"
    );
}
