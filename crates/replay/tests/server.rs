//! T010.1：回放服务（清单 1 的 SSE 侧、清单 2 的 throw/hang、清单 3 的脚本用完）。

use std::time::{Duration, Instant};

use model::AssistantStreamRecord;
use replay::{ReplayCall, ReplayScript, start_replay_server};

fn chunk(text: &str) -> AssistantStreamRecord {
    AssistantStreamRecord::TextChunks {
        time0: 1,
        index: 0,
        dt: Vec::new(),
        texts: vec![text.to_owned()],
    }
}

async fn post(url: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("连接回放服务")
}

#[tokio::test]
async fn replays_calls_in_order_then_reports_exhaustion() {
    let script = ReplayScript {
        calls: vec![
            ReplayCall::Stream(vec![chunk("第一段")]),
            ReplayCall::Stream(vec![chunk("第二段")]),
        ],
        pace_ms: None,
    };
    let server = start_replay_server(script).await.expect("起回放服务");
    let url = format!("{}/v1/messages", server.url());

    let first = post(&url).await;
    assert_eq!(first.status(), 200);
    let body = first.text().await.expect("读第一段");
    assert!(
        body.contains("message_start"),
        "第一段缺 message_start：{body}"
    );
    assert!(body.contains("第一段"), "第一段内容不在：{body}");
    assert!(
        body.contains("message_stop"),
        "第一段缺 message_stop：{body}"
    );

    let second = post(&url).await;
    assert_eq!(second.status(), 200);
    assert!(second.text().await.expect("读第二段").contains("第二段"));

    let third = post(&url).await;
    assert_eq!(third.status(), 500, "脚本用完要回 500");
    let body = third.text().await.expect("读用完说明");
    assert!(body.contains("回放脚本已用完"), "缺说明：{body}");
    server.shutdown();
}

#[tokio::test]
async fn throw_returns_status_and_body() {
    let script = ReplayScript {
        calls: vec![ReplayCall::Throw {
            status: 429,
            body: serde_json::json!({"error": {"message": "太频繁"}}),
        }],
        pace_ms: None,
    };
    let server = start_replay_server(script).await.expect("起回放服务");
    let response = post(&format!("{}/messages", server.url())).await;
    assert_eq!(response.status(), 429);
    let body = response.text().await.expect("读 body");
    assert!(body.contains("太频繁"), "body={body}");
    server.shutdown();
}

#[tokio::test]
async fn hang_does_not_answer() {
    let script = ReplayScript {
        calls: vec![ReplayCall::Hang],
        pace_ms: None,
    };
    let server = start_replay_server(script).await.expect("起回放服务");
    let url = format!("{}/messages", server.url());
    let result = tokio::time::timeout(Duration::from_millis(300), post(&url)).await;
    assert!(result.is_err(), "hang 必须在被取消前不回应");
    server.shutdown();
}

#[tokio::test]
async fn pace_ms_delays_frames() {
    let script = ReplayScript {
        calls: vec![ReplayCall::Stream(vec![chunk("慢一点")])],
        pace_ms: Some(40),
    };
    let server = start_replay_server(script).await.expect("起回放服务");
    let started = Instant::now();
    let response = post(&format!("{}/messages", server.url())).await;
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("读 body");
    assert!(body.contains("慢一点"));
    assert!(
        started.elapsed() >= Duration::from_millis(120),
        "帧间隔要生效，实际 {:?}",
        started.elapsed()
    );
    server.shutdown();
}
