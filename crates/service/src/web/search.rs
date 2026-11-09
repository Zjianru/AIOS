//! `web_search` 的模型面：参数、执行、开出条件，以及 Exa 结果的合并排版。

use std::path::PathBuf;

use serde_json::{Value, json};
use tokio::task::JoinSet;
use tools::{
    Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolKey, ToolResult,
};

use super::exa::{SearchError, SearchSource, WEB_SEARCH_MAX_RESULTS, search_one};
use super::{EXTERNAL_WEB_CONTENT_NOTICE, WEB_TOOL_TIMEOUT_MS};

/// 一次搜索最多接受的查询数。
const WEB_SEARCH_MAX_QUERIES: usize = 4;
/// 一条来源摘要最多给的字符数（含截断的省略号）。
const WEB_SEARCH_SUMMARY_MAX_CHARS: usize = 800;

/// 签发记录里的用途；设置页的「签发记录」表读它。
const EXA_SEARCH_PURPOSE: &str = "网页搜索";

/// 照抄 DSH 的引用说明。
const CITE_INSTRUCTION: &str = "Cite the relevant URLs above as markdown links in your answer.";

fn no_key() -> String {
    format!(
        "web_search 需要 Exa 的 key：去设置页填（{}），再试一次",
        super::EXA_SETTINGS_PATH
    )
}

fn key_unreadable() -> String {
    format!(
        "Exa 的 key 读不出来；去设置页看一眼（{}）",
        super::EXA_SETTINGS_PATH
    )
}

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

    /// 跑一批查询：任一条失败整批失败；成功的结果按查询顺序轮流合并。
    async fn search(&self, queries: &[String]) -> Result<String, ToolFailure> {
        let key = issue_search_key(&self.config)?;
        let mut pending = JoinSet::new();
        for (index, query) in queries.iter().enumerate() {
            let base_url = self.config.base_url.clone();
            let key = key.clone();
            let query = query.clone();
            pending.spawn(async move { (index, search_one(&base_url, &key, &query).await) });
        }
        let mut per_query: Vec<Option<Vec<SearchSource>>> = vec![None; queries.len()];
        let mut first_failure = None;
        while let Some(joined) = pending.join_next().await {
            match joined {
                Ok((index, Ok(sources))) => per_query[index] = Some(sources),
                Ok((_, Err(error))) => {
                    first_failure.get_or_insert(error);
                    // 一条失败就把同伴也停掉，别让整批等最慢的那条。
                    pending.abort_all();
                }
                Err(join_error) => {
                    if !join_error.is_cancelled() {
                        first_failure.get_or_insert(SearchError::Task);
                    }
                }
            }
        }
        if let Some(error) = first_failure {
            return Err(error.into_failure());
        }
        let per_query = per_query
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect();
        let (sources, truncated) = merge_sources(per_query);
        Ok(format_search_output(&sources, truncated))
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
            let queries = match parse_queries(&call.arguments) {
                Ok(queries) => queries,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            match self.search(&queries).await {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// 解析模型给的参数：1–4 条非空字符串，按第一次出现的顺序去重。
fn parse_queries(arguments: &str) -> Result<Vec<String>, ToolFailure> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| ToolFailure::plain("arguments must be a JSON object"))?;
    let queries = object
        .get("queries")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolFailure::plain("queries must be an array of strings"))?;
    if queries.is_empty() {
        return Err(ToolFailure::plain(
            "queries must contain at least one query",
        ));
    }
    if queries.len() > WEB_SEARCH_MAX_QUERIES {
        return Err(ToolFailure::plain(format!(
            "queries must contain at most {WEB_SEARCH_MAX_QUERIES} queries"
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut parsed = Vec::with_capacity(queries.len());
    for query in queries {
        let query = query
            .as_str()
            .ok_or_else(|| ToolFailure::plain("each query must be a non-empty string"))?;
        if query.trim().is_empty() {
            return Err(ToolFailure::plain("each query must be a non-empty string"));
        }
        if seen.insert(query) {
            parsed.push(query.to_owned());
        }
    }
    Ok(parsed)
}

/// 从 Keyring 现取一次 Exa 的 key；没存、读不出来都明确报，不静默跳过。
fn issue_search_key(config: &WebSearchConfig) -> Result<String, ToolFailure> {
    let keyring = credentials::Keyring::open(config.data_dir.clone(), config.identity.as_deref())
        .map_err(|_| ToolFailure::plain(key_unreadable()))?;
    let view = keyring
        .exa_key_view()
        .map_err(|_| ToolFailure::plain(key_unreadable()))?;
    if !view.key.saved {
        return Err(ToolFailure::plain(no_key()));
    }
    keyring
        .issue_exa_key(EXA_SEARCH_PURPOSE)
        .map(credentials::IssuedSecret::into_secret)
        .map_err(|_| ToolFailure::plain(key_unreadable()))
}
/// 每条查询的结果按名次轮流取，URL 去重，总量不超过 8 条；返回是否还有被截掉的。
fn merge_sources(per_query: Vec<Vec<SearchSource>>) -> (Vec<SearchSource>, bool) {
    let mut seen = std::collections::BTreeSet::new();
    let mut merged = Vec::new();
    let mut dropped = false;
    let deepest = per_query.iter().map(Vec::len).max().unwrap_or(0);
    'merge: for rank in 0..deepest {
        for sources in &per_query {
            if let Some(source) = sources.get(rank)
                && seen.insert(source.url.clone())
            {
                if merged.len() == WEB_SEARCH_MAX_RESULTS {
                    dropped = true;
                    break 'merge;
                }
                merged.push(source.clone());
            }
        }
    }
    (merged, dropped)
}

/// 模型看到的文本：不可信提示、Sources、截断说明、引用说明（照 DSH 的顺序）。
fn format_search_output(sources: &[SearchSource], truncated: bool) -> String {
    let mut parts = vec![EXTERNAL_WEB_CONTENT_NOTICE.to_owned()];
    if sources.is_empty() {
        parts.push("No results found.".to_owned());
    } else {
        let lines = sources.iter().map(format_source_line).collect::<Vec<_>>();
        let block = lines.join(
            "
",
        );
        parts.push(format!(
            "Sources:
{block}"
        ));
    }
    if truncated {
        let count = sources.len();
        parts.push(format!(
            "(Showing the first {count} sources. Refine the query for more.)"
        ));
    }
    parts.push(CITE_INSTRUCTION.to_owned());
    parts.join(
        "

",
    )
}

/// 一行来源：标题（没有就用 URL）、摘要、日期。
fn format_source_line(source: &SearchSource) -> String {
    let url = source.url.as_str();
    let label = source.title.as_deref().unwrap_or(url);
    let summary = bounded_summary(&source.summary);
    let mut meta = Vec::new();
    if !summary.is_empty() {
        meta.push(summary);
    }
    if let Some(date) = source.published_at.as_deref() {
        meta.push(format!("({date})"));
    }
    let suffix = if meta.is_empty() {
        String::new()
    } else {
        format!(" — {}", meta.join(" "))
    };
    format!("- [{label}]({url}){suffix}")
}

/// 空白折成一个空格；超过 800 字截到 799 字再加省略号。
fn bounded_summary(summary: &str) -> String {
    let collapsed = collapse_whitespace(summary);
    if collapsed.chars().count() <= WEB_SEARCH_SUMMARY_MAX_CHARS {
        return collapsed;
    }
    let mut bounded: String = collapsed
        .chars()
        .take(WEB_SEARCH_SUMMARY_MAX_CHARS - 1)
        .collect();
    bounded.push('…');
    bounded
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
