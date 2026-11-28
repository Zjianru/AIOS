//! 搜索服务商注册表：20 家的地址/key 需求/请求拼法/响应解析都在这里。
//!
//! 五家真通道先打通（Exa/Brave/Serper/DuckDuckGo/SearXNG）；其余 15 家照 Cherry
//! 850703a 的 provider 形状抄请求与解析，测试用录好的响应喂。这里不读 Keyring、
//! 不决定工具门；只回答「拿这把 key 向这个地址发这一条查询，怎么拼、怎么解析」。

use std::time::Duration;

use serde_json::Value;

use tools::ToolFailure;

/// 一次搜索合并后最多给的来源数。
pub(crate) const WEB_SEARCH_MAX_RESULTS: usize = 8;

/// key 的需求分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyNeed {
    /// 必须有 key。
    Required,
    /// 可以不填；没填也能用。
    Optional,
    /// 不要 key（DuckDuckGo / SearXNG）。
    None,
    /// 用模型连接里的 key（智谱），或在 Keyring 内从模型连接同步（DeepSeek）。
    Llm,
}

/// 一家服务商的静态事实。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderSpec {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) host: &'static str,
    pub(crate) key_need: KeyNeed,
    pub(crate) key_site: &'static str,
    pub(crate) site: &'static str,
    pub(crate) basic_auth: bool,
}

const fn p(
    id: &'static str,
    name: &'static str,
    host: &'static str,
    key_need: KeyNeed,
    key_site: &'static str,
    site: &'static str,
) -> ProviderSpec {
    ProviderSpec {
        id,
        name,
        host,
        key_need,
        key_site,
        site,
        basic_auth: false,
    }
}

pub(crate) const PROVIDERS: [ProviderSpec; 20] = [
    p(
        "exa",
        "Exa",
        "https://api.exa.ai",
        KeyNeed::Required,
        "https://dashboard.exa.ai/api-keys",
        "https://exa.ai",
    ),
    p(
        "brave",
        "Brave Search",
        "https://api.search.brave.com",
        KeyNeed::Required,
        "https://api.search.brave.com/app/keys",
        "https://brave.com/search/api/",
    ),
    p(
        "serper",
        "Serper",
        "https://google.serper.dev",
        KeyNeed::Required,
        "https://serper.dev/api-key",
        "https://serper.dev",
    ),
    p(
        "duckduckgo",
        "DuckDuckGo",
        "https://html.duckduckgo.com",
        KeyNeed::None,
        "",
        "https://duckduckgo.com",
    ),
    ProviderSpec {
        basic_auth: true,
        ..p(
            "searxng",
            "SearXNG",
            "",
            KeyNeed::None,
            "",
            "https://docs.searxng.org",
        )
    },
    p(
        "tavily",
        "Tavily",
        "https://api.tavily.com",
        KeyNeed::Required,
        "https://app.tavily.com/home",
        "https://tavily.com",
    ),
    p(
        "exa-mcp",
        "Exa MCP",
        "https://mcp.exa.ai/mcp",
        KeyNeed::Optional,
        "https://dashboard.exa.ai/api-keys",
        "https://exa.ai",
    ),
    p(
        "bocha",
        "博查",
        "https://api.bochaai.com",
        KeyNeed::Required,
        "https://open.bochaai.com",
        "https://bochaai.com",
    ),
    p(
        "zhipu",
        "智谱",
        "https://open.bigmodel.cn/api/paas/v4/web_search",
        KeyNeed::Llm,
        "设置 › 模型与连接",
        "https://open.bigmodel.cn",
    ),
    p(
        "deepseek",
        "DeepSeek",
        "https://api.deepseek.com",
        KeyNeed::Llm,
        "设置 › 模型与连接",
        "https://platform.deepseek.com",
    ),
    p(
        "yuanbao",
        "腾讯元宝",
        "https://api.wsa.cloud.tencent.com",
        KeyNeed::Required,
        "https://cloud.tencent.com/product/wsa",
        "https://yuanbao.tencent.com",
    ),
    p(
        "stepfun",
        "阶跃星辰",
        "https://api.stepfun.com",
        KeyNeed::Required,
        "https://platform.stepfun.com/interface-key",
        "https://stepfun.com",
    ),
    p(
        "jina",
        "Jina",
        "https://s.jina.ai",
        KeyNeed::Optional,
        "https://jina.ai/api-dashboard",
        "https://jina.ai",
    ),
    p(
        "firecrawl",
        "Firecrawl",
        "https://api.firecrawl.dev",
        KeyNeed::Optional,
        "https://www.firecrawl.dev/app/api-keys",
        "https://firecrawl.dev",
    ),
    p(
        "parallel",
        "Parallel",
        "https://api.parallel.ai",
        KeyNeed::Required,
        "https://platform.parallel.ai",
        "https://parallel.ai",
    ),
    p(
        "querit",
        "Querit",
        "https://api.querit.ai",
        KeyNeed::Required,
        "https://querit.ai",
        "https://querit.ai",
    ),
    p(
        "searchapi",
        "SearchApi",
        "https://www.searchapi.io",
        KeyNeed::Required,
        "https://www.searchapi.io/dashboard",
        "https://www.searchapi.io",
    ),
    p(
        "serpkite",
        "SerpKite",
        "https://api.serpkite.com",
        KeyNeed::Required,
        "https://serpkite.com",
        "https://serpkite.com",
    ),
    p(
        "serply",
        "Serply",
        "https://api.serply.io",
        KeyNeed::Required,
        "https://serply.io",
        "https://serply.io",
    ),
    p(
        "desearch",
        "Desearch",
        "https://api.desearch.ai",
        KeyNeed::Required,
        "https://desearch.ai",
        "https://desearch.ai",
    ),
];

pub(crate) fn provider(id: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|spec| spec.id == id)
}

/// 用户的覆盖地址；空/未填用内置。
pub(crate) fn host(spec: &ProviderSpec, override_host: Option<&str>) -> String {
    override_host
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(spec.host)
        .trim_end_matches('/')
        .to_owned()
}

/// 一次搜索失败的原因；不把 key 或请求头写进文案。
#[derive(Debug, Clone, Copy)]
pub(crate) enum SearchError {
    Client,
    Connect,
    Timeout,
    Transport,
    Http(u16),
    Body,
    Task,
}

impl SearchError {
    pub(crate) fn into_failure(self, name: &str) -> ToolFailure {
        match self {
            Self::Client => ToolFailure::plain("建不出搜索用的 HTTP 客户端；稍后再试"),
            Self::Connect => ToolFailure::plain(format!("连不上 {name}；检查网络后再试")),
            Self::Timeout => ToolFailure::plain(format!("等 {name} 回应超时了（30 秒）；稍后再试")),
            Self::Transport => ToolFailure::plain(format!("向 {name} 发请求时出错了；稍后再试")),
            Self::Http(status @ (401 | 403)) => ToolFailure::plain(format!(
                "{name} 拒绝了这把 key（{status}）；去设置页 › 模型与连接 › 联网搜索 检查"
            )),
            Self::Http(status) => ToolFailure::plain(format!("{name} 那边出错了（{status}）")),
            Self::Body => ToolFailure::plain(format!("{name} 返回的内容看不懂")),
            Self::Task => ToolFailure::plain("搜网页的任务没有正常结束"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchSource {
    pub(crate) url: String,
    pub(crate) title: Option<String>,
    pub(crate) summary: String,
    pub(crate) published_at: Option<String>,
}

fn source(
    url: impl Into<String>,
    title: Option<String>,
    summary: impl Into<String>,
) -> SearchSource {
    SearchSource {
        url: url.into(),
        title: title.filter(|value| !value.trim().is_empty()),
        summary: summary.into().trim().to_owned(),
        published_at: None,
    }
}

async fn response_json(response: reqwest::Response) -> Result<Value, SearchError> {
    let status = response.status();
    if !status.is_success() {
        return Err(SearchError::Http(status.as_u16()));
    }
    let bytes = response.bytes().await.map_err(|error| {
        if error.is_timeout() {
            SearchError::Timeout
        } else {
            SearchError::Transport
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|_| SearchError::Body)
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").trim().to_owned()
}

fn optional(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// 向一家服务商发一条查询。key 只在这一次请求里用；basic 只给 SearXNG。
pub(crate) async fn search_one(
    spec: &ProviderSpec,
    base_url: &str,
    key: Option<&str>,
    basic: Option<(&str, &str)>,
    query: &str,
) -> Result<Vec<SearchSource>, SearchError> {
    search_one_with_timeout(spec, base_url, key, basic, query, Duration::from_secs(30)).await
}

/// 带显式超时的请求；单测用短超时验证超时映射，产品只走上面 30 秒。
pub(crate) async fn search_one_with_timeout(
    spec: &ProviderSpec,
    base_url: &str,
    key: Option<&str>,
    basic: Option<(&str, &str)>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let client = model::http::client_without_redirects().map_err(|_| SearchError::Client)?;
    let base = base_url.trim_end_matches('/');
    match spec.id {
        "exa" => exa(&client, base, key, query, timeout).await,
        "brave" => brave(&client, base, key, query, timeout).await,
        "serper" => serper(&client, base, key, query, timeout).await,
        "duckduckgo" => duckduckgo(&client, base, query, timeout).await,
        "searxng" => searxng(&client, base, basic, query, timeout).await,
        "tavily" => tavily(&client, base, key, query, timeout).await,
        "exa-mcp" => exa_mcp(&client, base, key, query, timeout).await,
        "bocha" => bocha(&client, base, key, query, timeout).await,
        "zhipu" => zhipu(&client, base, key, query, timeout).await,
        "deepseek" => deepseek(&client, base, key, query, timeout).await,
        "yuanbao" => yuanbao(&client, base, key, query, timeout).await,
        "stepfun" => stepfun(&client, base, key, query, timeout).await,
        "jina" => jina(&client, base, key, query, timeout).await,
        "firecrawl" => firecrawl(&client, base, key, query, timeout).await,
        "parallel" => parallel(&client, base, key, query, timeout).await,
        "querit" => querit(&client, base, key, query, timeout).await,
        "searchapi" => searchapi(&client, base, key, query, timeout).await,
        "serpkite" => serpkite(&client, base, key, query, timeout).await,
        "serply" => serply(&client, base, key, query, timeout).await,
        "desearch" => desearch(&client, base, key, query, timeout).await,
        _ => Err(SearchError::Client),
    }
}

fn collect(items: impl IntoIterator<Item = Option<SearchSource>>) -> Vec<SearchSource> {
    items
        .into_iter()
        .flatten()
        .filter(|source| !source.summary.is_empty() && !source.url.is_empty())
        .take(WEB_SEARCH_MAX_RESULTS)
        .collect()
}

fn js(value: &Value, key: &str) -> String {
    text(value.get(key).unwrap_or(&Value::Null))
}

async fn exa(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let key = key.unwrap_or("").to_owned();
    let response = client
        .post(format!("{base}/search"))
        .timeout(timeout)
        .header("authorization", format!("Bearer {key}"))
        .header("x-api-key", &key)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body(serde_json::json!({"query":query,"type":"auto","contents":{"highlights":{"highlightsPerUrl":1}},"numResults":WEB_SEARCH_MAX_RESULTS}).to_string())
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "results").iter().map(|item| {
        let summary = item
            .get("highlights")
            .and_then(Value::as_array)
            .and_then(|rows| {
                rows.iter().find_map(|row| {
                    row.as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
            })
            .unwrap_or("");
        let mut row = source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            summary,
        );
        row.published_at = optional(item.get("publishedDate").unwrap_or(&Value::Null));
        Some(row)
    })))
}

async fn brave(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .get(format!("{base}/res/v1/web/search"))
        .timeout(timeout)
        .query(&[("q", query), ("count", "8")])
        .header("accept", "application/json")
        .header("x-subscription-token", key.unwrap_or(""))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    let rows = value
        .get("web")
        .map(|web| array(web, "results"))
        .unwrap_or(&[]);
    Ok(collect(rows.iter().map(|item| {
        let summary = optional(item.get("description").unwrap_or(&Value::Null)).unwrap_or_default();
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            summary,
        ))
    })))
}

async fn serper(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .post(format!("{base}/search"))
        .timeout(timeout)
        .header("x-api-key", key.unwrap_or(""))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body(serde_json::json!({"q":query,"num":WEB_SEARCH_MAX_RESULTS}).to_string())
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "organic").iter().map(|item| {
        Some(source(
            js(item, "link"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            js(item, "snippet"),
        ))
    })))
}

async fn duckduckgo(
    client: &reqwest::Client,
    base: &str,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .get(format!("{base}/html/"))
        .timeout(timeout)
        .query(&[("q", query)])
        .header("accept", "text/html")
        .header("user-agent", "AIOS/1.0")
        .send()
        .await
        .map_err(map_transport)?;
    let status = response.status();
    if !status.is_success() {
        return Err(SearchError::Http(status.as_u16()));
    }
    let html = response.text().await.map_err(|_| SearchError::Transport)?;
    Ok(parse_ddg(&html))
}

fn parse_ddg(html: &str) -> Vec<SearchSource> {
    let mut rows = Vec::new();
    for chunk in html.split("result__a").skip(1) {
        let Some(href) = chunk
            .split("href=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
        else {
            continue;
        };
        let title = chunk
            .split('>')
            .nth(1)
            .and_then(|rest| rest.split('<').next())
            .unwrap_or("")
            .trim()
            .to_owned();
        let url = decode_ddg(href);
        if url.starts_with("http") {
            rows.push(source(url, Some(title), "DuckDuckGo 搜索结果"));
        }
        if rows.len() >= WEB_SEARCH_MAX_RESULTS {
            break;
        }
    }
    rows
}

fn decode_ddg(href: &str) -> String {
    let raw = href
        .strip_prefix("//duckduckgo.com/l/?uddg=")
        .or_else(|| href.split("uddg=").nth(1))
        .unwrap_or(href);
    let encoded = raw.split('&').next().unwrap_or(raw);
    percent_decode(encoded)
}

fn percent_decode(value: &str) -> String {
    let mut out = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                out.push(hex as char);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

async fn searxng(
    client: &reqwest::Client,
    base: &str,
    basic: Option<(&str, &str)>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let mut request = client
        .get(format!("{base}/search"))
        .timeout(timeout)
        .query(&[("q", query), ("format", "json")])
        .header("accept", "application/json");
    if let Some((user, password)) = basic {
        request = request.basic_auth(user, Some(password));
    }
    let response = request.send().await.map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "results").iter().map(|item| {
        let summary = optional(item.get("content").unwrap_or(&Value::Null)).unwrap_or_default();
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            summary,
        ))
    })))
}

async fn tavily(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .post(format!("{base}/search"))
        .timeout(timeout)
        .bearer_auth(key.unwrap_or(""))
        .json(&serde_json::json!({"query":query,"max_results":WEB_SEARCH_MAX_RESULTS}))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "results").iter().map(|item| {
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            js(item, "content"),
        ))
    })))
}

async fn exa_mcp(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let mut request = client
        .post(base)
        .timeout(timeout)
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"web_search","arguments":{"query":query}}})).send().await.map_err(map_transport)?;
    let value = response_json(response).await?;
    let text = value
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find_map(|row| row.get("text").and_then(Value::as_str))
        })
        .unwrap_or("");
    Ok(parse_mcp_text(text))
}

fn parse_mcp_text(text: &str) -> Vec<SearchSource> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let rows = value
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    collect(rows.iter().map(|item| {
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            js(item, "content"),
        ))
    }))
}

async fn bocha(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .post(format!("{base}/v1/web-search"))
        .timeout(timeout)
        .bearer_auth(key.unwrap_or(""))
        .json(&serde_json::json!({"query":query,"count":WEB_SEARCH_MAX_RESULTS,"summary":true}))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    let rows = value
        .get("data")
        .map(|data| data.get("webPages"))
        .and_then(|pages| pages)
        .map(|pages| array(pages, "value"))
        .unwrap_or(&[]);
    Ok(collect(rows.iter().map(|item| {
        Some(source(
            js(item, "url"),
            optional(item.get("name").unwrap_or(&Value::Null)),
            optional(item.get("summary").unwrap_or(&Value::Null))
                .or_else(|| optional(item.get("snippet").unwrap_or(&Value::Null)))
                .unwrap_or_default(),
        ))
    })))
}

async fn zhipu(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client.post(base).timeout(timeout).bearer_auth(key.unwrap_or("")).json(&serde_json::json!({"search_query":query,"search_engine":"search_std","search_intent":false})).send().await.map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "search_result").iter().map(|item| {
        Some(source(
            js(item, "link"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            js(item, "content"),
        ))
    })))
}

async fn deepseek(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .post(format!("{base}/v1/tools/search"))
        .timeout(timeout)
        .bearer_auth(key.unwrap_or(""))
        .json(&serde_json::json!({"query":query,"count":WEB_SEARCH_MAX_RESULTS}))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "results").iter().map(|item| {
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            optional(item.get("content").unwrap_or(&Value::Null))
                .or_else(|| optional(item.get("summary").unwrap_or(&Value::Null)))
                .or_else(|| optional(item.get("snippet").unwrap_or(&Value::Null)))
                .unwrap_or_default(),
        ))
    })))
}

async fn yuanbao(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .post(format!("{base}/SearchPro"))
        .timeout(timeout)
        .bearer_auth(key.unwrap_or(""))
        .json(&serde_json::json!({"Query":query}))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    let pages = value
        .get("Response")
        .and_then(|response| response.get("Pages"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut rows = Vec::new();
    for page in pages {
        let Some(text) = page.as_str() else { continue };
        let Ok(parsed) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        rows.push(source(
            js(&parsed, "url"),
            optional(parsed.get("title").unwrap_or(&Value::Null)),
            optional(parsed.get("content").unwrap_or(&Value::Null))
                .or_else(|| optional(parsed.get("passage").unwrap_or(&Value::Null)))
                .unwrap_or_default(),
        ));
    }
    Ok(collect(rows.into_iter().map(Some)))
}

async fn stepfun(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .post(format!("{base}/v1/search"))
        .timeout(timeout)
        .bearer_auth(key.unwrap_or(""))
        .json(&serde_json::json!({"query":query,"n":WEB_SEARCH_MAX_RESULTS}))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "results").iter().map(|item| {
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            optional(item.get("content").unwrap_or(&Value::Null))
                .or_else(|| optional(item.get("snippet").unwrap_or(&Value::Null)))
                .unwrap_or_default(),
        ))
    })))
}

async fn jina(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .get(format!("{base}/{}", query.trim()))
        .timeout(timeout)
        .header("accept", "application/json")
        .bearer_auth(key.unwrap_or(""))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    let rows = value
        .get("data")
        .or_else(|| value.get("results"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(collect(rows.iter().map(|item| {
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            optional(item.get("content").unwrap_or(&Value::Null))
                .or_else(|| optional(item.get("description").unwrap_or(&Value::Null)))
                .unwrap_or_default(),
        ))
    })))
}

async fn firecrawl(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client.post(format!("{base}/v2/search")).timeout(timeout).bearer_auth(key.unwrap_or("")).json(&serde_json::json!({"query":query,"limit":WEB_SEARCH_MAX_RESULTS,"scrapeOptions":{"formats":["markdown"]}})).send().await.map_err(map_transport)?;
    let value = response_json(response).await?;
    let rows = value
        .get("data")
        .map(|data| array(data, "web"))
        .unwrap_or(&[]);
    Ok(collect(rows.iter().map(|item| {
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            optional(item.get("markdown").unwrap_or(&Value::Null))
                .or_else(|| optional(item.get("description").unwrap_or(&Value::Null)))
                .unwrap_or_default(),
        ))
    })))
}

async fn parallel(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client.post(format!("{base}/v1/search")).timeout(timeout).header("x-api-key", key.unwrap_or("")).json(&serde_json::json!({"objective":query,"search_queries":[query],"advanced_settings":{"max_results":WEB_SEARCH_MAX_RESULTS}})).send().await.map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "results").iter().map(|item| {
        let summary = item
            .get("excerpts")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default();
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            summary,
        ))
    })))
}

async fn querit(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .post(format!("{base}/v1/search"))
        .timeout(timeout)
        .bearer_auth(key.unwrap_or(""))
        .json(&serde_json::json!({"query":query,"count":WEB_SEARCH_MAX_RESULTS}))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    let rows = value
        .get("results")
        .map(|results| array(results, "result"))
        .unwrap_or(&[]);
    Ok(collect(rows.iter().map(|item| {
        Some(source(
            js(item, "url"),
            optional(item.get("title").unwrap_or(&Value::Null))
                .or_else(|| optional(item.get("site_name").unwrap_or(&Value::Null))),
            js(item, "snippet"),
        ))
    })))
}

async fn searchapi(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .get(format!("{base}/api/v1/search"))
        .timeout(timeout)
        .query(&[("engine", "google"), ("q", query)])
        .bearer_auth(key.unwrap_or(""))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "organic_results").iter().map(
        |item| {
            Some(source(
                js(item, "link"),
                optional(item.get("title").unwrap_or(&Value::Null)),
                optional(item.get("snippet").unwrap_or(&Value::Null)).unwrap_or_default(),
            ))
        },
    )))
}

async fn serpkite(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .get(format!("{base}/v1/search"))
        .timeout(timeout)
        .query(&[("q", query), ("num", "8")])
        .header("accept", "application/json")
        .bearer_auth(key.unwrap_or(""))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    let rows = array(&value, "results");
    Ok(collect(rows.iter().map(|item| {
        Some(source(
            js(item, "link"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            optional(item.get("snippet").unwrap_or(&Value::Null)).unwrap_or_default(),
        ))
    })))
}

async fn serply(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let encoded = percent_encode(query);
    let response = client
        .get(format!("{base}/v1/search/q={encoded}&num=8"))
        .timeout(timeout)
        .header("accept", "application/json")
        .header("x-api-key", key.unwrap_or(""))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "results").iter().map(|item| {
        Some(source(
            js(item, "link"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            optional(item.get("description").unwrap_or(&Value::Null)).unwrap_or_default(),
        ))
    })))
}

async fn desearch(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    query: &str,
    timeout: Duration,
) -> Result<Vec<SearchSource>, SearchError> {
    let response = client
        .get(format!("{base}/web"))
        .timeout(timeout)
        .query(&[("query", query)])
        .header("accept", "application/json")
        .header("authorization", key.unwrap_or(""))
        .send()
        .await
        .map_err(map_transport)?;
    let value = response_json(response).await?;
    Ok(collect(array(&value, "data").iter().map(|item| {
        Some(source(
            js(item, "link"),
            optional(item.get("title").unwrap_or(&Value::Null)),
            optional(item.get("snippet").unwrap_or(&Value::Null)).unwrap_or_default(),
        ))
    })))
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

fn map_transport(error: reqwest::Error) -> SearchError {
    if error.is_timeout() {
        SearchError::Timeout
    } else if error.is_connect() {
        SearchError::Connect
    } else {
        SearchError::Transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_twenty_unique_providers_and_the_five_real_channels() {
        assert_eq!(PROVIDERS.len(), 20);
        let mut ids = std::collections::BTreeSet::new();
        for spec in PROVIDERS {
            assert!(ids.insert(spec.id), "重复服务商：{}", spec.id);
        }
        for id in ["exa", "brave", "serper", "duckduckgo", "searxng"] {
            assert!(provider(id).is_some(), "缺真通道 {id}");
        }
        assert_eq!(provider("duckduckgo").unwrap().key_need, KeyNeed::None);
        assert_eq!(provider("searxng").unwrap().key_need, KeyNeed::None);
        assert_eq!(provider("zhipu").unwrap().key_need, KeyNeed::Llm);
        assert_eq!(provider("deepseek").unwrap().key_need, KeyNeed::Llm);
        assert_eq!(provider("tavily").unwrap().key_need, KeyNeed::Required);
        assert_eq!(provider("jina").unwrap().key_need, KeyNeed::Optional);
    }

    #[tokio::test]
    async fn exa_response_is_parsed_without_a_summary_losing_the_row() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/search"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "results": [{
                        "url": "https://example.com/a",
                        "title": "A",
                        "highlights": ["摘要 A"],
                        "publishedDate": "2026-11-28"
                    }]
                })),
            )
            .mount(&server)
            .await;
        let rows = search_one(
            provider("exa").unwrap(),
            &server.uri(),
            Some("k"),
            None,
            "q",
        )
        .await
        .expect("exa");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://example.com/a");
        assert_eq!(rows[0].summary, "摘要 A");
    }

    #[tokio::test]
    async fn brave_response_is_parsed_from_web_results() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/res/v1/web/search"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "web": {"results": [{"url": "https://example.com/b", "title": "B", "description": "摘要 B"}]}
            })))
            .mount(&server)
            .await;
        let rows = search_one(
            provider("brave").unwrap(),
            &server.uri(),
            Some("k"),
            None,
            "q",
        )
        .await
        .expect("brave");
        assert_eq!(rows[0].summary, "摘要 B");
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct Case {
        id: &'static str,
        method: &'static str,
        path: &'static str,
        success: serde_json::Value,
    }

    fn cases() -> [Case; 5] {
        [
            Case {
                id: "exa",
                method: "POST",
                path: "/search",
                success: serde_json::json!({"results":[{"url":"https://example.com/e","title":"E","highlights":["摘要 E"]}]}),
            },
            Case {
                id: "brave",
                method: "GET",
                path: "/res/v1/web/search",
                success: serde_json::json!({"web":{"results":[{"url":"https://example.com/b","title":"B","description":"摘要 B"}]}}),
            },
            Case {
                id: "serper",
                method: "POST",
                path: "/search",
                success: serde_json::json!({"organic":[{"link":"https://example.com/s","title":"S","snippet":"摘要 S"}]}),
            },
            Case {
                id: "duckduckgo",
                method: "GET",
                path: "/html/",
                success: serde_json::json!(
                    "<a class=\"result__a\" href=\"https://example.com/d\">DDG</a>"
                ),
            },
            Case {
                id: "searxng",
                method: "GET",
                path: "/search",
                success: serde_json::json!({"results":[{"url":"https://example.com/x","title":"X","content":"摘要 X"}]}),
            },
        ]
    }

    async fn server_for(case: &Case, status: u16, body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        let response = if status == 200 {
            if case.id == "duckduckgo" || body == serde_json::json!("not-json") {
                ResponseTemplate::new(200).set_body_string(body.as_str().unwrap_or_default())
            } else {
                ResponseTemplate::new(200).set_body_json(body)
            }
        } else {
            ResponseTemplate::new(status)
        };
        Mock::given(method(case.method))
            .and(path(case.path))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn five_real_channels_succeed_and_map_auth_and_rate_limit() {
        for case in cases() {
            let spec = provider(case.id).unwrap();
            let server = server_for(&case, 200, case.success.clone()).await;
            let rows = search_one_with_timeout(
                spec,
                &server.uri(),
                Some("k"),
                None,
                "q",
                Duration::from_secs(2),
            )
            .await
            .unwrap_or_else(|error| panic!("{} success: {error:?}", case.id));
            assert!(!rows.is_empty(), "{} 要有结果", case.id);

            for status in [401, 429] {
                let server = server_for(&case, status, serde_json::Value::Null).await;
                let error = search_one_with_timeout(
                    spec,
                    &server.uri(),
                    Some("k"),
                    None,
                    "q",
                    Duration::from_secs(2),
                )
                .await
                .expect_err("非 2xx 必须失败");
                assert!(
                    matches!(error, SearchError::Http(got) if got == status),
                    "{} 期望 HTTP {status}，实际 {error:?}",
                    case.id
                );
            }

            // 坏 JSON：JSON provider 报 Body；DuckDuckGo 是 HTML，空页按空结果。
            let server = server_for(&case, 200, serde_json::json!("not-json")).await;
            let result = search_one_with_timeout(
                spec,
                &server.uri(),
                Some("k"),
                None,
                "q",
                Duration::from_secs(2),
            )
            .await;
            if case.id == "duckduckgo" {
                assert!(result.expect("DDG 空/坏 HTML 按 0 条").is_empty());
            } else {
                assert!(
                    matches!(result, Err(SearchError::Body)),
                    "{} 坏 JSON 要报 Body",
                    case.id
                );
            }
        }
    }

    #[tokio::test]
    async fn five_real_channels_map_timeout() {
        for case in cases() {
            let server = MockServer::start().await;
            Mock::given(method(case.method))
                .and(path(case.path))
                .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(200)))
                .mount(&server)
                .await;
            let error = search_one_with_timeout(
                provider(case.id).unwrap(),
                &server.uri(),
                Some("k"),
                None,
                "q",
                Duration::from_millis(30),
            )
            .await
            .expect_err("超时");
            assert!(
                matches!(error, SearchError::Timeout),
                "{} 超时要映射 Timeout",
                case.id
            );
        }
    }
}

#[tokio::test]
#[ignore = "live smoke：需要真实网络/用户 key，手工跑"]
async fn live_smoke_five_real_channels() {
    let key = |name: &str| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    let probes: [(&str, Option<String>); 5] = [
        ("exa", key("EXA_API_KEY")),
        ("brave", key("BRAVE_API_KEY")),
        ("serper", key("SERPER_API_KEY")),
        ("duckduckgo", None),
        ("searxng", None),
    ];
    let mut failures = Vec::new();
    for (id, key) in probes {
        let spec = provider(id).expect("provider");
        let host = if id == "searxng" {
            std::env::var("AIOS_SMOKE_SEARXNG_HOST")
                .unwrap_or_else(|_| "https://searx.be".to_owned())
        } else {
            spec.host.to_owned()
        };
        match search_one(spec, &host, key.as_deref(), None, "AIOS").await {
            Ok(rows) => println!("SMOKE {id}: ok {} results", rows.len()),
            Err(SearchError::Http(401 | 403)) if key.is_none() => {
                println!("SMOKE {id}: 未验证（缺 key）")
            }
            Err(error) => {
                println!("SMOKE {id}: 未验证（请求失败 {error:?}）");
                failures.push(id);
            }
        }
    }
    assert!(
        !failures.contains(&"duckduckgo"),
        "DuckDuckGo 不许 key，必须真跑通；失败：{failures:?}"
    );
}

#[cfg(test)]
mod recorded_contract_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct Case {
        id: &'static str,
        method: &'static str,
        path: &'static str,
        success: serde_json::Value,
    }

    async fn server_for(case: &Case, status: u16, body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        let response = if status == 200 {
            if body == serde_json::json!("not-json") {
                ResponseTemplate::new(200).set_body_string(body.as_str().unwrap_or_default())
            } else {
                ResponseTemplate::new(200).set_body_json(body)
            }
        } else {
            ResponseTemplate::new(status)
        };
        Mock::given(method(case.method))
            .and(path(case.path))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    fn recorded_cases() -> [Case; 15] {
        [
            Case { id: "tavily", method: "POST", path: "/search", success: serde_json::json!({"results":[{"url":"https://example.com/t","title":"T","content":"摘要 T"}]}) },
            Case { id: "exa-mcp", method: "POST", path: "/", success: serde_json::json!({"result":{"content":[{"text":"{\"results\":[{\"url\":\"https://example.com/m\",\"title\":\"M\",\"content\":\"摘要 M\"}]}"}]}}) },
            Case { id: "bocha", method: "POST", path: "/v1/web-search", success: serde_json::json!({"code":0,"data":{"webPages":{"value":[{"url":"https://example.com/bc","name":"BC","summary":"摘要 BC"}]}}}) },
            Case { id: "zhipu", method: "POST", path: "/", success: serde_json::json!({"search_result":[{"link":"https://example.com/z","title":"Z","content":"摘要 Z"}]}) },
            Case { id: "deepseek", method: "POST", path: "/v1/tools/search", success: serde_json::json!({"results":[{"url":"https://example.com/ds","title":"DS","content":"摘要 DS"}]}) },
            Case { id: "yuanbao", method: "POST", path: "/SearchPro", success: serde_json::json!({"Response":{"Pages":["{\"url\":\"https://example.com/y\",\"title\":\"Y\",\"content\":\"摘要 Y\"}"]}}) },
            Case { id: "stepfun", method: "POST", path: "/v1/search", success: serde_json::json!({"results":[{"url":"https://example.com/st","title":"ST","content":"摘要 ST"}]}) },
            Case { id: "jina", method: "GET", path: "/q", success: serde_json::json!({"data":[{"url":"https://example.com/j","title":"J","content":"摘要 J"}]}) },
            Case { id: "firecrawl", method: "POST", path: "/v2/search", success: serde_json::json!({"data":{"web":[{"url":"https://example.com/f","title":"F","markdown":"摘要 F"}]}}) },
            Case { id: "parallel", method: "POST", path: "/v1/search", success: serde_json::json!({"results":[{"url":"https://example.com/p","title":"P","excerpts":["摘要 P"]}]}) },
            Case { id: "querit", method: "POST", path: "/v1/search", success: serde_json::json!({"results":{"result":[{"url":"https://example.com/q","title":"Q","snippet":"摘要 Q"}]}}) },
            Case { id: "searchapi", method: "GET", path: "/api/v1/search", success: serde_json::json!({"organic_results":[{"link":"https://example.com/sa","title":"SA","snippet":"摘要 SA"}]}) },
            Case { id: "serpkite", method: "GET", path: "/v1/search", success: serde_json::json!({"results":[{"link":"https://example.com/sk","title":"SK","snippet":"摘要 SK"}]}) },
            Case { id: "serply", method: "GET", path: "/v1/search/q=q&num=8", success: serde_json::json!({"results":[{"link":"https://example.com/sp","title":"SP","description":"摘要 SP"}]}) },
            Case { id: "desearch", method: "GET", path: "/web", success: serde_json::json!({"data":[{"link":"https://example.com/de","title":"DE","snippet":"摘要 DE"}]}) },
        ]
    }

    #[tokio::test]
    async fn fifteen_recorded_providers_parse_success_auth_rate_limit_bad_json_and_timeout() {
        for case in recorded_cases() {
            let spec = provider(case.id).unwrap();
            let server = server_for(&case, 200, case.success.clone()).await;
            let rows = search_one_with_timeout(
                spec,
                &server.uri(),
                Some("k"),
                None,
                "q",
                Duration::from_secs(2),
            )
            .await
            .unwrap_or_else(|error| panic!("{} success: {error:?}", case.id));
            assert!(!rows.is_empty(), "{} 要有结果", case.id);

            for status in [401, 429] {
                let server = server_for(&case, status, serde_json::Value::Null).await;
                let error = search_one_with_timeout(
                    spec,
                    &server.uri(),
                    Some("k"),
                    None,
                    "q",
                    Duration::from_secs(2),
                )
                .await
                .expect_err("非 2xx 必须失败");
                assert!(
                    matches!(error, SearchError::Http(got) if got == status),
                    "{} 期望 HTTP {status}，实际 {error:?}",
                    case.id
                );
            }

            let server = server_for(&case, 200, serde_json::json!("not-json")).await;
            assert!(
                matches!(
                    search_one_with_timeout(
                        spec,
                        &server.uri(),
                        Some("k"),
                        None,
                        "q",
                        Duration::from_secs(2)
                    )
                    .await,
                    Err(SearchError::Body)
                ),
                "{} 坏 JSON 要报 Body",
                case.id
            );

            let server = MockServer::start().await;
            Mock::given(method(case.method))
                .and(path(case.path))
                .respond_with(
                    ResponseTemplate::new(200).set_delay(Duration::from_millis(200)),
                )
                .mount(&server)
                .await;
            let error = search_one_with_timeout(
                spec,
                &server.uri(),
                Some("k"),
                None,
                "q",
                Duration::from_millis(30),
            )
            .await
            .expect_err("超时");
            assert!(matches!(error, SearchError::Timeout), "{} 超时要映射 Timeout", case.id);
        }
    }
}
