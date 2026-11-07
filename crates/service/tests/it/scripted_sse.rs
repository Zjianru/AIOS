//! 一个最小的可编排 SSE 服务器：按脚本顺序服务请求，写出一段帧后可以挂住不关，
//! 并记录收到的请求体。只给取消/打断相关的 service 集成测试用，不是产品代码。
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// 一次请求的脚本：先写 frames；hold 时等测试放行再关连接。
pub struct SseScript {
    pub frames: String,
    pub hold: bool,
}

pub struct ScriptedSseServer {
    port: u16,
    ready: Option<tokio::sync::oneshot::Receiver<()>>,
    control: Option<std::sync::mpsc::Sender<()>>,
    stopped: std::sync::Arc<AtomicBool>,
    bodies: std::sync::Arc<Mutex<Vec<Vec<u8>>>>,
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
        let stopped = std::sync::Arc::new(AtomicBool::new(false));
        let stopped_in_thread = std::sync::Arc::clone(&stopped);
        let bodies = std::sync::Arc::new(Mutex::new(Vec::new()));
        let bodies_in_thread = std::sync::Arc::clone(&bodies);
        let mut ready_tx = Some(ready_tx);
        let handle = std::thread::spawn(move || {
            for script in scripts {
                let Some(mut stream) = accept_while_running(&listener, &stopped_in_thread) else {
                    return;
                };
                // macOS 上 accept 出来的 socket 可能继承 listener 的非阻塞标志。
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

    /// 等到当前 hold 的响应已经写出（此时模型流还没结束）。
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

    /// 已收到的请求体，按请求顺序。
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
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(_) => return None,
        }
    }
}

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
