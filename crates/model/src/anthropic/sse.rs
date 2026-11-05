//! SSE 字节流的增量解析。

use crate::{ErrorCode, ModelError};

/// SSE 字节流的增量解析器：只吐出以空行结束的完整事件帧。
#[derive(Debug, Default)]
pub(super) struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    /// 追加收到的字节。
    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
        // SSE 允许 CRLF；JSON 事件数据里不会出现裸 CR，统一按 LF 处理。
        if self.buffer.contains(&b'\r') {
            self.buffer.retain(|byte| *byte != b'\r');
        }
    }

    /// 取出下一个完整帧；缓冲里没有完整帧时返回 `None`。
    pub(super) fn next_frame(&mut self) -> Result<Option<String>, ModelError> {
        let Some(position) = self.buffer.windows(2).position(|window| window == b"\n\n") else {
            return Ok(None);
        };
        let frame: Vec<u8> = self.buffer.drain(..position + 2).take(position).collect();
        String::from_utf8(frame).map(Some).map_err(|_| {
            ModelError::new(
                ErrorCode::MalformedResponse,
                "DeepSeek Messages 的 SSE 帧不是合法 UTF-8",
            )
        })
    }
}

/// 解析一帧里的 `data:` 载荷；心跳/注释帧返回 `None`。
pub(super) fn frame_data(frame: &str) -> Result<Option<String>, ModelError> {
    let mut data = String::new();
    for line in frame.split('\n') {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value);
        }
    }
    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some(data))
    }
}
