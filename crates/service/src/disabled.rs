//! 屏蔽的模型（照 Magpie `internal/provider/disabled.go`）：规则存在 settings.json，
//! 每次出站请求之前都查一遍，拦在发往厂商之前。
//!
//! 匹配不分大小写；`*` 是任意一串（可以跨 `/`），`?` 是任一字符；规则去掉首尾空白，
//! 空规则跳过。一个模型认三种名字：`<连接>/<模型>`、模型 id、id 的最后一段。

use std::path::Path;

use crate::error::ServiceError;
use crate::protocol::{
    DisabledModelUser, DisabledModelsCheckRequest, DisabledModelsCheckResponse,
    DisabledModelsSetRequest, DisabledModelsSetResponse,
};
use crate::root_lifecycle::RootHost;
use crate::settings;

/// 规则列表规范化：去首尾空白、丢掉空规则、不分大小写去重、保持原来的顺序。
pub fn normalize(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for pattern in list {
        let pattern = pattern.trim();
        if pattern.is_empty()
            || out
                .iter()
                .any(|existing| lowercase(existing) == lowercase(pattern))
        {
            continue;
        }
        out.push(pattern.to_owned());
    }
    out
}

/// 这个模型认哪些名字：模型 id、`<连接>/<模型>`、id 的最后一段。
pub fn names_of(connection: &str, model: &str) -> Vec<String> {
    let model = model.trim();
    let mut names = vec![model.to_owned()];
    if !connection.is_empty() {
        names.push(format!("{connection}/{model}"));
    }
    if let Some(index) = model.rfind('/')
        && index + 1 < model.len()
    {
        names.push(model[index + 1..].to_owned());
    }
    names
}

/// 简单通配（照 Magpie `globMatch`）：`*` 是任意一串，`/` 也算；`?` 是任一字符；
/// 其余照字面。两边先按字节走，和来源一致（ASCII 通配符 + UTF-8 字面量）。
pub fn glob_match(pattern: &str, value: &str) -> bool {
    if !pattern.contains(['*', '?']) {
        return pattern == value;
    }
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut star: i64 = -1;
    let mut mark = 0_usize;
    let mut pi = 0_usize;
    let mut si = 0_usize;
    while si < value.len() {
        if pi < pattern.len() && pattern[pi] == b'*' {
            star = pi as i64;
            mark = si;
            pi += 1;
        } else if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == value[si]) {
            pi += 1;
            si += 1;
        } else if star >= 0 {
            mark += 1;
            pi = star as usize + 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

/// 命中这个模型的那条规则（原样返回规则文本）；没有就是 None。
pub fn match_disabled(list: &[String], connection: &str, model: &str) -> Option<String> {
    for pattern in list {
        let lowered = lowercase(pattern.trim());
        if lowered.is_empty() {
            continue;
        }
        for name in names_of(connection, model) {
            if name.is_empty() {
                continue;
            }
            if glob_match(&lowered, &lowercase(&name)) {
                return Some(pattern.clone());
            }
        }
    }
    None
}

/// 现在的禁用表（settings.json 每次现读，不缓存）。
pub fn disabled_models(data_dir: &Path) -> Vec<String> {
    settings::load(data_dir).disabled_models
}

/// 被屏蔽的模型被拒时给用户看的话：说出模型和命中的规则，并指出两个出口。
pub fn refusal(model: &str, pattern: &str) -> String {
    format!(
        "{model} 已在设置里屏蔽（规则「{pattern}」）。到设置 › 模型与连接里解除，或者换一个模型"
    )
}

/// 出站之前查一次：被屏蔽就返回 `MODEL_DISABLED`，不发请求（也不重试）。
///
/// 这一层不区分是谁在发：聊天、重试、压缩摘要、验证连接都经过同一条出站接缝。
pub fn admit(data_dir: &Path, connection: &str, model: &str) -> Result<(), model::ModelError> {
    if let Some(pattern) = match_disabled(&disabled_models(data_dir), connection, model) {
        return Err(model::ModelError::new(
            model::ErrorCode::ModelDisabled,
            refusal(model, &pattern),
        ));
    }
    Ok(())
}

impl RootHost {
    /// 整表替换屏蔽规则：立即保存、立即生效，并存好后发设置变化事件。
    pub async fn disabled_models_set(
        &self,
        request: DisabledModelsSetRequest,
    ) -> Result<DisabledModelsSetResponse, ServiceError> {
        let models = normalize(&request.models);
        let mut settings = settings::load(&self.options.data_dir);
        settings.disabled_models = models.clone();
        settings::save(&self.options.data_dir, &settings).map_err(ServiceError::SettingsInvalid)?;
        // 立即生效：页面不用刷新；订阅端收到后重拉设置/屏蔽表。
        self.emit_settings_changed();
        Ok(DisabledModelsSetResponse { models })
    }

    /// 一条规则现在匹配哪些模型、谁在用（确认框用）。
    ///
    /// 占位：命中和使用者要等本轮冻结的连接 × 模型查询；先返回空表。
    pub async fn disabled_models_check(
        &self,
        _request: DisabledModelsCheckRequest,
    ) -> Result<DisabledModelsCheckResponse, ServiceError> {
        Ok(DisabledModelsCheckResponse {
            hits: Vec::new(),
            users: Vec::<DisabledModelUser>::new(),
        })
    }
}

fn lowercase(value: &str) -> String {
    value.to_lowercase()
}
