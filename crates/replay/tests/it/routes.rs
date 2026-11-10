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

/// 一条能推出回放的会话日志行：assistant/message + 一段文本流。
fn event_line(text: &str) -> String {
    let value = serde_json::json!({
        "type": "assistant/message",
        "seq": 1,
        "time": 1,
        "data": {
            "turn": 1,
            "step": 1,
            "message": {
                "id": "msg-route-file",
                "role": "assistant",
                "source": {"kind": "model", "provider": "deepseek", "model": "deepseek-flash"},
                "content": [{"type": "text", "text": text}]
            },
            "stream": [
                {"type": "text-chunks", "time0": 1, "index": 0, "dt": [], "texts": [text]},
                {"type": "chunk", "time": 1, "chunk": {"type": "block-end", "index": 0, "block": {"type": "text", "text": text}}},
                {"type": "chunk", "time": 1, "chunk": {"type": "finish", "reason": {"kind": "stop"}}}
            ]
        }
    });
    value.to_string()
}

#[test]
fn routes_load_from_file_relative_to_its_directory() {
    let dir = tempfile::tempdir().expect("临时目录");
    std::fs::write(dir.path().join("kind.jsonl"), event_line("来自文件") + "\n").expect("写日志");
    let route_path = dir.path().join("routes.json");
    std::fs::write(
        &route_path,
        r#"{"routes":[{"contains":"KIND","file":"kind.jsonl"}]}"#,
    )
    .expect("写路由文件");

    let script = ReplayScript::from_routes_file(&route_path).expect("读路由文件");
    assert_eq!(script.routes.len(), 1);
    assert_eq!(script.routes[0].contains, "KIND");
    assert_eq!(script.routes[0].calls.len(), 1, "路由的脚本从日志推出来");
    assert!(script.calls.is_empty(), "路由文件不产出默认脚本");

    // 没有 routes 的文件是格式错误，不静默当成空脚本。
    let bad = dir.path().join("empty.json");
    std::fs::write(&bad, "{}").expect("写坏路由文件");
    assert!(ReplayScript::from_routes_file(&bad).is_err());
}
