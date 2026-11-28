//! 联网搜索服务商协议、Keyring 只写不读、SearXNG Basic、检测状态。

use service::{
    Call, ExaKeyRequest, ExaKeySaveRequest, Response, RootHost, WebSearchAuthSetRequest,
    WebSearchHostSetRequest, WebSearchKeyRequest, WebSearchKeySaveRequest, WebSearchOpenRequest,
    WebSearchReadRequest,
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
    assert!(empty.enabled.is_empty(), "默认不开");
    assert_eq!(empty.schedule.mode, "balanced");
    assert_eq!(empty.schedule.per_provider, 5);
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
        .dispatch(Call::WebSearchOpen(WebSearchOpenRequest {
            id: "duckduckgo".to_owned(),
        }))
        .await
        .expect("open")
    {
        Response::WebSearchOpen(view) => view,
        other => panic!("期望 WebSearchOpen：{other:?}"),
    };
    assert_eq!(selected.enabled, vec!["duckduckgo".to_owned()]);
    assert_eq!(selected.priority, vec!["duckduckgo".to_owned()]);
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
    host.dispatch(Call::WebSearchOpen(WebSearchOpenRequest {
        id: "exa".to_owned(),
    }))
    .await
    .expect("open");
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
    host.dispatch(Call::WebSearchOpen(WebSearchOpenRequest {
        id: "searxng".to_owned(),
    }))
    .await
    .expect("open");
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

#[tokio::test]
async fn web_search_open_close_priority_and_schedule_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let host = open(crate::common::options(&data)).await;
    for id in ["exa", "duckduckgo"] {
        host.dispatch(Call::WebSearchOpen(service::WebSearchOpenRequest {
            id: id.to_owned(),
        }))
        .await
        .expect("open");
    }
    let view = match host
        .dispatch(Call::WebSearchPrioritySet(
            service::WebSearchPrioritySetRequest {
                ids: vec!["duckduckgo".to_owned(), "exa".to_owned()],
            },
        ))
        .await
        .expect("priority")
    {
        Response::WebSearchPrioritySet(view) => view,
        other => panic!("期望 priority：{other:?}"),
    };
    assert_eq!(
        view.priority,
        vec!["duckduckgo".to_owned(), "exa".to_owned()]
    );
    assert_eq!(view.schedule.mode, "balanced");
    let cheap = match host
        .dispatch(Call::WebSearchScheduleSet(
            service::WebSearchScheduleSetRequest {
                mode: "cheap".to_owned(),
                per_provider: 3,
            },
        ))
        .await
        .expect("schedule")
    {
        Response::WebSearchScheduleSet(view) => view,
        other => panic!("期望 schedule：{other:?}"),
    };
    assert_eq!(cheap.schedule.mode, "cheap");
    assert_eq!(cheap.schedule.per_provider, 3);
    let closed = match host
        .dispatch(Call::WebSearchClose(service::WebSearchOpenRequest {
            id: "exa".to_owned(),
        }))
        .await
        .expect("close")
    {
        Response::WebSearchClose(view) => view,
        other => panic!("期望 close：{other:?}"),
    };
    assert_eq!(closed.enabled, vec!["duckduckgo".to_owned()]);
    assert_eq!(closed.priority, vec!["duckduckgo".to_owned()]);
    let bad = host
        .dispatch(Call::WebSearchScheduleSet(
            service::WebSearchScheduleSetRequest {
                mode: "fast".to_owned(),
                per_provider: 5,
            },
        ))
        .await;
    assert!(bad.is_err(), "非法调度要拒");
    let bad_count = host
        .dispatch(Call::WebSearchScheduleSet(
            service::WebSearchScheduleSetRequest {
                mode: "balanced".to_owned(),
                per_provider: 7,
            },
        ))
        .await;
    assert!(bad_count.is_err(), "每家取几条只收 3/5/10");
}
