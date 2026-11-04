use std::path::Path;

use model::AssistantStreamRecord;

/// 回放脚本/服务用到的错误。
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("读回放脚本失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("回放脚本格式不对：{0}")]
    Format(String),
}

/// 一次模型调用要回什么。
#[derive(Debug, Clone, PartialEq)]
pub enum ReplayCall {
    /// 正常回一段流（从日志里的紧凑记录编码成 SSE）。
    Stream(Vec<AssistantStreamRecord>),
    /// 2xx 之前就失败：HTTP 状态和响应体。
    Throw { status: u16, body: serde_json::Value },
    /// 回一段流之后把连接断掉（流到一半断开；没有 message_stop）。
    Cut(Vec<AssistantStreamRecord>),
    /// 不回应，直到请求被取消。
    Hang,
}

/// 一份回放脚本：按调用顺序排列的条目 + 可选的帧间隔。
#[derive(Debug, Clone)]
pub struct ReplayScript {
    pub calls: Vec<ReplayCall>,
    pub pace_ms: Option<u64>,
}

impl ReplayScript {
    /// 从一份会话日志推脚本（没有覆盖文件）。
    pub fn from_log(path: &Path) -> Result<Self, ReplayError> {
        Self::from_log_with_override(path, None)
    }

    /// 从一份会话日志推脚本，可选一份同目录的覆盖文件。
    pub fn from_log_with_override(
        path: &Path,
        override_path: Option<&Path>,
    ) -> Result<Self, ReplayError> {
        let text = std::fs::read_to_string(path)?;
        let mut calls = Vec::new();
        for (line_index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
                ReplayError::Format(format!("第 {} 行不是合法 JSON：{error}", line_index + 1))
            })?;
            let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if kind != "assistant/message" && kind != "assistant/attempt" {
                continue;
            }
            let event: session::Event = serde_json::from_value(value).map_err(|error| {
                ReplayError::Format(format!("第 {} 行的事件读不出来：{error}", line_index + 1))
            })?;
            match event.kind {
                session::EventKind::AssistantMessage { stream, .. }
                | session::EventKind::AssistantAttempt { stream, .. } => {
                    calls.push(ReplayCall::Stream(stream));
                }
                _ => {}
            }
        }
        let mut script = Self {
            calls,
            pace_ms: None,
        };
        if let Some(override_path) = override_path {
            script.apply_override(override_path)?;
        }
        Ok(script)
    }

    pub fn len(&self) -> usize {
        self.calls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    fn apply_override(&mut self, path: &Path) -> Result<(), ReplayError> {
        let text = std::fs::read_to_string(path)?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| ReplayError::Format(format!("覆盖文件不是合法 JSON：{error}")))?;
        if let Some(entries) = value.as_array() {
            self.calls = entries
                .iter()
                .map(call_from_value)
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(());
        }
        let patches = value
            .get("patches")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                ReplayError::Format("覆盖文件必须是条目数组，或 {patches:[{at, entry}]}".to_owned())
            })?;
        for patch in patches {
            let at = patch
                .get("at")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| ReplayError::Format("patch 缺少 at".to_owned()))?
                as usize;
            let entry = patch
                .get("entry")
                .ok_or_else(|| ReplayError::Format("patch 缺少 entry".to_owned()))?;
            let call = call_from_value(entry)?;
            if at < self.calls.len() {
                self.calls[at] = call;
            } else if at == self.calls.len() {
                self.calls.push(call);
            } else {
                return Err(ReplayError::Format(format!(
                    "patch 的 at={at} 超过脚本长度 {}",
                    self.calls.len()
                )));
            }
        }
        Ok(())
    }
}

/// 覆盖文件里的一个 entry。
pub fn call_from_value(value: &serde_json::Value) -> Result<ReplayCall, ReplayError> {
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ReplayError::Format("entry 缺少 kind".to_owned()))?;
    match kind {
        "stream" | "cut" => {
            let records: Vec<AssistantStreamRecord> = serde_json::from_value(
                value
                    .get("records")
                    .cloned()
                    .ok_or_else(|| ReplayError::Format("entry 缺少 records".to_owned()))?,
            )
            .map_err(|error| ReplayError::Format(format!("entry.records 读不出来：{error}")))?;
            if kind == "stream" {
                Ok(ReplayCall::Stream(records))
            } else {
                Ok(ReplayCall::Cut(records))
            }
        }
        "throw" => {
            let status = value
                .get("status")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(500);
            let status = u16::try_from(status)
                .map_err(|_| ReplayError::Format("entry.status 超出 HTTP 状态范围".to_owned()))?;
            let body = value
                .get("body")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"error": {"message": "回放脚本要求失败"}}));
            Ok(ReplayCall::Throw { status, body })
        }
        "hang" => Ok(ReplayCall::Hang),
        other => Err(ReplayError::Format(format!("不认识的 entry kind：{other}"))),
    }
}
