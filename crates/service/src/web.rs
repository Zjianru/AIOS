//! 网页工具：`web_search` 和 `web_fetch` 的模型面（名字、说明、参数、超时、开出条件）。
//!
//! `web_search` 走 Exa：加载过 `web` 且 Keyring 里有 Exa 的 key 才开出；每次执行现读 key，
//! 1–4 条查询去重后并发、轮流合并成最多 8 条来源（没有摘要的结果丢掉）。
//! `web_fetch`（带拦截的读网页）在下一张票接上。

use std::path::PathBuf;

use serde_json::{Value, json};
use tools::{
    Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolKey, ToolResult,
};

/// 一次网页工具调用的时间上限（和 Exa 请求、读网页共用 30 秒）。
const WEB_TOOL_TIMEOUT_MS: u64 = 30_000;

/// `web_search` 的装配：Keyring 位置和 Exa 根地址；服务启动/重开根会话时注入。
#[derive(Clone)]
pub(crate) struct WebSearchConfig {
    pub(crate) data_dir: PathBuf,
    pub(crate) identity: Option<PathBuf>,
    pub(crate) base_url: String,
}

/// 联网搜索工具。
pub(crate) struct WebSearchTool {
    parameters: Value,
    config: WebSearchConfig,
}

impl WebSearchTool {
    pub(crate) fn new(config: WebSearchConfig) -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "maxItems": 4,
                        "description": "1–4 search queries; their results are merged."
                    }
                },
                "required": ["queries"]
            }),
            config,
        }
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for current information. Returns up to 8 sources with summaries and URLs; cite the URLs you use."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(WEB_TOOL_TIMEOUT_MS)
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkillWithKey {
            skills: vec!["web".to_owned()],
            key: ToolKey::Exa,
        }
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let endpoint = format!("{}/search", self.config.base_url.trim_end_matches('/'));
            let identity = self
                .config
                .identity
                .as_ref()
                .map_or_else(|| "<default>".to_owned(), |path| path.display().to_string());
            ToolResult::error(
                call.id,
                ToolFailure::plain(format!(
                    "web_search is not wired up yet ({endpoint}; data {}; identity {identity})",
                    self.config.data_dir.display()
                )),
            )
        })
    }
}

/// 读网页工具。
pub struct WebFetchTool {
    parameters: Value,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The HTTP(S) URL to fetch."
                    }
                },
                "required": ["url"]
            }),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch the content of a specific HTTP(S) URL and return it as text. HTML pages are converted to Markdown."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(WEB_TOOL_TIMEOUT_MS)
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkillWithKey {
            skills: vec!["web".to_owned()],
            key: ToolKey::Exa,
        }
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            ToolResult::error(
                call.id,
                ToolFailure::plain("web_fetch is not wired up yet; answer without it"),
            )
        })
    }
}
