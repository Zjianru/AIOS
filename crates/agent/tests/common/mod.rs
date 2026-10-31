//! agent 集成测试共用的小工具。
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use wiremock::{Request, Respond, ResponseTemplate};

/// 一个按脚本顺序回放的 mock 响应器（第 N 次请求拿第 N 个响应）。
pub struct SequenceResponder {
    responses: Mutex<VecDeque<ResponseTemplate>>,
}

impl SequenceResponder {
    pub fn new(responses: Vec<ResponseTemplate>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

impl Respond for SequenceResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.responses
            .lock()
            .expect("mock 脚本锁被毒化")
            .pop_front()
            .expect("mock 收到的请求比脚本多")
    }
}

/// 一个 text/event-stream 响应模板。
pub fn sse(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}
