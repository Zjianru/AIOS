//! tools 集成测试共用工具：会话上下文、工具调用、截断路径解析。

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use tools::{Tool, ToolCall, ToolContext, ToolResult, ToolSessionState};

/// 与真实会话无关的测试上下文：空观察表；任务目录放在独立临时目录，不污染 cwd。
pub fn context(cwd: &Path) -> ToolContext {
    let data_dir = tempfile::tempdir().expect("测试数据目录").keep();
    session_context_with_id(cwd, &data_dir, "sess-1")
}

pub fn session_context(cwd: &Path, data_dir: &Path) -> ToolContext {
    session_context_with_id(cwd, data_dir, "sess-1")
}

pub fn session_context_with_id(cwd: &Path, data_dir: &Path, session_id: &str) -> ToolContext {
    ToolContext::with_session(cwd, Arc::new(ToolSessionState::new(data_dir, session_id)))
}

pub async fn run(tool: &dyn Tool, arguments: Value, context: &ToolContext) -> ToolResult {
    tool.execute(
        ToolCall {
            id: "call_1".to_owned(),
            name: tool.name().to_owned(),
            arguments: arguments.to_string(),
        },
        context,
    )
    .await
}

/// 从模型的截断通知里取出第 nth 个全文路径。
pub fn truncated_path(content: &str, nth: usize) -> String {
    let marker = "[output truncated; full output: ";
    let mut start = 0;
    for _ in 0..=nth {
        let found = content[start..].find(marker).expect("有截断通知");
        start += found + marker.len();
    }
    let rest = &content[start..];
    rest[..rest.find(']').unwrap()].to_owned()
}
