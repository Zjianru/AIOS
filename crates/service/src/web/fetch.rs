//! `web_fetch` 的模型面与执行：抓取、解码、HTML 转 Markdown、截断。

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use encoding_rs::Encoding;
use reqwest::Url;
use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy;
use serde_json::{Value, json};
use tools::{
    Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolKey, ToolResult,
};

use super::resolver::{PublicResolver, ResolveFailure};
use super::url_policy::{
    FetchError, WEB_FETCH_MAX_REDIRECTS, check_fetch_address, check_literal_address,
    is_followable_redirect, parse_fetch_url, same_origin, validate_parsed_url,
};
use super::{EXTERNAL_WEB_CONTENT_NOTICE, WEB_TOOL_TIMEOUT_MS};

/// 读网页的网络超时；工具外层也按同一个数限时。
pub(crate) const WEB_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// `web_fetch` 的装配：Keyring、AIOS 自己的监听地址、出站环境和超时。
#[derive(Clone)]
pub(crate) struct WebFetchConfig {
    pub(crate) data_dir: PathBuf,
    pub(crate) identity: Option<PathBuf>,
    /// AIOS 自己监听的地址和端口；服务绑定后填入，读网页时用来拒绝读自己。
    pub(crate) self_addr: Option<std::sync::Arc<std::sync::OnceLock<SocketAddr>>>,
    /// 出站 HTTP 的环境（代理）；生产读进程环境，测试从边界注入。
    pub(crate) http: model::http::HttpEnv,
    /// 网络读取超时；生产 30 秒，测试缩短以覆盖超时路径。
    pub(crate) timeout: Duration,
}

/// 读网页工具。
pub(crate) struct WebFetchTool {
    parameters: Value,
    config: WebFetchConfig,
    /// 直连时钉住 DNS 的解析器；测试里可以换查询函数（分类照做）。
    resolver: PublicResolver,
}

impl WebFetchTool {
    pub(crate) fn new(config: WebFetchConfig) -> Self {
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
            config,
            resolver: PublicResolver::new(),
        }
    }

    /// 测试用：换掉解析器的查询函数，分类照做。
    #[cfg(test)]
    pub(crate) fn with_lookup<F, Fut>(config: WebFetchConfig, lookup: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = std::io::Result<Vec<std::net::IpAddr>>> + Send + 'static,
    {
        let mut tool = Self::new(config);
        tool.resolver = PublicResolver::with_lookup(lookup);
        tool
    }

    /// 读一个链接：先过 URL 规则；直连时由解析器钉住 DNS，走代理时请求前解析一次；
    /// 每一跳重定向后再按当前模式判地址，最后抓取、解码、转 Markdown。
    async fn fetch(&self, input: &str) -> Result<String, ToolFailure> {
        let original = parse_fetch_url(input).map_err(FetchError::into_failure)?;
        ensure_exa_key(&self.config)?;
        let proxied = self
            .config
            .http
            .proxy_for(&original)
            .map_err(|_| FetchError::Client.into_failure())?
            .is_some();
        let client = if proxied {
            // 走代理时域名由代理解析，程序只能请求前解析一次、逐个查地址。
            check_fetch_address(&original, self.config.self_addr.as_deref())
                .await
                .map_err(FetchError::into_failure)?;
            model::http::client_with_env_without_redirects(self.config.http.clone())
                .map_err(|_| FetchError::Client.into_failure())?
        } else {
            // 直连时解析和分类都在解析器里做，连接用的就是查过的那份地址；
            // IP 字面量不经过解析器，照旧在请求前判。
            check_literal_address(&original, self.config.self_addr.as_deref())
                .map_err(FetchError::into_failure)?;
            direct_client(self.resolver.clone())?
        };
        let mut current = original.clone();
        let mut redirects = 0_u8;
        loop {
            let response = client
                .get(current.clone())
                .timeout(self.config.timeout)
                .send()
                .await
                .map_err(map_request_error)?;
            let status = response.status();
            if is_followable_redirect(status)
                && let Some(location) = response.headers().get(LOCATION)
            {
                let location = location
                    .to_str()
                    .map_err(|_| FetchError::InvalidUrl.into_failure())?;
                if redirects == WEB_FETCH_MAX_REDIRECTS {
                    return Err(FetchError::TooManyRedirects.into_failure());
                }
                redirects += 1;
                let next = current
                    .join(location)
                    .map_err(|_| FetchError::InvalidUrl.into_failure())?;
                validate_parsed_url(&next).map_err(FetchError::into_failure)?;
                // 只跟同源重定向，走不走代理不随跳变化；直连的下一跳由解析器在连接时判。
                if proxied {
                    check_fetch_address(&next, self.config.self_addr.as_deref())
                        .await
                        .map_err(FetchError::into_failure)?;
                } else {
                    check_literal_address(&next, self.config.self_addr.as_deref())
                        .map_err(FetchError::into_failure)?;
                }
                if !same_origin(&original, &next) {
                    return Err(FetchError::CrossOrigin(next.to_string()).into_failure());
                }
                current = next;
                continue;
            }
            return render_response(response, &current).await;
        }
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
            let url = match parse_fetch_arguments(&call.arguments) {
                Ok(url) => url,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            match self.fetch(&url).await {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// 正文最多留 20 万字；超出的截掉并写明。
const WEB_FETCH_MAX_BODY_CHARS: usize = 200_000;
/// 响应正文最多读 5 MiB：先于 charset 解码和字符截断，避免整包读进内存。
const WEB_FETCH_MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
/// 截断时照抄 DSH 的说明。
const FETCH_TRUNCATION_FOOTER: &str =
    "\n\n(Content truncated. Fetch a more specific URL or section for the full text.)";

fn no_fetch_key() -> String {
    format!(
        "web_fetch 需要 Exa 的 key：去设置页填（{}），再试一次",
        super::EXA_SETTINGS_PATH
    )
}

fn fetch_key_unreadable() -> String {
    format!(
        "Exa 的 key 读不出来；去设置页看一眼（{}）",
        super::EXA_SETTINGS_PATH
    )
}

/// 解析模型给的参数：一个非空 URL 字符串。
fn parse_fetch_arguments(arguments: &str) -> Result<String, ToolFailure> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| ToolFailure::plain("arguments must be a JSON object"))?;
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("url must be a non-empty string"))?;
    if url.trim().is_empty() {
        return Err(ToolFailure::plain("url must be a non-empty string"));
    }
    Ok(url.to_owned())
}
/// 执行时现查 Keyring：已加载 web 后删掉 key，要明确说去设置页填，不发请求。
fn ensure_exa_key(config: &WebFetchConfig) -> Result<(), ToolFailure> {
    let keyring =
        credentials::Keyring::open_read_only(config.data_dir.clone(), config.identity.as_deref())
            .map_err(|_| ToolFailure::plain(fetch_key_unreadable()))?;
    if keyring.exa_key_saved() {
        Ok(())
    } else {
        Err(ToolFailure::plain(no_fetch_key()))
    }
}

fn map_request_error(error: reqwest::Error) -> ToolFailure {
    if let Some(failure) = resolve_failure(&error) {
        return failure.into_failure();
    }
    if error.is_timeout() {
        FetchError::Timeout.into_failure()
    } else if error.is_connect() {
        FetchError::Connect.into_failure()
    } else {
        FetchError::Transport.into_failure()
    }
}

/// 解析器拒掉的地址藏在连接错误的 source 链里；认出来报分类文案，不报「连不上」。
fn resolve_failure(error: &reqwest::Error) -> Option<FetchError> {
    let mut source = error.source();
    while let Some(current) = source {
        if let Some(failure) = current.downcast_ref::<ResolveFailure>() {
            return Some(failure.to_fetch_error());
        }
        source = current.source();
    }
    None
}

/// 直连的 client：不读环境代理、不跟重定向；域名交给钉住 DNS 的解析器。
fn direct_client(resolver: PublicResolver) -> Result<reqwest::Client, ToolFailure> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .dns_resolver(Arc::new(resolver))
        .build()
        .map_err(|_| FetchError::Client.into_failure())
}

async fn render_response(response: reqwest::Response, url: &Url) -> Result<String, ToolFailure> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let kind = classify_content_type(content_type.as_deref()).ok_or_else(|| {
        FetchError::UnsupportedContentType(
            content_type
                .clone()
                .unwrap_or_else(|| "缺少 Content-Type".to_owned()),
        )
        .into_failure()
    })?;
    // 先按字节封顶再解码；charset 仍按 Content-Type 声明（和 reqwest::Response::text 一致）。
    let (raw, byte_truncated) = read_body_limited(response, WEB_FETCH_MAX_BODY_BYTES).await?;
    let charset = content_type
        .as_deref()
        .and_then(charset_of)
        .unwrap_or("utf-8");
    let encoding = Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (decoded, _, _) = encoding.decode(&raw);
    let (source, source_truncated) = take_chars(&decoded, WEB_FETCH_MAX_BODY_CHARS);
    let (body, converted_truncated) = match kind {
        FetchKind::Html => {
            let markdown = html_to_markdown(&source)?;
            take_chars(&markdown, WEB_FETCH_MAX_BODY_CHARS)
        }
        FetchKind::Text => (source, false),
    };
    Ok(format_fetch_output(
        url,
        status,
        &body,
        source_truncated || converted_truncated,
        byte_truncated,
    ))
}

/// 从 Content-Type 里取 charset（大小写不敏感）。
fn charset_of(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|part| {
        let (name, value) = part.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').trim())
    })
}

/// 流式读正文，最多读 `max` 字节；返回是否因为超过上限而停下。
///
/// 只多读一个已经到手的 chunk 来判断「恰好 max」和「超过 max」，不把整包读进内存。
async fn read_body_limited(
    mut response: reqwest::Response,
    max: usize,
) -> Result<(Vec<u8>, bool), ToolFailure> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_request_error)? {
        let room = max.saturating_sub(body.len());
        if chunk.len() > room {
            body.extend_from_slice(&chunk[..room]);
            return Ok((body, true));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, false))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetchKind {
    Html,
    Text,
}

/// 只解码 HTML 和文本；二进制或不支持的类型在发正文给模型之前拒绝。
fn classify_content_type(content_type: Option<&str>) -> Option<FetchKind> {
    let mime = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    if mime == "text/html" || mime == "application/xhtml+xml" {
        return Some(FetchKind::Html);
    }
    if mime.starts_with("text/")
        || mime == "application/json"
        || mime == "application/xml"
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
    {
        return Some(FetchKind::Text);
    }
    None
}

/// HTML 转 Markdown：标题用 ATX，列表用 -，表格用 GFM；脚本、样式和嵌入内容整段丢掉。
fn html_to_markdown(html: &str) -> Result<String, ToolFailure> {
    let options = htmd::options::Options {
        heading_style: htmd::options::HeadingStyle::Atx,
        bullet_list_marker: htmd::options::BulletListMarker::Dash,
        ul_bullet_spacing: 1,
        ol_number_spacing: 1,
        ..htmd::options::Options::default()
    };
    let converter = htmd::HtmlToMarkdown::builder()
        .options(options)
        .skip_tags(vec![
            "head", "title", "script", "style", "noscript", "template", "iframe", "object", "embed",
        ])
        .scripting_enabled(false)
        .build();
    converter
        .convert(html)
        .map(|markdown| markdown.trim().to_owned())
        .map_err(|_| FetchError::Convert.into_failure())
}

/// 取前 max 个字符；返回是否还有被截掉的。
fn take_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_owned(), false);
    }
    (text.chars().take(max).collect(), true)
}

/// 模型看到的正文：Fetched + 状态、照抄的不可信提示、正文，截断时再写一句说明。
fn format_fetch_output(
    url: &Url,
    status: reqwest::StatusCode,
    body: &str,
    truncated: bool,
    byte_truncated: bool,
) -> String {
    let status = status.as_u16();
    let mut output =
        format!("Fetched {url} (HTTP {status})\n\n{EXTERNAL_WEB_CONTENT_NOTICE}\n\n{body}");
    if truncated {
        output.push_str(FETCH_TRUNCATION_FOOTER);
    }
    if byte_truncated {
        output.push_str(&format!(
            "\n\n(Response body stopped at the {} MB limit.)",
            WEB_FETCH_MAX_BODY_BYTES / (1024 * 1024)
        ));
    }
    output
}
