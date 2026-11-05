//! agent 集成测试共用的小工具（mock 响应器、可控测试工具、可控 SSE 服务器）。
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
        // 先在锁内取响应、释放锁，再在锁外 panic：脚本耗尽时后面的重试不会撞上毒锁。
        let next = {
            let mut responses = self
                .responses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            responses.pop_front()
        };
        match next {
            Some(response) => response,
            None => panic!("mock 收到的请求比脚本多"),
        }
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

/// 一个最小的、可控制节奏的 SSE 服务器：按脚本依次服务请求，并记录请求体。
/// 只给取消 / 中断相关的集成测试用；不是产品代码。
pub struct ScriptedSseServer {
    port: u16,
    ready: Option<tokio::sync::oneshot::Receiver<()>>,
    control: Option<std::sync::mpsc::Sender<()>>,
    stopped: Arc<AtomicBool>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    handle: Option<JoinHandle<()>>,
}

impl ScriptedSseServer {
    pub fn start(scripts: Vec<SseScript>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定测试 SSE 端口");
        listener
            .set_nonblocking(true)
            .expect("测试 SSE 端口设成非阻塞");
        let port = listener.local_addr().expect("读取测试端口").port();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (control_tx, control_rx) = std::sync::mpsc::channel::<()>();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_in_thread = Arc::clone(&stopped);
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let bodies_in_thread = Arc::clone(&bodies);
        let mut ready_tx = Some(ready_tx);
        let handle = std::thread::spawn(move || {
            for script in scripts {
                let Some(mut stream) = accept_while_running(&listener, &stopped_in_thread) else {
                    return;
                };
                // macOS 上 accept 出来的 socket 可能继承 listener 的非阻塞标志；
                // 这里显式恢复阻塞，否则请求体还没到时 read 会 WouldBlock、记录不到。
                let _ = stream.set_nonblocking(false);
                if let Some(body) = read_request(&mut stream) {
                    bodies_in_thread
                        .lock()
                        .expect("测试 SSE 请求记录锁被毒化")
                        .push(body);
                }
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
            stopped,
            bodies,
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

    /// 已收到的请求体，按请求顺序（断言「第二个请求带了取消后投递的消息」用）。
    pub fn request_bodies(&self) -> Vec<Vec<u8>> {
        self.bodies
            .lock()
            .expect("测试 SSE 请求记录锁被毒化")
            .clone()
    }
}

impl Drop for ScriptedSseServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(sender) = self.control.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 非阻塞 accept：测试失败/提前结束时 Drop 能很快把线程收回来。
fn accept_while_running(
    listener: &TcpListener,
    stopped: &AtomicBool,
) -> Option<std::net::TcpStream> {
    loop {
        if stopped.load(Ordering::SeqCst) {
            return None;
        }
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                // T009.H6：accept 非阻塞轮询间隔，等测试连接；属真实等待
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(_) => return None,
        }
    }
}

/// 读一个 HTTP 请求（头 + content-length 指定的体），返回请求体字节。
fn read_request(stream: &mut std::net::TcpStream) -> Option<Vec<u8>> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok()?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
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
    let body_have = buffer.len().saturating_sub(header_end);
    let mut remaining = content_length.saturating_sub(body_have);
    while remaining > 0 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let take = read.min(remaining);
                buffer.extend_from_slice(&chunk[..take]);
                remaining -= take;
            }
        }
    }
    Some(buffer[header_end..].to_vec())
}

/// 只接一个请求、读完请求后**先不写响应头**的服务器：给「取消打断等响应头」测试用。
/// 测试等 wait_request()，取消后由 Drop 放行线程（不写响应、直接关连接）。
pub struct HeadersGatedServer {
    port: u16,
    ready: Option<tokio::sync::oneshot::Receiver<()>>,
    release: Option<std::sync::mpsc::Sender<()>>,
    stopped: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HeadersGatedServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定测试端口");
        listener.set_nonblocking(true).expect("测试端口设成非阻塞");
        let port = listener.local_addr().expect("读取测试端口").port();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_in_thread = Arc::clone(&stopped);
        let mut ready_tx = Some(ready_tx);
        let handle = std::thread::spawn(move || {
            let Some(mut stream) = accept_while_running(&listener, &stopped_in_thread) else {
                return;
            };
            let _ = stream.set_nonblocking(false);
            let _ = read_request(&mut stream);
            if let Some(sender) = ready_tx.take() {
                let _ = sender.send(());
            }
            let _ = release_rx.recv();
            // 取消后客户端已经断开；不再写响应，直接关连接。
        });
        Self {
            port,
            ready: Some(ready_rx),
            release: Some(release_tx),
            stopped,
            handle: Some(handle),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 等到请求已经收到、响应头还没写。
    pub async fn wait_request(&mut self) {
        if let Some(receiver) = self.ready.take() {
            let _ = receiver.await;
        }
    }
}

impl Drop for HeadersGatedServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(sender) = self.release.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 两阶段 SSE 脚本：先写 prefix，可选 hold 等测试放行，再写 suffix。
pub struct PhaseScript {
    pub prefix: String,
    pub suffix: String,
    pub hold: bool,
}

/// 支持「先写一半、按住、再写剩下」的 SSE 服务器：给 max-tokens 后还有后续 step 的测试用。
pub struct PhaseSseServer {
    port: u16,
    ready: Option<tokio::sync::oneshot::Receiver<()>>,
    control: Option<std::sync::mpsc::Sender<()>>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    stopped: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PhaseSseServer {
    pub fn start(scripts: Vec<PhaseScript>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定测试 SSE 端口");
        listener
            .set_nonblocking(true)
            .expect("测试 SSE 端口设成非阻塞");
        let port = listener.local_addr().expect("读取测试端口").port();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (control_tx, control_rx) = std::sync::mpsc::channel::<()>();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_in_thread = Arc::clone(&stopped);
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let bodies_in_thread = Arc::clone(&bodies);
        let mut ready_tx = Some(ready_tx);
        let handle = std::thread::spawn(move || {
            for script in scripts {
                let Some(mut stream) = accept_while_running(&listener, &stopped_in_thread) else {
                    return;
                };
                let _ = stream.set_nonblocking(false);
                if let Some(body) = read_request(&mut stream) {
                    bodies_in_thread
                        .lock()
                        .expect("测试 SSE 请求记录锁被毒化")
                        .push(body);
                }
                let header = "HTTP/1.1 200 OK
content-type: text/event-stream
connection: close

";
                if stream.write_all(header.as_bytes()).is_err() {
                    continue;
                }
                if stream.write_all(script.prefix.as_bytes()).is_err() {
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
                if stream.write_all(script.suffix.as_bytes()).is_err() {
                    continue;
                }
                let _ = stream.flush();
            }
        });
        Self {
            port,
            ready: Some(ready_rx),
            control: Some(control_tx),
            bodies,
            stopped,
            handle: Some(handle),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 等到 prefix 已经写出、suffix 还没写。
    pub async fn wait_ready(&mut self) {
        if let Some(receiver) = self.ready.take() {
            let _ = receiver.await;
        }
    }

    /// 放行第一个 hold 的脚本（写 suffix）。
    pub fn release(&mut self) {
        if let Some(sender) = self.control.take() {
            let _ = sender.send(());
        }
    }

    pub fn request_bodies(&self) -> Vec<Vec<u8>> {
        self.bodies
            .lock()
            .expect("测试 SSE 请求记录锁被毒化")
            .clone()
    }
}

impl Drop for PhaseSseServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(sender) = self.control.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 回放一组带输入的 tool_use 调用 SSE（写类工具的参数要真实输入）。
pub fn tool_calls_with_inputs_sse(calls: &[(&str, &str, serde_json::Value)]) -> String {
    let mut body = String::from(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_tools\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"deepseek-flash\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
    );
    for (index, (id, name, input)) in calls.iter().enumerate() {
        let id = serde_json::json!(id).to_string();
        let name = serde_json::json!(name).to_string();
        body.push_str(&format!(
            "event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":{index},\"content_block\":{{\"type\":\"tool_use\",\"id\":{id},\"name\":{name},\"input\":{{}}}}}}\n\n"
        ));
        let partial = serde_json::json!(input.to_string()).to_string();
        body.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":{index},\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{partial}}}}}\n\n"
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

/// 测试共用：指向 mock 服务的模型配置（5 个测试文件原来各写一遍）。
pub fn model_config(base_url: &str) -> model::DeepSeekConfig {
    model::DeepSeekConfig {
        base_url: base_url.to_owned(),
        model: "deepseek-flash".to_owned(),
        max_tokens: 2048,
        context_window: model::DEFAULT_CONTEXT_WINDOW,
        effort: model::Effort::new("high"),
    }
}

/// 测试共用：建一个临时目录上的 agent，工具集合由调用方给。
/// 测试默认配置：指令/skill 根落在数据目录下，避免读到本机 ~/.agents。
pub fn isolated_config(data_dir: &std::path::Path) -> agent::AgentConfig {
    agent::AgentConfig {
        instruction_roots: vec![data_dir.join("agents-home")],
        skill_roots: vec![data_dir.join("skills-home")],
        ..agent::AgentConfig::default()
    }
}

pub async fn setup_agent(
    base_url: &str,
    tools: tools::ToolSet,
) -> (
    agent::Agent,
    tempfile::TempDir,
    tempfile::TempDir,
    session::SessionId,
) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = agent::Agent::create(
        isolated_config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(model_config(base_url), test_credential())
                .unwrap(),
            tools,
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, cwd, id)
}

/// T012.3：指定模型/档的测试会话（日志 actual≠wanted 用）。
pub async fn setup_agent_with_model(
    base_url: &str,
    model_id: &str,
    effort: model::Effort,
) -> (
    agent::Agent,
    tempfile::TempDir,
    tempfile::TempDir,
    session::SessionId,
) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let model = model::DeepSeek::with_credentials(
        model::DeepSeekConfig {
            base_url: base_url.to_owned(),
            model: model_id.to_owned(),
            max_tokens: 64_000,
            context_window: 1_000_000,
            effort,
        },
        test_credential(),
    )
    .unwrap();
    let agent = agent::Agent::create(
        isolated_config(data_dir.path()),
        agent::Deps {
            model,
            tools: tools::ToolSet::empty(),
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, cwd, id)
}

/// 自定义工作目录与指令根的测试会话（T005.5 指令文件用）。
pub async fn setup_agent_custom(
    base_url: &str,
    tools: tools::ToolSet,
    cwd: &std::path::Path,
    instruction_roots: Vec<std::path::PathBuf>,
) -> (agent::Agent, tempfile::TempDir, session::SessionId) {
    let data_dir = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd).unwrap();
    let id = store.id().clone();
    let config = agent::AgentConfig {
        instruction_roots,
        ..isolated_config(data_dir.path())
    };
    let agent = agent::Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(model_config(base_url), test_credential())
                .unwrap(),
            tools,
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, id)
}

/// 自定义工作目录、指令根与 skill 根的测试会话（T005.6 skill 目录用）。
pub async fn setup_agent_with_roots(
    base_url: &str,
    tools: tools::ToolSet,
    cwd: &std::path::Path,
    skill_roots: Vec<std::path::PathBuf>,
) -> (agent::Agent, tempfile::TempDir, session::SessionId) {
    let data_dir = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd).unwrap();
    let id = store.id().clone();
    let config = agent::AgentConfig {
        skill_roots,
        ..isolated_config(data_dir.path())
    };
    let agent = agent::Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::with_credentials(model_config(base_url), test_credential())
                .unwrap(),
            tools,
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, id)
}

/// 测试共用：等 idle；超时按秒数。
pub async fn wait_idle(agent: &agent::Agent, seconds: u64) {
    tokio::time::timeout(std::time::Duration::from_secs(seconds), agent.idle())
        .await
        .expect("idle() 超时");
}

/// T008.F9：测试凭证来源——产品删掉固定 key 构造入口后，测试显式注入一个来源。
pub struct TestCredential;

impl model::CredentialSource for TestCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok("test-key".to_owned())
    }
}

pub fn test_credential() -> std::sync::Arc<dyn model::CredentialSource> {
    std::sync::Arc::new(TestCredential)
}
