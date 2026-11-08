//! T012.6：学 400（关不掉思考/必须思考）与两个鉴权头（测试清单 4、5）。
//!
//! 接缝：wiremock 的 HTTP 接缝（spec 测试决定 (b)）；断言真实请求的请求体/请求头序列。
//! 期望值来源：Magpie `gateway/anthropic.go:425`（alwaysThinks）、`thinking_needed.go`
//! （thinkingNeededRefusal）、`provider/authheader.go`（RetryOneAuth：x-api-key 先、Authorization 后）。

use std::sync::{Arc, Mutex};

use model::{
    CancelToken, CredentialContext, CredentialSource, DeepSeek, DeepSeekConfig, ModelError,
};
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::common::SequenceResponder;

/// 一次正常的纯文本回答 SSE（照 agent tests 的 FINAL_TEXT_SSE 最小版）。
const FINAL_TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_final","type":"message","role":"assistant","model":"mystery-model","content":[],"usage":{"input_tokens":50,"output_tokens":1}}}

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

fn sse(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

/// 记录 `remember_auth_header` 回报的凭证来源。
#[derive(Default)]
struct RecordingSource {
    remembered: Mutex<Vec<String>>,
}

impl RecordingSource {
    fn remembered(&self) -> Vec<String> {
        self.remembered.lock().expect("remembered").clone()
    }
}

impl CredentialSource for RecordingSource {
    fn issue(&self, _purpose: &str, _context: &CredentialContext) -> Result<String, ModelError> {
        Ok("sk-test-key".to_owned())
    }

    fn remember_auth_header(&self, header: &str) {
        self.remembered
            .lock()
            .expect("remembered")
            .push(header.to_owned());
    }
}

fn config(uri: &str) -> DeepSeekConfig {
    DeepSeekConfig {
        base_url: uri.to_owned(),
        model: "mystery-model".to_owned(),
        ..DeepSeekConfig::default()
    }
}

fn cancel() -> CancelToken {
    CancelToken::new(
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(tokio::sync::Notify::new()),
    )
}

/// 跑一次 stream 到结束；成功返回 ().
async fn drain(deepseek: &DeepSeek) -> Result<(), ModelError> {
    let mut stream = deepseek.stream(&[], &[], &cancel()).await?;
    while let Some(chunk) = stream.next().await {
        chunk?;
    }
    Ok(())
}

fn body_of(request: &Request) -> Value {
    serde_json::from_slice(&request.body).expect("请求体是 JSON")
}

/// 测试清单 4：mock 第一次回 2013 原文、第二次回 200。断言：第二个请求是最低的非关档；
/// 这一轮成功；同一进程里下一轮第一次请求直接发最低档（Magpie RetryOneAuth 同形）。
#[tokio::test]
async fn 学_400_后本轮立即按最低非关档重发且下一轮直接最低档() {
    let server = MockServer::start().await;
    let rejected = ResponseTemplate::new(400).set_body_json(serde_json::json!({
        "type": "error",
        "error": {
            "type": "invalid_request_error",
            "message": "mystery-model requires adaptive thinking; thinking.type=\"disabled\" (including reasoning.effort=none) is not allowed (2013)"
        }
    }));
    let ok = sse(FINAL_TEXT_SSE.to_owned());
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            rejected,
            ok.clone(),
            ok.clone(),
        ]))
        .mount(&server)
        .await;
    let source = Arc::new(RecordingSource::default());
    let deepseek = DeepSeek::with_credentials(
        config(&server.uri()),
        Arc::clone(&source) as Arc<dyn CredentialSource>,
    )
    .expect("构造");

    // 第一轮：想要的档是 none（默认 config 是 high；这里显式用 none 模拟「用户选了不思考」）。
    let off = deepseek.with_effort(model::Effort::new("none"));
    drain(&off).await.expect("400 后本轮应按最低非关档重发成功");
    // T012.6 P2：日志侧 effort_state 的 actual 与实际重发的档一致（不是旧 actual）。
    assert_eq!(
        off.effort_state(),
        ("none".to_owned(), "low".to_owned()),
        "学到 400 后 effort_state 的 actual 是实际发的最低非关档"
    );
    // 第二轮：同一实例的下一轮，第一次请求就直接最低档。
    drain(&off).await.expect("学到的下一轮直接最低档");

    let requests = server.received_requests().await.expect("请求记录");
    assert_eq!(requests.len(), 3, "第一轮 2 次（400 + 重发）+ 第二轮 1 次");
    let bodies: Vec<Value> = requests.iter().map(body_of).collect();
    assert_eq!(
        bodies[0]["thinking"]["type"], "disabled",
        "第一次先按要的 none 发：{}",
        bodies[0]
    );
    assert_eq!(
        bodies[1]["thinking"]["type"], "enabled",
        "400 后本轮立即按最低非关档重发：{}",
        bodies[1]
    );
    assert_eq!(
        bodies[1]["thinking"]["budget_tokens"], 4096,
        "最低非关档是 low（预算 4096）：{}",
        bodies[1]
    );
    assert_eq!(
        bodies[2]["thinking"]["type"], "enabled",
        "同进程下一轮第一次请求直接最低档：{}",
        bodies[2]
    );
    assert_eq!(bodies[2]["thinking"]["budget_tokens"], 4096);
}

/// 测试清单 5：mock 拒绝两个头一起带；断言各试一次（x-api-key 先、Authorization 后），
/// 并记住通过的那个；下一轮只带学到的头。
#[tokio::test]
async fn 双头被拒后各试一次并记住通过的头() {
    let server = MockServer::start().await;
    let both_refused = ResponseTemplate::new(401).set_body_json(serde_json::json!({
        "type": "error",
        "error": {
            "type": "authentication_error",
            "message": "x-api-key 和 Authorization 不可以同时存在，只能使用一个"
        }
    }));
    let unauthorized = ResponseTemplate::new(401).set_body_json(serde_json::json!({
        "type": "error",
        "error": {"type": "authentication_error", "message": "bad key"}
    }));
    let ok = sse(FINAL_TEXT_SSE.to_owned());
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            both_refused,
            unauthorized,
            ok.clone(),
            ok.clone(),
        ]))
        .mount(&server)
        .await;
    let source = Arc::new(RecordingSource::default());
    let deepseek = DeepSeek::with_credentials(
        config(&server.uri()),
        Arc::clone(&source) as Arc<dyn CredentialSource>,
    )
    .expect("构造");

    drain(&deepseek).await.expect("单头轮换后本轮应成功");
    assert_eq!(
        source.remembered(),
        vec!["authorization".to_owned()],
        "记住通过的单头（x-api-key 先试、Authorization 后试）"
    );
    drain(&deepseek).await.expect("记住后下一轮成功");

    let requests = server.received_requests().await.expect("请求记录");
    assert_eq!(requests.len(), 4, "第一轮 3 次 + 第二轮 1 次");
    let has = |index: usize, header: &str| requests[index].headers.contains_key(header);
    assert!(
        has(0, "x-api-key") && has(0, "authorization"),
        "默认两个头一起发"
    );
    assert!(
        has(1, "x-api-key") && !has(1, "authorization"),
        "先只试 x-api-key"
    );
    assert!(
        !has(2, "x-api-key") && has(2, "authorization"),
        "再只试 authorization"
    );
    assert!(
        !has(3, "x-api-key") && has(3, "authorization"),
        "记住通过的头后下一轮只带它"
    );
    assert!(
        requests[2].headers["authorization"]
            .to_str()
            .expect("ascii")
            .starts_with("Bearer "),
        "Authorization 用 Bearer"
    );
}
