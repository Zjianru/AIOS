//! DeepSeek Messages 适配器的连接默认值。

use crate::Effort;

/// 默认 Messages 端点（spec 实现决定）。
pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/anthropic";

/// 默认单次请求输出上限（spec 实现决定）。
pub const DEFAULT_MAX_TOKENS: u32 = 256_000;

/// 流读取的空闲超时（spec 实现决定：300 秒）。
pub(crate) const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// DeepSeek Messages 适配器的连接与默认值。
#[derive(Debug, Clone)]
pub struct DeepSeekConfig {
    /// Messages 协议的根地址；请求发往 `<base_url>/v1/messages`。
    pub base_url: String,
    /// API key，从环境变量读（D34）；不写进日志。
    pub api_key: String,
    /// 模型 id。
    pub model: String,
    /// 单次请求输出上限。
    pub max_tokens: u32,
    /// 推理强度。
    pub effort: Effort,
}

impl Default for DeepSeekConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key: String::new(),
            model: "deepseek-flash".to_owned(),
            max_tokens: DEFAULT_MAX_TOKENS,
            effort: Effort::High,
        }
    }
}
