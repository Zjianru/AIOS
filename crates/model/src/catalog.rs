//! T012.3/T012.5：models.dev 目录里的推理能力规则（照 Magpie catalog.go）。
//!
//! 目录只决定两件事：模型会不会思考（thinks）、能选哪些档（efforts）。
//! 本模块先做纯规则；T012.5 负责拉取、缓存和内置快照。

use std::collections::BTreeMap;

use serde::Deserialize;

pub fn deserialize_snapshot(raw: &str) -> CatalogSnapshot {
    match serde_json::from_str(raw) {
        Ok(snapshot) => snapshot,
        Err(error) => panic!("内置目录测试数据必须是合法 JSON: {error}"),
    }
}

/// models.dev 快照的一角（测试用；T012.5 会接完整目录）。
#[derive(Debug, Deserialize)]
pub struct CatalogSnapshot {
    pub providers: BTreeMap<String, CatalogProvider>,
}

#[derive(Debug, Deserialize)]
pub struct CatalogProvider {
    pub id: String,
    #[serde(default)]
    pub models: BTreeMap<String, CatalogModel>,
}

#[derive(Debug, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub reasoning_options: Vec<ReasoningOption>,
}

#[derive(Debug, Deserialize)]
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

/// 目录只对 DeepSeek 的 toggle 追加 none（Magpie reasoningOffProviders；我们只接三家，只留 deepseek）。
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

    #[test]
    fn toggle_adds_none_only_for_deepseek() {
        let snapshot = deserialize_snapshot(SNAPSHOT);
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
}
