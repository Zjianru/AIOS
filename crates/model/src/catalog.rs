//! T012.3/T012.5：models.dev 目录（推理档规则、拉取、24h 缓存、内置快照、获取可用模型合并）。
//!
//! 规则照 Magpie catalog.go：toggle 加 none 只给 DeepSeek；同 family 借档；budget-only 无档。
//! T012.5 增加：models.dev 拉取/缓存/快照，目录字段（窗口/输出/价格/thinks/efforts），
//! 以及和 GET /v1/models 的结果合并（厂商列表字段优先，用户填的不动由 service 覆盖）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::ToolUpdate;

/// models.dev 的快照地址（T012.5 启动拉取）。
pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";
/// 缓存多久算过期（照 Magpie fresh.go）。
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// 内置快照：只带 DeepSeek / MiniMax / MiniMax 国内站三家。
pub fn snapshot_json() -> &'static str {
    include_str!("catalog/snapshot.json")
}

pub fn deserialize_snapshot(raw: &str) -> CatalogSnapshot {
    parse_snapshot(raw)
}

fn parse_snapshot(raw: &str) -> CatalogSnapshot {
    serde_json::from_str(raw).expect("内置 models.dev 快照必须是合法 JSON")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    pub providers: BTreeMap<String, CatalogProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProvider {
    pub id: String,
    #[serde(default)]
    pub models: BTreeMap<String, CatalogModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub reasoning_options: Vec<ReasoningOption>,
    #[serde(default)]
    pub limit: Limit,
    #[serde(default)]
    pub cost: Option<Cost>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Limit {
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    /// 缓存读单价（美元/百万 token）；目录没给就是 None（T012 F1：有缓存用量却无价 → 这一轮无价格）。
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// 缓存写单价；多数目录没有这一项。
    #[serde(default)]
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningOption {
    Toggle,
    Effort {
        values: Vec<String>,
    },
    #[serde(rename = "budget_tokens")]
    BudgetTokens {
        #[serde(default)]
        min: Option<u32>,
        #[serde(default)]
        max: Option<u32>,
    },
}

fn reasoning_off_provider(provider_id: &str) -> bool {
    provider_id == "deepseek"
}

fn budgeted(model: &CatalogModel) -> bool {
    model
        .reasoning_options
        .iter()
        .any(|option| matches!(option, ReasoningOption::BudgetTokens { .. }))
}

/// 模型能选的档：effort 选项的 values；DeepSeek 的 toggle 在最前面加 none；
/// 只有 toggle 的模型返回空（Magpie mdModel.efforts）。
pub fn efforts(model: &CatalogModel, provider_id: &str) -> Vec<String> {
    let mut toggle = false;
    let mut values = Vec::new();
    for option in &model.reasoning_options {
        match option {
            ReasoningOption::Toggle => toggle = true,
            ReasoningOption::Effort { values: listed } => values = listed.clone(),
            ReasoningOption::BudgetTokens { .. } => {}
        }
    }
    if reasoning_off_provider(provider_id)
        && toggle
        && !values.is_empty()
        && !values.contains(&"none".to_owned())
    {
        values.insert(0, "none".to_owned());
    }
    values
}

/// 模型实际的档：自己的 efforts；会思考但没列档的，借同 family、发布日期不晚于它、
/// 最新一个会思考模型的档；只有预算的模型不借（Magpie mdProvider.levels）。
pub fn levels(provider: &CatalogProvider, model: &CatalogModel) -> Vec<String> {
    let own = efforts(model, &provider.id);
    if !own.is_empty() || !model.reasoning || model.family.is_empty() || budgeted(model) {
        return own;
    }
    let mut best: Option<(&str, &str, Vec<String>)> = None;
    for sibling in provider.models.values() {
        if sibling.id == model.id || !sibling.reasoning || sibling.family != model.family {
            continue;
        }
        if !model.release_date.is_empty() && sibling.release_date > model.release_date {
            continue;
        }
        let sibling_efforts = efforts(sibling, &provider.id);
        if sibling_efforts.is_empty() {
            continue;
        }
        let take = match &best {
            None => true,
            Some((best_date, best_id, _)) => {
                sibling.release_date.as_str() > *best_date
                    || (sibling.release_date == *best_date && sibling.id.as_str() > *best_id)
            }
        };
        if take {
            best = Some((&sibling.release_date, &sibling.id, sibling_efforts));
        }
    }
    best.map(|(_, _, efforts)| efforts).unwrap_or_default()
}

/// 目录给一个模型的可展示字段（T012.5）。
///
/// 线上字段用 camelCase：页面（设置页的获取面板、选择器）直接读，四类价格不另起名字。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_output: Option<f64>,
    /// T012 F1：缓存读/写单价（stats 里按用户价或目录标价取）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_cache_write: Option<f64>,
    pub thinks: bool,
    pub efforts: Vec<String>,
    /// 这个模型声明支持的对话中途工具更新模式；只认 `addition-only`（其他值读成 None）。
    /// 只增可选字段：旧设置文件/旧 live 列表原样读。
    #[serde(
        default,
        deserialize_with = "deserialize_tool_update",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_update: Option<ToolUpdate>,
}

/// 只认 `addition-only`；不认识的字符串（例如 DSH 的 `in-history`）读成 None，不报错。
fn deserialize_tool_update<'de, D>(deserializer: D) -> Result<Option<ToolUpdate>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.as_deref().and_then(ToolUpdate::parse))
}

impl CatalogSnapshot {
    pub fn provider(&self, provider_id: &str) -> Option<&CatalogProvider> {
        self.providers.get(provider_id)
    }

    pub fn model(&self, provider_id: &str, model_id: &str) -> Option<&CatalogModel> {
        self.provider(provider_id)?.models.get(model_id)
    }

    /// 这个 provider 的模型，字段来自目录。顺序是 BTreeMap 的键序（按 id），
    /// 不按版本或发布日期排序。
    pub fn models_for(&self, provider_id: &str) -> Vec<ModelInfo> {
        self.provider(provider_id)
            .map(|provider| {
                provider
                    .models
                    .values()
                    .map(|model| model_info(provider, model))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 路由声明支持的对话中途工具更新模式（适配器按它选两条路）。
    ///
    /// 写死的路由不要求模型出现在目录里：厂商目录可能漏项或改名，官方写死的那条仍要生效。
    pub fn tool_update_for(&self, provider_id: &str, model_id: &str) -> Option<ToolUpdate> {
        declared_tool_update(provider_id, model_id)
    }

    /// 适配器需要的思考能力：会思考吗、能选哪些档。
    pub fn reasoning_for(&self, provider_id: &str, model_id: &str) -> Option<(bool, Vec<String>)> {
        let provider = self.provider(provider_id)?;
        let model = provider.models.get(model_id)?;
        Some((model.reasoning, levels(provider, model)))
    }
}

fn model_info(provider: &CatalogProvider, model: &CatalogModel) -> ModelInfo {
    let max_tokens = model
        .limit
        .output
        .and_then(|value| u32::try_from(value).ok());
    ModelInfo {
        id: model.id.clone(),
        name: if model.name.is_empty() {
            model.id.clone()
        } else {
            model.name.clone()
        },
        context_window: model.limit.context,
        max_tokens,
        price_input: model.cost.as_ref().map(|cost| cost.input),
        price_output: model.cost.as_ref().map(|cost| cost.output),
        price_cache_read: model.cost.as_ref().and_then(|cost| cost.cache_read),
        price_cache_write: model.cost.as_ref().and_then(|cost| cost.cache_write),
        thinks: model.reasoning,
        efforts: levels(provider, model),
        tool_update: declared_tool_update(&provider.id, &model.id),
    }
}

/// 目录里写死的路由能力：models.dev 不声明 API 能力，DeepSeek 的官方 Flash 路由声明
/// `addition-only`（同 DSH 默认目录的 deepseek-flash）；其余模型没有来源，按没声明算。
///
/// 适配器每条请求都要问一次，所以这里不经过全局快照（不 clone 目录）。
pub fn declared_tool_update(provider_id: &str, model_id: &str) -> Option<ToolUpdate> {
    (provider_id == "deepseek" && model_id == "deepseek-flash").then_some(ToolUpdate::AdditionOnly)
}

/// 连接对应的目录 provider id：内置提供方用 preset，自定义地址按 endpoint 推断。
/// 日志里的厂商身份、统计定价和压缩策略匹配都用同一个来源。
pub fn provider_id_for_connection(preset: &str, endpoint: &str) -> String {
    if preset.is_empty() || preset == "custom" {
        provider_id_for_endpoint(endpoint)
    } else {
        preset.to_owned()
    }
}

/// 从 endpoint 推目录 provider id（预设 id 可直接用；custom 走这里）。
pub fn provider_id_for_endpoint(endpoint: &str) -> String {
    if endpoint.contains("deepseek") {
        "deepseek".to_owned()
    } else if endpoint.contains("minimaxi") || endpoint.contains("api.minimax.cn") {
        "minimax-cn".to_owned()
    } else if endpoint.contains("minimax") {
        "minimax".to_owned()
    } else {
        "custom".to_owned()
    }
}

/// 模型列表 URL（T012.5）：Anthropic base 是 `/v1/messages` 的根，列表在它的 Chat base
/// 的 `/v1/models`（Magpie catalog `FetchAt`：Anthropic base 与 Chat base 是各自根，列表按
/// Chat base 的 `/v1/models` 找）；预设三家都以 `…/anthropic` 结尾，直接拼会请求
/// `…/anthropic/v1/models`（审计 T012.5 fix-now 1）。
pub fn models_url(endpoint: &str) -> String {
    let base = endpoint.trim_end_matches('/');
    let chat_base = base.strip_suffix("/anthropic").unwrap_or(base);
    format!("{chat_base}/v1/models")
}

/// GET /v1/models 的模型项（厂商列表字段优先）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveModel {
    pub id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u32>,
    /// 厂商在 `api_capabilities.anthropic_messages.tool_update` 声明的能力；只认 addition-only。
    pub tool_update: Option<ToolUpdate>,
}

/// 解析厂商 /v1/models 的响应：数组，或 { data: [...] }（Anthropic/OpenAI 风格）。
pub fn parse_live_models(value: &Value) -> Vec<LiveModel> {
    let items = value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array));
    let Some(items) = items else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?.to_owned();
            let display_name = item
                .get("display_name")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let context_window = item
                .get("context_length")
                .or_else(|| item.get("context_window"))
                .and_then(Value::as_u64);
            let max_tokens = item
                .get("max_output_tokens")
                .or_else(|| item.get("max_tokens"))
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok());
            let tool_update = item
                .pointer("/api_capabilities/anthropic_messages/tool_update")
                .and_then(Value::as_str)
                .and_then(ToolUpdate::parse);
            Some(LiveModel {
                id,
                display_name,
                context_window,
                max_tokens,
                tool_update,
            })
        })
        .collect()
}

/// 把目录模型和厂商列表合并：顺序照厂商这次给的（不重排）；同一 id 厂商的 name/窗口/输出
/// 优先，目录价格/档保留；目录独有、厂商这次没给的模型排在后面。
pub fn merge_live(catalog_models: Vec<ModelInfo>, live: Vec<LiveModel>) -> Vec<ModelInfo> {
    let mut catalog_by_id: BTreeMap<String, ModelInfo> = catalog_models
        .into_iter()
        .map(|model| (model.id.clone(), model))
        .collect();
    let mut merged = Vec::with_capacity(catalog_by_id.len() + live.len());
    for live in live {
        let mut info = catalog_by_id.remove(&live.id).unwrap_or_else(|| ModelInfo {
            id: live.id.clone(),
            name: live.id.clone(),
            context_window: None,
            max_tokens: None,
            price_input: None,
            price_output: None,
            price_cache_read: None,
            price_cache_write: None,
            thinks: false,
            efforts: Vec::new(),
            tool_update: None,
        });
        if let Some(name) = live.display_name {
            info.name = name;
        }
        if live.context_window.is_some() {
            info.context_window = live.context_window;
        }
        if live.max_tokens.is_some() {
            info.max_tokens = live.max_tokens;
        }
        if live.tool_update.is_some() {
            info.tool_update = live.tool_update;
        }
        merged.push(info);
    }
    merged.extend(catalog_by_id.into_values());
    merged
}

/// 厂商这次给的列表转成可存的 ModelInfo：保持厂商顺序，字段只来自厂商。
pub fn live_models_info(live: &[LiveModel]) -> Vec<ModelInfo> {
    live.iter()
        .map(|live| ModelInfo {
            id: live.id.clone(),
            name: live.display_name.clone().unwrap_or_else(|| live.id.clone()),
            context_window: live.context_window,
            max_tokens: live.max_tokens,
            price_input: None,
            price_output: None,
            price_cache_read: None,
            price_cache_write: None,
            thinks: false,
            efforts: Vec::new(),
            tool_update: live.tool_update,
        })
        .collect()
}

/// 一个连接存下的厂商 live 列表（照 Magpie catalog.LivePath 的形状）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveList {
    /// 存下这份列表的时间（Unix 秒）。
    pub fetched_at: u64,
    /// 厂商 /v1/models 原样给的模型（不并目录、不写用户字段）。
    pub models: Vec<ModelInfo>,
}

/// 存 live 列表的目录：<数据目录>/catalog/models（Magpie LivePath，键换成连接 id）。
pub fn live_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("catalog").join("models")
}

/// 一个连接存下的 live 列表文件。连接 id 里可能有路径分隔符，文件名按字节转义。
pub fn live_path(data_dir: &Path, connection_id: &str) -> PathBuf {
    let mut name = String::with_capacity(connection_id.len());
    for byte in connection_id.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            name.push(char::from(byte));
        } else {
            name.push_str(&format!("%{byte:02X}"));
        }
    }
    live_dir(data_dir).join(format!("{name}.json"))
}

/// 读一个连接上次存下的厂商列表；没有/读不出来都是 None。
pub fn read_live(data_dir: &Path, connection_id: &str) -> Option<LiveList> {
    let text = std::fs::read_to_string(live_path(data_dir, connection_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 存一个连接这次从厂商拿到的列表；空列表不写（也不动上一次的文件）。
pub fn save_live(
    data_dir: &Path,
    connection_id: &str,
    fetched_at: u64,
    models: Vec<ModelInfo>,
) -> Result<(), String> {
    if models.is_empty() {
        return Ok(());
    }
    let path = live_path(data_dir, connection_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string(&LiveList { fetched_at, models })
        .map_err(|error| error.to_string())?;
    std::fs::write(path, json).map_err(|error| error.to_string())
}

/// 丢掉一个连接存下的厂商列表：换了厂商身份（地址或预设）以后，旧表不能再算数。
///
/// 没存过不算失败——那已经是「没有旧表」的状态。
pub fn remove_live(data_dir: &Path, connection_id: &str) -> Result<(), String> {
    match std::fs::remove_file(live_path(data_dir, connection_id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// 目录来源（缓存/快照/网络）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    Network,
    Cache,
    Snapshot,
}

/// 缓存文件：<数据目录>/catalog/models-dev.json。
pub fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("catalog").join("models-dev.json")
}

pub fn read_cache(data_dir: &Path) -> Option<CatalogSnapshot> {
    let text = std::fs::read_to_string(cache_path(data_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_cache(data_dir: &Path, snapshot: &CatalogSnapshot) -> Result<(), String> {
    let path = cache_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
    std::fs::write(path, json).map_err(|error| error.to_string())
}

pub fn is_fresh(modified: SystemTime, now: SystemTime) -> bool {
    match now.duration_since(modified) {
        Ok(age) => age < CACHE_TTL,
        // 时钟回拨/未来 mtime：当作刚写过，不再拉。
        Err(_) => true,
    }
}

fn cache_is_fresh(data_dir: &Path, now: SystemTime) -> bool {
    std::fs::metadata(cache_path(data_dir))
        .and_then(|metadata| metadata.modified())
        .map(|modified| is_fresh(modified, now))
        .unwrap_or(false)
}

/// 拉 models.dev（用统一的出站 client；测试注入 URL）。
pub async fn fetch(url: &str) -> Result<CatalogSnapshot, String> {
    let client = crate::http::client().map_err(|error| error.message)?;
    let response = client
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("models.dev 返回 {}", response.status()));
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

/// 缓存新鲜用缓存；否则拉网络；拉不到按缓存、再按内置快照。
pub async fn load_with(
    data_dir: &Path,
    url: &str,
    now: SystemTime,
) -> (CatalogSnapshot, CatalogSource) {
    let cached = read_cache(data_dir);
    if cache_is_fresh(data_dir, now)
        && let Some(cached) = &cached
    {
        return (cached.clone(), CatalogSource::Cache);
    }
    match fetch(url).await {
        Ok(snapshot) => {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "缓存写失败不挡住本次目录使用，下次会重新拉网络"
            )]
            let _ = write_cache(data_dir, &snapshot);
            (snapshot, CatalogSource::Network)
        }
        Err(_) => match cached {
            Some(snapshot) => (snapshot, CatalogSource::Cache),
            None => (snapshot(), CatalogSource::Snapshot),
        },
    }
}

/// 生产入口：当前时间 + models.dev 正式地址。
pub async fn load(data_dir: &Path, now: SystemTime) -> (CatalogSnapshot, CatalogSource) {
    load_with(data_dir, MODELS_DEV_URL, now).await
}

/// 内置快照（解析一次）。
pub fn snapshot() -> CatalogSnapshot {
    static SNAPSHOT: OnceLock<CatalogSnapshot> = OnceLock::new();
    SNAPSHOT
        .get_or_init(|| parse_snapshot(snapshot_json()))
        .clone()
}

/// 进程内的当前目录（适配器和获取可用模型都读它）。
static GLOBAL: OnceLock<RwLock<CatalogSnapshot>> = OnceLock::new();

pub fn set_global(snapshot: CatalogSnapshot) {
    if let Some(lock) = GLOBAL.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = snapshot;
        }
    } else {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "并发初始化只保留第一份目录，后到的设置忽略"
        )]
        let _ = GLOBAL.set(RwLock::new(snapshot));
    }
}

pub fn global() -> Option<CatalogSnapshot> {
    GLOBAL
        .get()
        .and_then(|lock| lock.read().ok().map(|guard| guard.clone()))
}

/// 启动时先用缓存/快照（不等网络），返回来源；随后由后台 refresh 换新。
pub fn bootstrap_global(data_dir: &Path) -> CatalogSource {
    match read_cache(data_dir) {
        Some(cached) => {
            set_global(cached);
            CatalogSource::Cache
        }
        None => {
            set_global(snapshot());
            CatalogSource::Snapshot
        }
    }
}

/// 后台刷新：拉网络/缓存/快照并更新 GLOBAL。
pub async fn refresh_global(data_dir: &Path, now: SystemTime) -> CatalogSource {
    let (snapshot, source) = load(data_dir, now).await;
    set_global(snapshot);
    source
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: &str = include_str!("../tests/test_data/models.dev-mini.json");

    fn lookup<'a>(
        snapshot: &'a CatalogSnapshot,
        provider_id: &str,
        model_id: &str,
    ) -> (&'a CatalogProvider, &'a CatalogModel) {
        let provider = snapshot
            .providers
            .get(provider_id)
            .expect("provider 在快照里");
        let model = provider.models.get(model_id).expect("model 在快照里");
        (provider, model)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aios-catalog-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn toggle_adds_none_only_for_deepseek() {
        let snapshot = serde_json::from_str(SNAPSHOT).expect("snapshot");
        let (provider, model) = lookup(&snapshot, "deepseek", "deepseek-flash");
        assert_eq!(
            efforts(model, &provider.id),
            vec!["none", "low", "high", "max"]
        );
        let (provider, model) = lookup(&snapshot, "minimax-cn", "MiniMax-M3");
        assert_eq!(efforts(model, &provider.id), Vec::<String>::new());
    }

    #[test]
    fn borrowing_levels_from_the_same_family() {
        let snapshot = deserialize_snapshot(SNAPSHOT);
        let (provider, model) = lookup(&snapshot, "xai", "grok-4.20-0309-reasoning");
        assert_eq!(
            levels(provider, model),
            vec!["low", "medium", "high", "xhigh"]
        );
    }

    #[test]
    fn budget_only_and_no_kin_models_have_no_levels() {
        let snapshot = deserialize_snapshot(SNAPSHOT);
        let (provider, model) = lookup(&snapshot, "commandcode", "claude-haiku-4-5-20251001");
        assert_eq!(levels(provider, model), Vec::<String>::new());
        let (provider, model) = lookup(&snapshot, "minimax-cn", "MiniMax-M3");
        assert_eq!(levels(provider, model), Vec::<String>::new());
    }

    #[test]
    fn calls_ones_own_efforts_levels() {
        let snapshot = deserialize_snapshot(SNAPSHOT);
        let (provider, model) = lookup(&snapshot, "minimax-cn", "MiniMax-M3.1-Flash");
        assert_eq!(
            levels(provider, model),
            vec!["low", "medium", "high", "xhigh", "max"]
        );
    }

    /// 列表跟厂商 live 顺序走：厂商怎么排就怎么显示，不按版本/目录重排；
    /// 目录这次没给的模型排在后面，目录字段照常合并。
    #[test]
    fn live_merge_keeps_vendor_order() {
        let info = |id: &str, price: f64| ModelInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            context_window: Some(1_000),
            max_tokens: None,
            price_input: Some(price),
            price_output: None,
            price_cache_read: None,
            price_cache_write: None,
            thinks: true,
            efforts: vec!["low".to_owned()],
            tool_update: None,
        };
        let live = |id: &str| LiveModel {
            id: id.to_owned(),
            display_name: None,
            context_window: Some(2_000),
            max_tokens: None,
            tool_update: None,
        };
        let merged = merge_live(
            vec![info("catalog-a", 0.1), info("catalog-b", 0.2)],
            vec![live("vendor-new"), live("catalog-b")],
        );
        let ids: Vec<&str> = merged.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["vendor-new", "catalog-b", "catalog-a"],
            "厂商顺序在前，目录独有项在后"
        );
        assert_eq!(merged[1].price_input, Some(0.2), "目录字段照常合并");
        assert_eq!(merged[0].price_input, None, "目录没有的模型没有标价");
    }

    #[test]
    fn built_in_snapshot_has_the_three_vendors_and_fields() {
        let snapshot = snapshot();
        let models = snapshot.models_for("deepseek");
        let flash = models
            .iter()
            .find(|model| model.id == "deepseek-flash")
            .expect("deepseek-flash");
        assert!(flash.thinks);
        assert_eq!(flash.efforts, vec!["none", "low", "high", "max"]);
        assert!(flash.context_window.is_some());
        assert!(flash.max_tokens.is_some());
        assert!(flash.price_input.is_some());
        assert!(!snapshot.models_for("minimax-cn").is_empty());
        assert_eq!(
            provider_id_for_endpoint("https://api.minimaxi.com/anthropic"),
            "minimax-cn"
        );
        assert_eq!(
            provider_id_for_endpoint("https://api.deepseek.com/anthropic"),
            "deepseek"
        );
        assert_eq!(
            provider_id_for_endpoint("https://relay.example.com"),
            "custom"
        );
    }

    #[tokio::test]
    async fn load_prefers_network_then_cache_then_snapshot() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/catalog.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(snapshot_json()))
            .mount(&server)
            .await;
        let url = format!("{}/catalog.json", server.uri());
        let data_dir = temp_dir("load");
        let now = SystemTime::now();

        let (network, source) = load_with(&data_dir, &url, now).await;
        assert_eq!(source, CatalogSource::Network);
        assert!(cache_path(&data_dir).exists());
        assert_eq!(
            network.models_for("deepseek").len(),
            snapshot().models_for("deepseek").len()
        );

        // 缓存新鲜：不再拉网络（即使 URL 不可用也走缓存）。
        let (cached, source) = load_with(&data_dir, "http://127.0.0.1:1/nope", now).await;
        assert_eq!(source, CatalogSource::Cache);
        assert_eq!(
            cached.models_for("minimax-cn"),
            network.models_for("minimax-cn")
        );

        // 缓存过期 + 网络失败：仍用缓存。
        let future = now + Duration::from_secs(48 * 60 * 60);
        let (stale, source) = load_with(&data_dir, "http://127.0.0.1:1/nope", future).await;
        assert_eq!(source, CatalogSource::Cache);
        assert_eq!(stale.models_for("deepseek"), network.models_for("deepseek"));

        // 没有缓存 + 网络失败：内置快照，字段与快照一致。
        let empty_dir = temp_dir("snapshot");
        let (fallback, source) = load_with(&empty_dir, "http://127.0.0.1:1/nope", now).await;
        assert_eq!(source, CatalogSource::Snapshot);
        assert_eq!(
            fallback.models_for("minimax"),
            snapshot().models_for("minimax")
        );

        std::fs::remove_dir_all(data_dir).ok();
        std::fs::remove_dir_all(empty_dir).ok();
    }

    #[test]
    fn live_merge_lets_vendor_fields_win_and_keeps_catalog_price() {
        let catalog = snapshot().models_for("deepseek");
        let live = vec![
            LiveModel {
                id: "deepseek-flash".to_owned(),
                display_name: Some("DeepSeek Flash (vendor)".to_owned()),
                context_window: Some(123),
                max_tokens: Some(456),
                tool_update: None,
            },
            LiveModel {
                id: "deepseek-future".to_owned(),
                display_name: Some("Future".to_owned()),
                context_window: Some(999),
                max_tokens: None,
                tool_update: None,
            },
        ];
        let merged = merge_live(catalog, live);
        let flash = merged
            .iter()
            .find(|model| model.id == "deepseek-flash")
            .expect("flash");
        assert_eq!(flash.name, "DeepSeek Flash (vendor)");
        assert_eq!(flash.context_window, Some(123));
        assert_eq!(flash.max_tokens, Some(456));
        assert!(flash.price_input.is_some(), "目录价格保留");
        assert!(flash.efforts.contains(&"none".to_owned()));
        let future = merged
            .iter()
            .find(|model| model.id == "deepseek-future")
            .expect("future");
        assert_eq!(future.context_window, Some(999));
        assert!(!future.thinks);
    }

    #[test]
    fn parse_live_models_accepts_array_and_data_wrapper() {
        let wrapped: Value = serde_json::from_str(
            r#"{"data":[{"id":"m1","display_name":"M1","context_length":1000,"max_output_tokens":200}]}"#,
        )
        .unwrap();
        let models = parse_live_models(&wrapped);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "m1");
        assert_eq!(models[0].context_window, Some(1000));
        let array: Value = serde_json::from_str(
            r#"[{"id":"m2","name":"M2","context_window":3000,"max_tokens":400}]"#,
        )
        .unwrap();
        assert_eq!(parse_live_models(&array)[0].max_tokens, Some(400));
    }

    /// 目录里 deepseek-flash 写死 addition-only（同 DSH 默认目录）；其余模型没有来源。
    #[test]
    fn catalog_declares_deepseek_flash_addition_only() {
        let snapshot = snapshot();
        let models = snapshot.models_for("deepseek");
        let flash = models
            .iter()
            .find(|model| model.id == "deepseek-flash")
            .expect("deepseek-flash");
        assert_eq!(flash.tool_update, Some(ToolUpdate::AdditionOnly));
        let plain = models
            .iter()
            .find(|model| model.id == "deepseek-v4-flash")
            .expect("deepseek-v4-flash");
        assert_eq!(plain.tool_update, None, "没写死的模型不声明能力");
        assert!(
            snapshot
                .models_for("minimax-cn")
                .iter()
                .all(|model| model.tool_update.is_none()),
            "MiniMax 没有来源，按没声明算"
        );
        assert_eq!(
            snapshot.tool_update_for("deepseek", "deepseek-flash"),
            Some(ToolUpdate::AdditionOnly)
        );
        assert_eq!(
            snapshot.tool_update_for("deepseek", "mystery-model"),
            None,
            "目录里没有的模型按没声明算"
        );
    }

    /// 获取模型时读厂商返回的 `api_capabilities.anthropic_messages.tool_update`：只认
    /// addition-only；读到的能力落进模型信息（驼峰 `toolUpdate`）。
    #[test]
    fn vendor_tool_update_is_read_from_the_model_listing() {
        let listing: Value = serde_json::from_str(
            r#"{"data":[
                {"id":"m1","api_capabilities":{"anthropic_messages":{"tool_update":"addition-only"}}},
                {"id":"m2","api_capabilities":{"anthropic_messages":{"tool_update":"in-history"}}},
                {"id":"m3"}
            ]}"#,
        )
        .unwrap();
        let models = parse_live_models(&listing);
        assert_eq!(models[0].tool_update, Some(ToolUpdate::AdditionOnly));
        assert_eq!(models[1].tool_update, None, "只认 addition-only");
        assert_eq!(models[2].tool_update, None, "没声明就是没能力");
        let saved = serde_json::to_value(live_models_info(&models)).unwrap();
        assert_eq!(saved[0]["toolUpdate"], "addition-only");
        assert!(
            saved[2].get("toolUpdate").is_none(),
            "没有能力时不写字段：{saved}"
        );
    }

    /// 厂商字段写进合并后的模型信息；厂商这次没声明时保留目录的能力。
    #[test]
    fn live_merge_carries_vendor_tool_update() {
        let flash = snapshot()
            .models_for("deepseek")
            .into_iter()
            .find(|model| model.id == "deepseek-flash")
            .expect("deepseek-flash");
        let merged = merge_live(
            vec![flash],
            vec![LiveModel {
                id: "deepseek-flash".to_owned(),
                display_name: None,
                context_window: None,
                max_tokens: None,
                tool_update: None,
            }],
        );
        assert_eq!(
            merged[0].tool_update,
            Some(ToolUpdate::AdditionOnly),
            "厂商没声明时保留目录能力"
        );
        let merged = merge_live(
            Vec::new(),
            vec![LiveModel {
                id: "deepseek-new".to_owned(),
                display_name: None,
                context_window: None,
                max_tokens: None,
                tool_update: Some(ToolUpdate::AdditionOnly),
            }],
        );
        assert_eq!(
            merged[0].tool_update,
            Some(ToolUpdate::AdditionOnly),
            "厂商声明的能力落进模型信息"
        );
    }

    #[test]
    fn models_url_asks_the_chat_base_for_anthropic_endpoints() {
        // 审计 T012.5 fix-now 1：预设三家都以 …/anthropic 结尾，列表在 Chat base 的 /v1/models。
        assert_eq!(
            models_url("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            models_url("https://api.minimaxi.com/anthropic/"),
            "https://api.minimaxi.com/v1/models"
        );
        assert_eq!(
            models_url("https://api.minimax.io/anthropic"),
            "https://api.minimax.io/v1/models"
        );
        // 已经指向 Chat base 的自定义地址：原样拼 /v1/models。
        assert_eq!(
            models_url("https://relay.example.com"),
            "https://relay.example.com/v1/models"
        );
    }

    #[test]
    fn empty_live_list_is_not_written_and_keeps_the_previous_file() {
        let dir = temp_dir("live-empty");
        let saved = live_models_info(&[LiveModel {
            id: "m1".to_owned(),
            display_name: None,
            context_window: None,
            max_tokens: None,
            tool_update: None,
        }]);
        save_live(&dir, "conn", 7, saved).expect("存列表");
        save_live(&dir, "conn", 8, Vec::new()).expect("空表不写");
        let live = read_live(&dir, "conn").expect("上一次的文件还在");
        assert_eq!(live.fetched_at, 7, "空表不动上一次的时间");
        assert_eq!(
            live.models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1"]
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn unreadable_live_file_reads_as_missing() {
        let dir = temp_dir("live-broken");
        assert!(read_live(&dir, "conn").is_none(), "没存过是 None");
        std::fs::create_dir_all(live_dir(&dir)).expect("列表目录");
        std::fs::write(live_path(&dir, "conn"), "").expect("空文件");
        assert!(read_live(&dir, "conn").is_none(), "空文件读不出来");
        std::fs::write(live_path(&dir, "conn"), "{\"fetchedAt\":").expect("坏 JSON");
        assert!(read_live(&dir, "conn").is_none(), "坏 JSON 读不出来");
        std::fs::remove_dir_all(dir).ok();
    }
}
