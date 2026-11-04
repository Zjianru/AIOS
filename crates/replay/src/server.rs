use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::{ReplayCall, ReplayError, ReplayScript, encode_sse, encode_sse_cut};

struct Shared {
    calls: Vec<ReplayCall>,
    pace_ms: Option<u64>,
    cursor: usize,
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
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let shared = Arc::new(Mutex::new(Shared {
        calls: script.calls,
        pace_ms: script.pace_ms,
        cursor: 0,
    }));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
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
    let (call, pace_ms) = {
        let mut state = shared.lock().await;
        let index = state.cursor;
        state.cursor += 1;
        (state.calls.get(index).cloned(), state.pace_ms)
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
    let _ = head;
    let _ = body;
    match call {
        ReplayCall::Throw { status, body } => {
            let body = body.to_string().into_bytes();
            write_response(&mut stream, status, "application/json", body).await
        }
        ReplayCall::Stream(records) => {
            let frames = encode_sse(&records);
            write_sse(&mut stream, &frames, pace_ms).await
        }
        ReplayCall::Cut(records) => {
            let frames = encode_sse_cut(&records);
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
