//! `web_search` 的模型面：参数、执行、开出条件，以及搜索结果的合并排版。

use std::path::PathBuf;

use serde_json::{Value, json};
use tokio::task::JoinSet;
use tools::{
    Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolKey, ToolResult,
};

use super::providers::{self, SearchError, SearchSource, WEB_SEARCH_MAX_RESULTS};
use super::{EXTERNAL_WEB_CONTENT_NOTICE, WEB_TOOL_TIMEOUT_MS};

/// 一次搜索最多接受的查询数。
const WEB_SEARCH_MAX_QUERIES: usize = 4;
/// 一条来源摘要最多给的字符数（含截断的省略号）。
const WEB_SEARCH_SUMMARY_MAX_CHARS: usize = 800;

/// 照抄 DSH 的引用说明。
const CITE_INSTRUCTION: &str = "Cite the relevant URLs above as markdown links in your answer.";

/// `web_search` 的装配：Keyring 位置和 Exa 根地址；服务启动/重开根会话时注入。
#[derive(Clone)]
pub(crate) struct WebSearchConfig {
    pub(crate) data_dir: PathBuf,
    pub(crate) identity: Option<PathBuf>,
}

/// 联网搜索工具。
pub(crate) struct WebSearchTool {
    parameters: Value,
    config: WebSearchConfig,
}

impl WebSearchTool {
    pub(crate) fn new(config: WebSearchConfig) -> Self {
        Self {
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "maxItems": 4,
                        "description": "1-4 search queries; their results are merged."
                    },
                    "providers": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Provider ids to use. Omit to let priority/schedule pick."
                    },
                    "sites": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Domain wildcards like *.github.com (not regular expressions)."
                    },
                    "include": {
                        "type": "boolean",
                        "description": "true = only these sites; false = exclude them. Default true."
                    },
                    "topic": {"type": "string", "enum": ["web", "news"], "description": "Default web."},
                    "category": {
                        "type": "string",
                        "enum": ["general", "code", "company", "docs", "research"],
                        "description": "Default general."
                    }
                },
                "required": ["queries"]
            }),
            config,
        }
    }

    async fn search(&self, request: &SearchRequest) -> Result<String, ToolFailure> {
        let mut rows =
            crate::web_search::enabled_for(&self.config.data_dir, self.config.identity.as_deref())
                .map_err(|error| ToolFailure::plain(error.to_string()))?;
        if rows.is_empty() {
            return Err(ToolFailure::plain(
                "没有开着的联网搜索服务商；去设置页 › 模型与连接 › 联网搜索 打开一家",
            ));
        }
        let mut skipped = Vec::new();
        let mut chosen = Vec::new();
        if request.providers.is_empty() {
            let row = rows
                .drain(..)
                .find(|row| row.missing.is_none() && supports_topic(row, &request.topic))
                .ok_or_else(|| {
                    ToolFailure::plain(
                        "开着的服务商都还不能用；去设置 › 模型与连接 › 联网搜索 补 key/地址",
                    )
                })?;
            chosen.push(row);
        } else {
            for id in &request.providers {
                let Some(row) = rows.iter().find(|row| row.spec.id == id).cloned() else {
                    skipped.push(format!("{id}: 没开"));
                    continue;
                };
                if let Some(missing) = &row.missing {
                    skipped.push(format!("{}: {missing}", row.spec.name));
                    continue;
                }
                if !supports_topic(&row, &request.topic) {
                    skipped.push(format!("{}: 不支持 {}", row.spec.name, request.topic));
                    continue;
                }
                chosen.push(row);
            }
            if chosen.is_empty() {
                return Err(ToolFailure::plain(format!(
                    "点名的服务商都不能用：{}",
                    skipped.join("；")
                )));
            }
        }

        let per_provider = crate::settings::load(&self.config.data_dir)
            .web_search
            .schedule
            .per_provider as usize;
        let mut pending = JoinSet::new();
        for (provider_index, row) in chosen.iter().enumerate() {
            for (query_index, query) in request.queries.iter().enumerate() {
                let row = row.clone();
                let query = query.clone();
                let filters = providers::SearchFilters {
                    sites: request.sites.clone(),
                    include: request.include,
                    topic: request.topic.clone(),
                    category: request.category.clone(),
                };
                pending.spawn(async move {
                    let basic = row
                        .basic
                        .as_ref()
                        .map(|(user, password)| (user.as_str(), password.as_str()));
                    let result = providers::search_one_filtered(
                        row.spec,
                        &row.host,
                        row.key.as_deref(),
                        basic,
                        &query,
                        &filters,
                        std::time::Duration::from_secs(30),
                    )
                    .await;
                    (provider_index, query_index, row.spec.id, result)
                });
            }
        }
        let mut hits: std::collections::BTreeMap<String, Hit> = std::collections::BTreeMap::new();
        let mut failures = Vec::new();
        let mut succeeded_providers = std::collections::BTreeSet::new();
        while let Some(joined) = pending.join_next().await {
            match joined {
                Ok((provider_index, _, provider_id, Ok(sources))) => {
                    succeeded_providers.insert(provider_id.to_owned());
                    for source in sources.into_iter().take(per_provider) {
                        if !site_allowed(&request.sites, request.include, &source.url) {
                            continue;
                        }
                        let entry = hits.entry(source.url.clone()).or_insert_with(|| Hit {
                            source: source.clone(),
                            providers: Default::default(),
                            rank: provider_index,
                        });
                        entry.providers.insert(provider_id.to_owned());
                    }
                }
                Ok((_, _, provider_id, Err(error))) => {
                    crate::web::search::record_failure(&self.config.data_dir, provider_id, &error);
                    failures.push(format!("{}: {:?}", provider_id, error));
                }
                Err(join_error) => {
                    if !join_error.is_cancelled() {
                        failures.push("搜索任务没有正常结束".to_owned());
                    }
                }
            }
        }
        if succeeded_providers.is_empty() {
            return Err(ToolFailure::plain(format!(
                "联网搜索失败：{}",
                failures.join("；")
            )));
        }
        let mut merged: Vec<Hit> = hits.into_values().collect();
        merged.sort_by(|left, right| {
            right
                .providers
                .len()
                .cmp(&left.providers.len())
                .then(left.rank.cmp(&right.rank))
                .then(left.source.url.cmp(&right.source.url))
        });
        let truncated = merged.len() > 20;
        merged.truncate(20);
        let duplicate_count = duplicate_count(&merged);
        Ok(format_search_output(
            &merged,
            truncated,
            &skipped,
            &failures,
            duplicate_count,
        ))
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
            key: ToolKey::WebSearch,
        }
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let request = match parse_request(&call.arguments) {
                Ok(request) => request,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            match self.search(&request).await {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

#[derive(Debug, Clone)]
struct SearchRequest {
    queries: Vec<String>,
    providers: Vec<String>,
    sites: Vec<String>,
    include: bool,
    topic: String,
    category: String,
}

fn parse_request(arguments: &str) -> Result<SearchRequest, ToolFailure> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| ToolFailure::plain("arguments must be a JSON object"))?;
    let queries = object
        .get("queries")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolFailure::plain("queries must be an array of strings"))?;
    if queries.is_empty() || queries.len() > WEB_SEARCH_MAX_QUERIES {
        return Err(ToolFailure::plain("queries must contain 1-4 queries"));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut parsed = Vec::new();
    for query in queries {
        let query = query
            .as_str()
            .ok_or_else(|| ToolFailure::plain("each query must be a string"))?;
        if query.trim().is_empty() {
            return Err(ToolFailure::plain("each query must be a non-empty string"));
        }
        if seen.insert(query) {
            parsed.push(query.to_owned());
        }
    }
    let strings = |name: &str| -> Result<Vec<String>, ToolFailure> {
        let Some(items) = object.get(name) else {
            return Ok(Vec::new());
        };
        items
            .as_array()
            .ok_or_else(|| ToolFailure::plain(format!("{name} must be an array")))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| ToolFailure::plain(format!("{name} items must be strings")))
            })
            .collect()
    };
    let topic = object
        .get("topic")
        .and_then(Value::as_str)
        .unwrap_or("web")
        .to_owned();
    if !matches!(topic.as_str(), "web" | "news") {
        return Err(ToolFailure::plain("topic must be web or news"));
    }
    let category = object
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or("general")
        .to_owned();
    if !matches!(
        category.as_str(),
        "general" | "code" | "company" | "docs" | "research"
    ) {
        return Err(ToolFailure::plain("category is not supported"));
    }
    Ok(SearchRequest {
        queries: parsed,
        providers: strings("providers")?,
        sites: strings("sites")?,
        include: object
            .get("include")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        topic,
        category,
    })
}

fn supports_topic(row: &crate::web_search::SelectedSearch, topic: &str) -> bool {
    topic == "web" || providers::supports(row.spec.id, "新闻")
}

fn site_allowed(sites: &[String], include: bool, url: &str) -> bool {
    if sites.is_empty() {
        return true;
    }
    let matched = sites.iter().any(|site| providers::site_matches(site, url));
    matched == include
}

#[derive(Debug, Clone)]
struct Hit {
    source: SearchSource,
    providers: std::collections::BTreeSet<String>,
    rank: usize,
}

fn duplicate_count(hits: &[Hit]) -> usize {
    let mut seen = std::collections::BTreeSet::new();
    hits.iter()
        .filter(|hit| !seen.insert(hit.source.url.clone()))
        .count()
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

fn format_source_line(source: &SearchSource) -> String {
    let title = source.title.as_deref().unwrap_or("").trim();
    let mut summary = source.summary.trim().to_owned();
    if summary.chars().count() > WEB_SEARCH_SUMMARY_MAX_CHARS {
        summary = summary
            .chars()
            .take(WEB_SEARCH_SUMMARY_MAX_CHARS - 1)
            .collect::<String>()
            + "…";
    }
    let published = source
        .published_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    if title.is_empty() {
        format!("- {}\n  {summary}{published}", source.url)
    } else {
        format!("- [{title}]({}){published}\n  {summary}", source.url)
    }
}

/// 模型看到的文本：不可信提示、来源、选择/去重说明、引用说明。
fn format_search_output(
    hits: &[Hit],
    truncated: bool,
    skipped: &[String],
    failures: &[String],
    duplicate_count: usize,
) -> String {
    let mut parts = vec![EXTERNAL_WEB_CONTENT_NOTICE.to_owned()];
    if hits.is_empty() {
        parts.push("No results found.".to_owned());
    } else {
        let lines = hits
            .iter()
            .map(|hit| {
                let mut line = format_source_line(&hit.source);
                if hit.providers.len() > 1 {
                    line.push_str(&format!(
                        "
  also found by: {}",
                        hit.providers.iter().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
                line
            })
            .collect::<Vec<_>>();
        parts.push(format!(
            "Sources:
{}",
            lines.join(
                "
"
            )
        ));
        parts.push(format!(
            "Search decisions: {} duplicate result(s) removed; {} provider(s) contributed.",
            duplicate_count,
            hits.iter().map(|hit| hit.providers.len()).sum::<usize>()
        ));
    }
    if !skipped.is_empty() {
        parts.push(format!("Skipped providers: {}", skipped.join("; ")));
    }
    if !failures.is_empty() {
        parts.push(format!("Failed providers: {}", failures.join("; ")));
    }
    if truncated {
        parts.push("(Showing the first 20 sources. Refine the query for more.)".to_owned());
    }
    parts.push(CITE_INSTRUCTION.to_owned());
    parts.join(
        "

",
    )
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

/// 搜之前的来源状态：从设置里读持久化的最近失败/冷却。
pub(crate) fn provider_status(
    data_dir: &std::path::Path,
    id: &str,
    missing: Option<&str>,
) -> String {
    if let Some(missing) = missing {
        let key = if missing.contains("key") {
            "missing_key"
        } else {
            "missing_host"
        };
        return format!("{key} ({missing})");
    }
    let health = crate::settings::load(data_dir)
        .web_search
        .health
        .get(id)
        .cloned()
        .unwrap_or_default();
    if health.cooldown_until > now_seconds() {
        return format!("cooldown_until={}", health.cooldown_until);
    }
    if let Some(last) = health.last_failure {
        return format!("last_failure={last}");
    }
    "ready".to_owned()
}

pub(crate) fn record_failure(data_dir: &std::path::Path, id: &str, error: &SearchError) {
    let mut settings = match crate::settings::load_for_write(data_dir) {
        Ok(settings) => settings,
        Err(_) => return,
    };
    let entry = settings.web_search.health.entry(id.to_owned()).or_default();
    let reason = format!("{error:?}");
    if !matches!(error, SearchError::Http(401)) {
        entry.failures += 1;
        if entry.failures >= 3 {
            entry.cooldown_until = now_seconds() + 600;
            entry.failures = 0;
        }
    }
    entry.last_failure = Some(reason);
    let _ = crate::settings::save(data_dir, &settings);
}

#[cfg(test)]
pub(crate) fn reset_health(data_dir: &std::path::Path) {
    if let Ok(mut settings) = crate::settings::load_for_write(data_dir) {
        settings.web_search.health.clear();
        let _ = crate::settings::save(data_dir, &settings);
    }
}
