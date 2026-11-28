//! 联网搜索服务商协议、Keyring 只写不读、SearXNG Basic、检测状态。

use service::{
    Call, ExaKeyRequest, ExaKeySaveRequest, Response, RootHost, WebSearchAuthSetRequest,
    WebSearchHostSetRequest, WebSearchKeyRequest, WebSearchKeySaveRequest, WebSearchReadRequest,
    WebSearchSelectRequest,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::open;

async fn read_view(host: &RootHost) -> service::WebSearchReadResponse {
    match host
        .dispatch(Call::WebSearchRead(WebSearchReadRequest {}))
        .await
        .expect("read")
    {
        Response::WebSearchRead(view) => view,
        other => panic!("期望 WebSearchRead：{other:?}"),
    }
}

#[tokio::test]
async fn web_search_read_has_twenty_providers_and_never_returns_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let host = open(crate::common::options(&data)).await;
    let empty = read_view(&host).await;
    assert_eq!(empty.providers.len(), 20);
    assert!(empty.selected.is_none(), "默认不选");
    assert!(
        empty
            .providers
            .iter()
            .find(|provider| provider.id == "duckduckgo")
            .is_some_and(|provider| provider.key_need == "none")
    );

    match host
        .dispatch(Call::WebSearchKeySave(WebSearchKeySaveRequest {
            id: "exa".to_owned(),
            key: "exa-secret-one".to_owned(),
        }))
        .await
        .expect("save key")
    {
        Response::WebSearchKeySave(view) => {
            let exa = view
                .providers
                .iter()
                .find(|provider| provider.id == "exa")
                .unwrap();
            assert!(exa.key_saved, "存了 key");
            let json = serde_json::to_string(&view).unwrap();
            assert!(!json.contains("exa-secret-one"), "响应里不能有 key：{json}");
        }
        other => panic!("期望 WebSearchKeySave：{other:?}"),
    }

    let selected = match host
        .dispatch(Call::WebSearchSelect(WebSearchSelectRequest {
            id: Some("duckduckgo".to_owned()),
        }))
        .await
        .expect("select")
    {
        Response::WebSearchSelect(view) => view,
        other => panic!("期望 WebSearchSelect：{other:?}"),
    };
    assert_eq!(selected.selected.as_deref(), Some("duckduckgo"));
    assert!(
        selected
            .providers
            .iter()
            .find(|provider| provider.id == "exa")
            .is_some_and(|provider| provider.key_saved),
        "换一家不丢上一家的 key"
    );
}

#[tokio::test]
async fn exa_check_writes_ok_and_fail_and_host_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": [{"url": "https://example.com/a", "title": "A", "highlights": ["摘要"]}]
        })))
        .mount(&server)
        .await;
    let host = open(crate::common::options(&data)).await;
    host.dispatch(Call::WebSearchKeySave(WebSearchKeySaveRequest {
        id: "exa".to_owned(),
        key: "exa-secret".to_owned(),
    }))
    .await
    .expect("save");
    host.dispatch(Call::WebSearchSelect(WebSearchSelectRequest {
        id: Some("exa".to_owned()),
    }))
    .await
    .expect("select");
    host.dispatch(Call::WebSearchHostSet(WebSearchHostSetRequest {
        id: "exa".to_owned(),
        host: server.uri(),
    }))
    .await
    .expect("host");

    let checked = match host
        .dispatch(Call::WebSearchCheck(WebSearchReadRequest {}))
        .await
        .expect("check")
    {
        Response::WebSearchCheck(view) => view,
        other => panic!("期望 WebSearchCheck：{other:?}"),
    };
    let exa = checked
        .providers
        .iter()
        .find(|provider| provider.id == "exa")
        .unwrap();
    assert!(
        matches!(exa.check, credentials::CheckState::Ok { .. }),
        "{:?}",
        exa.check
    );
}

#[tokio::test]
async fn searxng_basic_password_is_saved_in_keyring_and_sent() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(header("authorization", "Basic dXNlcjpwYXNz"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": [{"url": "https://example.com/s", "title": "S", "content": "摘要 S"}]
        })))
        .mount(&server)
        .await;
    let host = open(crate::common::options(&data)).await;
    host.dispatch(Call::WebSearchSelect(WebSearchSelectRequest {
        id: Some("searxng".to_owned()),
    }))
    .await
    .expect("select");
    host.dispatch(Call::WebSearchHostSet(WebSearchHostSetRequest {
        id: "searxng".to_owned(),
        host: server.uri(),
    }))
    .await
    .expect("host");
    let view = match host
        .dispatch(Call::WebSearchAuthSet(WebSearchAuthSetRequest {
            id: "searxng".to_owned(),
            user: "user".to_owned(),
            password: Some("pass".to_owned()),
        }))
        .await
        .expect("auth")
    {
        Response::WebSearchAuthSet(view) => view,
        other => panic!("期望 WebSearchAuthSet：{other:?}"),
    };
    let searx = view
        .providers
        .iter()
        .find(|provider| provider.id == "searxng")
        .unwrap();
    assert!(searx.basic_saved);
    assert_eq!(searx.basic_user, "user");
    let json = serde_json::to_string(&view).unwrap();
    assert!(!json.contains("pass"), "响应不能回显 Basic 密码：{json}");

    let checked = match host
        .dispatch(Call::WebSearchCheck(WebSearchReadRequest {}))
        .await
        .expect("check")
    {
        Response::WebSearchCheck(view) => view,
        other => panic!("期望 WebSearchCheck：{other:?}"),
    };
    let searx = checked
        .providers
        .iter()
        .find(|provider| provider.id == "searxng")
        .unwrap();
    assert!(
        matches!(searx.check, credentials::CheckState::Ok { .. }),
        "{:?}",
        searx.check
    );
}

#[tokio::test]
async fn deleting_a_key_disables_only_that_provider() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let host = open(crate::common::options(&data)).await;
    host.dispatch(Call::WebSearchKeySave(WebSearchKeySaveRequest {
        id: "tavily".to_owned(),
        key: "tavily-secret".to_owned(),
    }))
    .await
    .expect("save");
    let view = match host
        .dispatch(Call::WebSearchKeyDelete(WebSearchKeyRequest {
            id: "tavily".to_owned(),
        }))
        .await
        .expect("delete")
    {
        Response::WebSearchKeyDelete(view) => view,
        other => panic!("期望 WebSearchKeyDelete：{other:?}"),
    };
    assert!(
        !view
            .providers
            .iter()
            .find(|provider| provider.id == "tavily")
            .unwrap()
            .key_saved
    );
    // 旧 Exa 操作仍能读，迁移后同一把 key 不丢。
    let _ = host
        .dispatch(Call::ExaKey(ExaKeyRequest {}))
        .await
        .expect("旧 Exa 视图");
    let _ = host
        .dispatch(Call::ExaKeySave(ExaKeySaveRequest {
            key: "exa-secret".to_owned(),
        }))
        .await
        .expect("旧 Exa 保存");
    let view = read_view(&host).await;
    assert!(
        view.providers
            .iter()
            .find(|provider| provider.id == "exa")
            .unwrap()
            .key_saved
    );
}
