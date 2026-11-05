//! T012.2/T012.3：厂商之间的差别集中在这个文件。
//!
//! 只有一套 Anthropic Messages 实现；厂商差异按地址/模型名匹配后写在这里。
//! 本票新增：按地址/模型给档位、thinking 预算和 output_config.effort（R1–R3）。

use crate::{Effort, budget_of, fit_effort, output_effort_at};

/// 一个模型的思考能力（T012.3 的内置表；T012.5 接目录后由目录提供）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelReasoning {
    pub thinks: bool,
    /// 能选的档；空 = 会思考但不能调（只有 toggle）或不知道档。
    pub efforts: Vec<String>,
    /// 关不掉思考的模型（MiniMax M3.1-Flash 实测 2013）。
    pub always_thinks: bool,
}

fn model_is_minimax_m31_flash(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model == "minimax-m3.1-flash"
        || model.starts_with("minimax-m3.1-flash-")
        || model.starts_with("minimax-m3.1-flash.")
        || model.starts_with("minimax-m3.1-flash/")
        || model.starts_with("minimax-m3.1-flash[")
}

fn model_is_minimax_m3(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model == "minimax-m3" || model.starts_with("minimax-m3-") || model.starts_with("minimax-m3.")
}

fn host_of(base_url: &str) -> String {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default()
}

/// 内置的厂商思考能力表（T012.3 的临时来源；T012.5 换成 models.dev 目录）。
pub(crate) fn reasoning(base_url: &str, model: &str) -> ModelReasoning {
    let host = host_of(base_url);
    if host == "api.deepseek.com" {
        return ModelReasoning {
            thinks: true,
            efforts: vec![
                "none".to_owned(),
                "low".to_owned(),
                "high".to_owned(),
                "max".to_owned(),
            ],
            always_thinks: false,
        };
    }
    if model_is_minimax_m31_flash(model) {
        return ModelReasoning {
            thinks: true,
            efforts: vec![
                "low".to_owned(),
                "medium".to_owned(),
                "high".to_owned(),
                "xhigh".to_owned(),
                "max".to_owned(),
            ],
            always_thinks: true,
        };
    }
    if model_is_minimax_m3(model) {
        return ModelReasoning {
            thinks: true,
            efforts: Vec::new(),
            always_thinks: false,
        };
    }
    ModelReasoning {
        thinks: true,
        efforts: Vec::new(),
        always_thinks: false,
    }
}

/// 序列化时 thinking 的形态（构造请求体用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffortWire {
    pub thinking: Option<ThinkingWire>,
    pub output_effort: Option<String>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThinkingWire {
    Disabled,
    Enabled { budget_tokens: u32 },
}

/// 把一个想要的档按模型规则翻成请求参数（R3）。
pub(crate) fn wire_effort(
    base_url: &str,
    model: &str,
    effort: &Effort,
    max_tokens: Option<u32>,
    default_max_tokens: u32,
) -> EffortWire {
    let profile = reasoning(base_url, model);
    let mut wanted = effort.as_str().to_owned();
    if effort.is_off() {
        if !profile.always_thinks {
            return EffortWire {
                thinking: Some(ThinkingWire::Disabled),
                output_effort: None,
                max_tokens: max_tokens.unwrap_or(default_max_tokens),
            };
        }
        wanted = fit_effort("low", &profile.efforts);
    }
    let mut effective = max_tokens.unwrap_or(default_max_tokens);
    if effective <= 2048 && !profile.always_thinks {
        return EffortWire {
            thinking: None,
            output_effort: None,
            max_tokens: effective,
        };
    }
    let actual = fit_effort(&wanted, &profile.efforts);
    let mut budget = budget_of(&actual);
    match max_tokens {
        Some(given) => budget = budget.min(given.saturating_sub(1024).max(1)),
        None => effective = effective.max(budget + 4096),
    }
    let host = host_of(base_url);
    EffortWire {
        thinking: Some(ThinkingWire::Enabled {
            budget_tokens: budget,
        }),
        output_effort: output_effort_at(&host, model).then_some(actual),
        max_tokens: effective,
    }
}

/// 这一轮实际会发的档（用于日志；与 wire_effort 的选择一致）。
pub(crate) fn actual_effort(base_url: &str, model: &str, effort: &Effort) -> String {
    let profile = reasoning(base_url, model);
    if effort.is_off() && profile.always_thinks {
        return fit_effort("low", &profile.efforts);
    }
    fit_effort(effort.as_str(), &profile.efforts)
}

#[cfg(test)]
mod effort_state_tests {
    use super::*;

    #[test]
    fn wanted_none_on_an_always_thinking_model_logs_the_lowest_level() {
        let (wanted, actual) = (
            Effort::new("none"),
            actual_effort(
                "https://api.minimax.cn/anthropic",
                "MiniMax-M3.1-Flash",
                &Effort::new("none"),
            ),
        );
        assert_eq!(wanted.as_str(), "none");
        assert_eq!(actual, "low");
    }

    #[test]
    fn deepseek_high_stays_high() {
        assert_eq!(
            actual_effort(
                "https://api.deepseek.com/anthropic",
                "deepseek-flash",
                &Effort::new("high"),
            ),
            "high"
        );
    }
}
