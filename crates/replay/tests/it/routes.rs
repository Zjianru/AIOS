//! 按请求体路由：两个会话的请求交错时，各自按自己的脚本顺序回放。

use model::AssistantStreamRecord;
use replay::{ReplayCall, ReplayRoute, ReplayScript, start_replay_server};

fn stream(text: &str) -> ReplayCall {
    ReplayCall::Stream(vec![AssistantStreamRecord::TextChunks {
        time0: 1,
        index: 0,
        dt: Vec::new(),
        texts: vec![text.to_owned()],
    }])
}

async fn post(url: &str, body: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .expect("连接回放服务")
}

#[tokio::test]
async fn routes_pick_scripts_by_request_body() {
    let script = ReplayScript {
        calls: Vec::new(),
        pace_ms: None,
        routes: vec![
            ReplayRoute {
                contains: "\"name\":\"task_accept\"".to_owned(),
                calls: vec![stream("kind-1"), stream("kind-2")],
                pace_ms: None,
            },
            ReplayRoute {
                contains: "\"name\":\"task_report\"".to_owned(),
                calls: vec![stream("thin-1")],
                pace_ms: None,
            },
        ],
    };
    let server = start_replay_server(script).await.expect("起回放服务");
    let url = format!("{}/v1/messages", server.url());

    // 两个会话的请求交错：Kind、ThinAgent、Kind。
    let kind = post(&url, r#"{"tools":[{"name":"task_accept"}]}"#).await;
    assert_eq!(kind.status(), 200);
    let body = kind.text().await.expect("读 Kind 第一段");
    assert!(body.contains("kind-1"), "Kind 第一段不对：{body}");

    let thin = post(&url, r#"{"tools":[{"name":"task_report"}]}"#).await;
    assert_eq!(thin.status(), 200);
    let body = thin.text().await.expect("读 ThinAgent 一段");
    assert!(body.contains("thin-1"), "ThinAgent 段不对：{body}");

    let kind = post(
        &url,
        r#"{"tools":[{"name":"task_accept"}],"messages":[{"content":"kind-2"}]}"#,
    )
    .await;
    assert_eq!(
        kind.status(),
        200,
        "Kind 的第二次请求不能串到别的路由"
    );
    let body = kind.text().await.expect("读 Kind 第二段");
    assert!(body.contains("kind-2"), "Kind 第二段不对：{body}");

    // 每条路由的游标各自前进：ThinAgent 用完报用完，不拿 Kind 剩下的。
    let thin = post(&url, r#"{"tools":[{"name":"task_report"}]}"#).await;
    assert_eq!(thin.status(), 500);
    let body = thin.text().await.expect("读用完说明");
    assert!(body.contains("回放脚本已用完"), "缺用完说明：{body}");

    // 没有一条路由匹配：明确报没有路由，不落回默认脚本。
    let other = post(&url, r#"{"tools":[{"name":"read"}]}"#).await;
    assert_eq!(other.status(), 500);
    let body = other.text().await.expect("读没有路由的说明");
    assert!(body.contains("没有匹配的路由"), "缺路由说明：{body}");
    server.shutdown();
}
