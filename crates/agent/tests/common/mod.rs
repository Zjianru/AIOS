//! agent 集成测试共用的小工具（mock 响应器、可控测试工具、可控 SSE 服务器）。
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;

use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc};
use tools::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolFuture, ToolResult};
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

/// 可控测试工具发出的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEvent {
    Started(String),
    Finished(String),
}

/// 可控的测试工具：通知开始，等 harness 放行，再通知结束。
pub struct GatedTool {
    name: &'static str,
    mode: ToolConcurrency,
    parameters: Value,
    started: mpsc::UnboundedSender<ToolEvent>,
    permits: Arc<Semaphore>,
}

impl Tool for GatedTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "test-only gated tool"
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn concurrency(&self) -> ToolConcurrency {
        self.mode
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let _ = self.started.send(ToolEvent::Started(self.name.to_owned()));
            let permit = Arc::clone(&self.permits)
                .acquire_owned()
                .await
                .expect("测试工具的信号量被关闭");
            let _ = self.started.send(ToolEvent::Finished(self.name.to_owned()));
            drop(permit);
            ToolResult::success(call.id, format!("ok:{}", self.name))
        })
    }
}

/// 造一个可控测试工具，并把它的放行信号量交给调用方。
pub fn gated(
    name: &'static str,
    mode: ToolConcurrency,
    started: &mpsc::UnboundedSender<ToolEvent>,
    permits: &mut Vec<(&'static str, Arc<Semaphore>)>,
) -> Box<dyn Tool> {
    let semaphore = Arc::new(Semaphore::new(0));
    permits.push((name, Arc::clone(&semaphore)));
    Box::new(GatedTool {
        name,
        mode,
        parameters: json!({"type": "object", "properties": {}}),
        started: started.clone(),
        permits: semaphore,
    })
}

/// 回放一组 tool_use 调用的 SSE（每个调用 input 为空对象）。
pub fn tool_calls_sse(calls: &[(&str, &str)]) -> String {
    let mut body = String::from(
        "event: message_start
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_tools\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"deepseek-flash\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
    );
    for (index, (id, name)) in calls.iter().enumerate() {
        body.push_str(&format!(
            "event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":{index},\"content_block\":{{\"type\":\"tool_use\",\"id\":\"{id}\",\"name\":\"{name}\",\"input\":{{}}}}}}\n\n"
        ));
        body.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":{index},\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{{}}\"}}}}\n\n"
        ));
        body.push_str(&format!(
            "event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{index}}}\n\n"
        ));
    }
    body.push_str(
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":2}}\n\n",
    );
    body.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    body
}

/// 一次正常的纯文本回答 SSE。
pub const FINAL_TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_final","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":50,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"工具都跑完了。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

/// 一段写到一半的 SSE（打开着 text 块，没有 content_block_stop/message_stop）。
pub const PARTIAL_TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_partial","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"已经流出来的"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"部分文字。"}}

"#;

/// 一个请求的脚本：先写 frames；hold 时等测试放行再关连接。
pub struct SseScript {
    pub frames: String,
    pub hold: bool,
}

/// 一个最小的、可控制节奏的 SSE 服务器：按脚本依次服务请求。
/// 只给 T004.3 的「流中途取消」测试用；不是产品代码。
pub struct ScriptedSseServer {
    port: u16,
    ready: Option<tokio::sync::oneshot::Receiver<()>>,
    control: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl ScriptedSseServer {
    pub fn start(scripts: Vec<SseScript>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定测试 SSE 端口");
        let port = listener.local_addr().expect("读取测试端口").port();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (control_tx, control_rx) = std::sync::mpsc::channel::<()>();
        let mut ready_tx = Some(ready_tx);
        let handle = std::thread::spawn(move || {
            for script in scripts {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                read_request(&mut stream);
                let header = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
                if stream.write_all(header.as_bytes()).is_err() {
                    continue;
                }
                if stream.write_all(script.frames.as_bytes()).is_err() {
                    continue;
                }
                if stream.flush().is_err() {
                    continue;
                }
                if script.hold {
                    if let Some(sender) = ready_tx.take() {
                        let _ = sender.send(());
                    }
                    let _ = control_rx.recv();
                }
                // drop(stream) 关连接；下一轮脚本等服务下一个请求。
            }
        });
        Self {
            port,
            ready: Some(ready_rx),
            control: Some(control_tx),
            handle: Some(handle),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 等到第一个 hold 响应已经写出（此时模型流还没结束）。
    pub async fn wait_ready(&mut self) {
        if let Some(receiver) = self.ready.take() {
            let _ = receiver.await;
        }
    }

    /// 放行当前 hold 的响应（关连接）。
    pub fn release(&mut self) {
        if let Some(sender) = self.control.take() {
            let _ = sender.send(());
        }
    }
}

impl Drop for ScriptedSseServer {
    fn drop(&mut self) {
        if let Some(sender) = self.control.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    let header_text = String::from_utf8_lossy(&buffer).to_string();
    let mut content_length = 0usize;
    for line in header_text.split("\r\n") {
        if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    let header_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(buffer.len(), |position| position + 4);
    let mut remaining = content_length.saturating_sub(buffer.len().saturating_sub(header_end));
    while remaining > 0 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => remaining = remaining.saturating_sub(read),
        }
    }
}
