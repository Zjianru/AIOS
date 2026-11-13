use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::{ReplayCall, ReplayError, ReplayRoute, ReplayScript, encode_sse, encode_sse_cut};

struct Shared {
    calls: Vec<ReplayCall>,
    pace_ms: Option<u64>,
    cursor: usize,
    routes: Vec<RouteState>,
    last_model: Option<String>,
    /// T012.9：最近一次请求的完整请求体（e2e 断言思考档用）。
    last_request: Option<serde_json::Value>,
    /// 假厂商的 /v1/models：空表照旧 404；非空时回这个列表（保存连接会来拉一次）。
    vendor_models: Vec<String>,
}

/// 一条路由的运行状态：匹配串和这条会话自己的游标。
struct RouteState {
    contains: String,
    calls: Vec<ReplayCall>,
    pace_ms: Option<u64>,
    cursor: usize,
}

impl RouteState {
    fn new(route: ReplayRoute) -> Self {
        Self {
            contains: route.contains,
            calls: route.calls,
            pace_ms: route.pace_ms,
            cursor: 0,
        }
    }
}

/// 起好的回放服务（地址 + 后台任务）。
pub struct ReplayServer {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl ReplayServer {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn shutdown(self) {
        self.task.abort();
    }
}

/// 起一个本机回放服务：说 Messages 协议，第 N 次请求回第 N 份脚本。
pub async fn start_replay_server(script: ReplayScript) -> Result<ReplayServer, ReplayError> {
    start_replay_server_with_models(script, Vec::new()).await
}

/// 同 start_replay_server，外加假厂商的 /v1/models 列表（空表照旧 404）。
pub async fn start_replay_server_with_models(
    script: ReplayScript,
    vendor_models: Vec<String>,
) -> Result<ReplayServer, ReplayError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let shared = Arc::new(Mutex::new(Shared {
        calls: script.calls,
        pace_ms: script.pace_ms,
        cursor: 0,
        routes: script.routes.into_iter().map(RouteState::new).collect(),
        last_model: None,
        last_request: None,
        vendor_models,
    }));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "单连接处理出错只断开该连接，回放服务继续接收"
                )]
                let _ = handle(stream, shared).await;
            });
        }
    });
    Ok(ReplayServer { addr, task })
}

async fn handle(mut stream: TcpStream, shared: Arc<Mutex<Shared>>) -> std::io::Result<()> {
    let Some((head, body)) = read_request(&mut stream).await? else {
        return Ok(());
    };
    let path = request_path(&head);
    // 假模型的旧行为：x-api-key 是 bad-key 时回 401（first-use 错 key 用例）；不消耗脚本。
    if header_value(&head, "x-api-key").as_deref() == Some("bad-key") {
        let body = serde_json::json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "invalid api key"}
        })
        .to_string();
        return write_response(
            &mut stream,
            401,
            "application/json",
            body.as_bytes().to_vec(),
        )
        .await;
    }
    // 保存连接/available-models 会请求厂商 /v1/models：夹具按给定列表回，空表 404，不消耗回放脚本。
    if path.contains("/v1/models") {
        let vendor_models = shared.lock().await.vendor_models.clone();
        if vendor_models.is_empty() {
            let body = br#"{"error":{"message":"fixture has no /v1/models"}}"#.to_vec();
            return write_response(&mut stream, 404, "application/json", body).await;
        }
        let body = serde_json::json!({
            "data": vendor_models
                .iter()
                // 不带 display_name：合并时保留目录里的显示名（e2e 断言目录名）。
                .map(|id| serde_json::json!({"id": id, "object": "model"}))
                .collect::<Vec<_>>()
        })
        .to_string()
        .into_bytes();
        return write_response(&mut stream, 200, "application/json", body).await;
    }
    // T010.2：测试夹具要断言下一轮用的模型；控制端点回最近一次请求的 model。
    // T012.9：测试夹具要断言下一轮请求的思考档；控制端点回最近一次请求的完整 body。
    if path.contains("__replay/last-request") {
        let request = shared.lock().await.last_request.clone();
        let body = serde_json::json!({ "request": request })
            .to_string()
            .into_bytes();
        return write_response(&mut stream, 200, "application/json", body).await;
    }
    if path.contains("__replay/last-model") {
        let model = shared.lock().await.last_model.clone();
        let body = serde_json::json!({ "model": model })
            .to_string()
            .into_bytes();
        return write_response(&mut stream, 200, "application/json", body).await;
    }
    let body_value: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    let requested_model = body_value
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let body_text = String::from_utf8_lossy(&body);
    let picked = {
        let mut state = shared.lock().await;
        if let Some(model) = requested_model {
            state.last_model = Some(model);
        }
        if !body_value.is_null() {
            state.last_request = Some(body_value.clone());
        }
        if state.routes.is_empty() {
            let index = state.cursor;
            state.cursor += 1;
            Some((state.calls.get(index).cloned(), state.pace_ms))
        } else {
            // 两个会话的请求会交错：先按请求体认会话，各用各的游标。
            state
                .routes
                .iter_mut()
                .find(|route| body_text.contains(&route.contains))
                .map(|route| {
                    let index = route.cursor;
                    route.cursor += 1;
                    (route.calls.get(index).cloned(), route.pace_ms)
                })
        }
    };
    let Some((call, pace_ms)) = picked else {
        return write_response(
            &mut stream,
            500,
            "application/json",
            "{\"error\":{\"message\":\"没有匹配的路由\"}}"
                .as_bytes()
                .to_vec(),
        )
        .await;
    };
    let Some(call) = call else {
        return write_response(
            &mut stream,
            500,
            "application/json",
            "{\"error\":{\"message\":\"回放脚本已用完\"}}"
                .as_bytes()
                .to_vec(),
        )
        .await;
    };
    match call {
        ReplayCall::Throw { status, body } => {
            let body = body.to_string().into_bytes();
            write_response(&mut stream, status, "application/json", body).await
        }
        ReplayCall::Stream(records) => {
            let frames = substitute_graph_id(encode_sse(&records), body_text.as_ref());
            write_sse(&mut stream, &frames, pace_ms).await
        }
        ReplayCall::Cut(records) => {
            let frames = substitute_graph_id(encode_sse_cut(&records), body_text.as_ref());
            write_sse(&mut stream, &frames, pace_ms).await
        }
        ReplayCall::Hang => loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        },
    }
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut buffer = Vec::new();
    loop {
        let mut chunk = [0_u8; 2048];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
        let Some(header_end) = find_header_end(&buffer) else {
            continue;
        };
        let content_length = content_length(&buffer[..header_end]);
        let body_start = header_end + 4;
        while buffer.len() < body_start + content_length {
            let mut chunk = [0_u8; 2048];
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        let body = buffer
            .get(body_start..body_start + content_length)
            .unwrap_or_default()
            .to_vec();
        return Ok(Some((buffer[..header_end].to_vec(), body)));
    }
}

fn header_value(head: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    let prefix = format!("{name}:");
    for line in text.split(
        "
",
    ) {
        if let Some(value) = line.to_ascii_lowercase().strip_prefix(&prefix) {
            return Some(value.trim().to_owned());
        }
    }
    None
}

/// T052：任务图的图 id 只有运行时才知道；脚本里用 __GRAPH_ID__ 占位，回放时从请求体里换成真 id。
fn substitute_graph_id(frames: Vec<String>, request_body: &str) -> Vec<String> {
    if !frames.iter().any(|frame| frame.contains("__GRAPH_ID__")) {
        return frames;
    }
    let Some(rest) = request_body.split("已建任务图 ").nth(1) else {
        return frames;
    };
    let id: String = rest
        .chars()
        .take_while(|value| value.is_ascii_hexdigit())
        .collect();
    if id.is_empty() {
        return frames;
    }
    frames
        .into_iter()
        .map(|frame| frame.replace("__GRAPH_ID__", &id))
        .collect()
}

fn request_path(head: &[u8]) -> String {
    let text = String::from_utf8_lossy(head);
    text.split(' ').nth(1).unwrap_or_default().to_owned()
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(head: &[u8]) -> usize {
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    for line in text.split("\r\n") {
        if let Some(value) = line.strip_prefix("content-length:") {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

async fn write_sse(
    stream: &mut TcpStream,
    frames: &[String],
    pace_ms: Option<u64>,
) -> std::io::Result<()> {
    let body: String = frames.concat();
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    let pace = Duration::from_millis(pace_ms.unwrap_or(0));
    for (index, frame) in frames.iter().enumerate() {
        if index > 0 && !pace.is_zero() {
            tokio::time::sleep(pace).await;
        }
        stream.write_all(frame.as_bytes()).await?;
        stream.flush().await?;
    }
    stream.flush().await
}
