//! Exa 的搜索请求与响应：一条查询一次请求，响应里没有摘要的条目丢掉。

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tools::ToolFailure;

/// Exa 请求自己的超时；和工具上限同一个数，网络层先报。
const EXA_TIMEOUT: Duration = Duration::from_secs(30);

/// 一次搜索合并后最多给的来源数。
pub(super) const WEB_SEARCH_MAX_RESULTS: usize = 8;

/// 一条查询打一次 Exa；不跟重定向，响应里没有摘要的条目在映射时丢掉。
pub(super) async fn search_one(
    base_url: &str,
    key: &str,
    query: &str,
) -> Result<Vec<SearchSource>, SearchError> {
    let client = model::http::client_without_redirects().map_err(|_| SearchError::Client)?;
    let body = json!({
        "query": query,
        "type": "auto",
        "contents": {"highlights": {"highlightsPerUrl": 1}},
        "numResults": WEB_SEARCH_MAX_RESULTS,
    })
    .to_string();
    let response = client
        .post(format!("{}/search", base_url.trim_end_matches('/')))
        .timeout(EXA_TIMEOUT)
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                SearchError::Timeout
            } else if error.is_connect() {
                SearchError::Connect
            } else {
                SearchError::Transport
            }
        })?;
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
    let payload: ExaResponse = serde_json::from_slice(&bytes).map_err(|_| SearchError::Body)?;
    Ok(payload
        .results
        .into_iter()
        .filter_map(map_exa_result)
        .collect())
}

/// 一次搜索失败的原因；文案里不带请求头或 key。
#[derive(Debug, Clone, Copy)]
pub(super) enum SearchError {
    Client,
    Connect,
    Timeout,
    Transport,
    Http(u16),
    Body,
    Task,
}

impl SearchError {
    pub(super) fn into_failure(self) -> ToolFailure {
        match self {
            Self::Client => ToolFailure::plain("建不出连 Exa 的 HTTP 客户端；稍后再试"),
            Self::Connect => ToolFailure::plain("连不上 Exa；检查网络后再试"),
            Self::Timeout => ToolFailure::plain("等 Exa 回应超时了（30 秒）；稍后再试"),
            Self::Transport => ToolFailure::plain("向 Exa 发请求时出错了；稍后再试"),
            Self::Http(status) if status == 401 || status == 403 => ToolFailure::plain(format!(
                "Exa 拒绝了这把 key（{status}）；去设置页 › 模型与连接 › 联网搜索 换一把"
            )),
            Self::Http(status) => ToolFailure::plain(format!("Exa 那边出错了（{status}）")),
            Self::Body => ToolFailure::plain("Exa 返回的内容看不懂（不是预期的 JSON）"),
            Self::Task => ToolFailure::plain("搜网页的任务没有正常结束"),
        }
    }
}

/// Exa 的响应外壳；没有 results 就当空结果。
#[derive(Debug, Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

/// Exa 的一条结果；摘要取第一条非空 highlight。
#[derive(Debug, Deserialize)]
struct ExaResult {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    highlights: Option<Vec<String>>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchSource {
    pub(super) url: String,
    pub(super) title: Option<String>,
    pub(super) summary: String,
    pub(super) published_at: Option<String>,
}

/// Exa 结果 → 来源：没有摘要（或没有 URL）的条目丢掉，不编一个摘要。
fn map_exa_result(result: ExaResult) -> Option<SearchSource> {
    let summary = result
        .highlights?
        .into_iter()
        .map(|highlight| highlight.trim().to_owned())
        .find(|highlight| !highlight.is_empty())?;
    if result.url.trim().is_empty() {
        return None;
    }
    Some(SearchSource {
        url: result.url,
        title: result.title.filter(|title| !title.trim().is_empty()),
        summary,
        published_at: result.published_date.filter(|date| !date.trim().is_empty()),
    })
}
