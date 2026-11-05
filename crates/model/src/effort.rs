//! T012.3：思考档的内核规则（照 Magpie catalog.go / codexcat.go / gateway ir.go）。
//!
//! 档位是厂商自己的字符串；内核只认 EffortRank 的排序、两个别名、最近档和默认档。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 档位从弱到强的排序（Magpie EffortRank）。
pub const EFFORT_RANK: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

/// 厂商的别名对应的排序档（Magpie EffortAliases）。
pub const EFFORT_ALIASES: [(&str, &str); 2] = [("light", "low"), ("extra_high", "xhigh")];

/// 一个思考档。厂商叫法原样保存（"none"、"low"、"light"、"extra_high" 等都可）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Effort(String);

impl Effort {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 非空字符串都收；空串返回 None（CLI 用它报错）。
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        (!raw.is_empty()).then(|| Self(raw.to_owned()))
    }

    /// 关思考只认排序里的 none（厂商别名不映射到 none）。
    pub fn is_off(&self) -> bool {
        self.0 == "none"
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 厂商叫法对应的排序档；不是别名就返回自己（Magpie EffortLevel）。
pub fn effort_level(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    EFFORT_ALIASES
        .iter()
        .find_map(|(alias, level)| (*alias == lower).then_some(*level))
        .unwrap_or(name)
}

fn rank_of(name: &str) -> Option<i64> {
    let level = effort_level(name);
    EFFORT_RANK
        .iter()
        .position(|rank| *rank == level)
        .map(|index| index as i64)
}

/// 模型档位里离 want 最近的一档，一样远取高的；不是要 none 就不会给 none；
/// 模型没有档、本来就有这一档、或者 want 不认识，原样返回（Magpie NearestEffort）。
pub fn nearest_effort(want: &str, levels: &[String]) -> String {
    let Some(want_rank) = rank_of(want) else {
        return want.to_owned();
    };
    if levels.is_empty() || levels.iter().any(|level| level == want) {
        return want.to_owned();
    }
    let mut best: Option<(&str, i64)> = None;
    for level in levels {
        if level == "none" {
            continue;
        }
        let Some(rank) = rank_of(level) else {
            continue;
        };
        let distance = (rank - want_rank).abs();
        let take = match best {
            None => true,
            Some((_, best_distance)) => {
                distance < best_distance || distance == best_distance && rank > want_rank
            }
        };
        if take {
            best = Some((level.as_str(), distance));
        }
    }
    best.map(|(level, _)| level.to_owned())
        .unwrap_or_else(|| want.to_owned())
}

/// 新会话/没选过时的默认档：有 medium 用 medium，再有 high 用 high，否则第一档
/// （Magpie DefaultEffort；空档位按内核默认 high）。
pub fn default_effort(levels: &[String]) -> String {
    for wanted in ["medium", "high"] {
        if levels.iter().any(|level| level == wanted) {
            return wanted.to_owned();
        }
    }
    levels.first().cloned().unwrap_or_else(|| "high".to_owned())
}

/// 发请求前的适配：ultra 按 max，再取模型最近的档（Magpie fitEffort）。
pub fn fit_effort(want: &str, levels: &[String]) -> String {
    if want == "ultra" {
        let levels: Vec<String> = levels
            .iter()
            .filter(|level| level.as_str() != "ultra")
            .cloned()
            .collect();
        return nearest_effort("max", &levels);
    }
    nearest_effort(want, levels)
}

/// Anthropic thinking 预算（Magpie budgetOf）。
pub fn budget_of(effort: &str) -> u32 {
    match effort {
        "low" => 4096,
        "medium" => 10_000,
        "high" => 24_000,
        "xhigh" | "max" => 32_000,
        _ => 10_000,
    }
}

/// 模型把档放在 output_config.effort 而不是只看预算（Magpie outputEffortAt）：
/// DeepSeek 自己的地址所有模型，或模型名匹配 MiniMax-M3.1-Flash。
pub fn output_effort_at(host: &str, model: &str) -> bool {
    host == "api.deepseek.com" || {
        let model = model.to_ascii_lowercase();
        model == "minimax-m3.1-flash"
            || model.starts_with("minimax-m3.1-flash-")
            || model.starts_with("minimax-m3.1-flash.")
            || model.starts_with("minimax-m3.1-flash/")
            || model.starts_with("minimax-m3.1-flash[")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(levels: &[&str]) -> Vec<String> {
        levels.iter().map(|level| (*level).to_owned()).collect()
    }

    #[test]
    fn nearest_effort_table_from_magpie() {
        let table: &[(&str, &[&str], &str)] = &[
            ("xhigh", &["low", "medium", "high", "xhigh", "max"], "xhigh"),
            ("xhigh", &["low", "high", "max"], "max"),
            ("xhigh", &["low", "medium", "high"], "high"),
            ("max", &["low", "medium", "high"], "high"),
            ("medium", &["low", "high"], "high"),
            ("minimal", &["none", "low", "high"], "low"),
            ("none", &["low", "high", "max"], "low"),
            ("high", &[], "high"),
            ("odd", &["low"], "odd"),
        ];
        for (want, levels, expected) in table {
            assert_eq!(
                nearest_effort(want, &strings(levels)),
                *expected,
                "want={want} levels={levels:?}"
            );
        }
    }

    #[test]
    fn nearest_effort_takes_vendor_words() {
        let trae = strings(&["light", "high", "extra_high"]);
        let table: &[(&str, &str)] = &[
            ("low", "light"),
            ("light", "light"),
            ("minimal", "light"),
            ("medium", "high"),
            ("xhigh", "extra_high"),
            ("extra_high", "extra_high"),
            ("max", "extra_high"),
        ];
        for (want, expected) in table {
            assert_eq!(
                nearest_effort(want, &trae),
                *expected,
                "want={want} levels=trae"
            );
        }
        assert_eq!(nearest_effort("light", &strings(&["low", "high"])), "low");
        assert_eq!(
            nearest_effort("extra_high", &strings(&["low", "high", "max"])),
            "max"
        );
        assert_eq!(
            nearest_effort("medium", &strings(&["low", "high", "max"])),
            "high"
        );
    }

    #[test]
    fn default_effort_prefers_medium_then_high_then_first() {
        assert_eq!(
            default_effort(&strings(&["low", "medium", "high"])),
            "medium"
        );
        assert_eq!(default_effort(&strings(&["low", "high", "max"])), "high");
        assert_eq!(default_effort(&strings(&["low", "max"])), "low");
        assert_eq!(default_effort(&[]), "high");
    }

    #[test]
    fn fit_effort_sends_ultra_as_max() {
        assert_eq!(
            fit_effort("ultra", &strings(&["low", "high", "max"])),
            "max"
        );
        assert_eq!(
            fit_effort("ultra", &strings(&["low", "medium", "high"])),
            "high"
        );
        assert_eq!(
            fit_effort("xhigh", &strings(&["low", "high", "max"])),
            "max"
        );
    }

    #[test]
    fn budget_and_output_effort_rules() {
        assert_eq!(budget_of("low"), 4096);
        assert_eq!(budget_of("medium"), 10_000);
        assert_eq!(budget_of("high"), 24_000);
        assert_eq!(budget_of("xhigh"), 32_000);
        assert_eq!(budget_of("max"), 32_000);
        assert_eq!(budget_of("none"), 10_000);
        assert!(output_effort_at("api.deepseek.com", "deepseek-flash"));
        assert!(output_effort_at(
            "api.minimax.cn",
            "MiniMax-M3.1-Flash-Preview"
        ));
        assert!(!output_effort_at("api.minimax.cn", "MiniMax-M3"));
        assert!(!output_effort_at("relay.example.com", "deepseek-flash"));
    }
}
