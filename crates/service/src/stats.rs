//! T012.8：会话统计与花费（读的时候现算，不落日志）。
//!
//! 照 Magpie 的「生效价格 + 每条用量乘价格」：用户填的价格 > 目录标价 × 连接的倍率 > 无价格。
//! 无价格的轮不计入合计，合计带 unpriced 标记（spec 故事 4）。
//!
//! 缓存价：Magpie 的账本对 cache read/write 有单独价目；AIOS 的目录只有输入/输出两个标价，
//! 第一版把 cache read/write 按输入单价算（handback known，接真实价目表时再细分）。

use credentials::ConnectionInfo;
use model::Usage;
use model::catalog::CatalogSnapshot;
use serde::{Deserialize, Serialize};
use session::EventKind;

/// 生效价格（美元/百万 token；用户价不乘倍率）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectivePrice {
    pub input: f64,
    pub output: f64,
}

/// 这个连接的某个模型，现在生效的价格（None = 无价格，不按 0 算）。
pub fn effective_price(
    connection: &ConnectionInfo,
    catalog: &CatalogSnapshot,
    model: &str,
) -> Option<EffectivePrice> {
    // 1) 用户在这个连接里给模型填的价格（美元/百万 token，不乘倍率）。
    if let Some(row) = connection.models.iter().find(|row| row.id == model)
        && let Some(price) = row.price
    {
        return Some(EffectivePrice {
            input: price,
            output: price,
        });
    }
    // 2) 目录标价 × 连接倍率（0 = 免费）。
    let provider = if connection.preset.is_empty() || connection.preset == "custom" {
        model::catalog::provider_id_for_endpoint(&connection.endpoint)
    } else {
        connection.preset.clone()
    };
    let info = catalog
        .models_for(&provider)
        .into_iter()
        .find(|info| info.id == model)?;
    let input = info.price_input?;
    let output = info.price_output.unwrap_or(input);
    Some(EffectivePrice {
        input: input * connection.price_rate,
        output: output * connection.price_rate,
    })
}

/// 一条用量按价格折算的美元花费（四类 token 各自乘单价 / 1e6）。
pub fn usage_cost_usd(usage: &Usage, price: &EffectivePrice) -> f64 {
    let input_tokens = usage.input_tokens.unwrap_or(0)
        + usage.cache_read_input_tokens.unwrap_or(0)
        + usage.cache_creation_input_tokens.unwrap_or(0);
    let output_tokens = usage.output_tokens.unwrap_or(0);
    (input_tokens as f64 * price.input + output_tokens as f64 * price.output) / 1_000_000.0
}

/// 会话合计（status 的统计字段；规格见 DSH StatsPills/ContextMeter 与 spec 故事 4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionStats {
    pub turns: u64,
    pub steps: u64,
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "cacheReadTokens")]
    pub cache_read_tokens: u64,
    #[serde(rename = "cacheWriteTokens")]
    pub cache_write_tokens: u64,
    /// 上下文占用（token；页面不推算）。
    #[serde(rename = "contextUsed")]
    pub context_used: u64,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
    /// 合计美元花费；一个价格都没有时 None（页面不显示花费）。
    #[serde(rename = "costUsd", skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// 有轮没算进花费（合计后面带「+」）。
    pub unpriced: bool,
    pub connection: String,
    pub model: String,
    #[serde(rename = "effortWanted")]
    pub effort_wanted: String,
}

/// 从日志现算会话合计。价格按当前连接 + 目录；每轮的模型取 assistant/message 的
/// message.source.model（没有就按当前 model 算）。
pub fn session_stats(
    events: &[session::Event],
    connections: &[ConnectionInfo],
    initial: &session::ModelSelection,
    catalog: &CatalogSnapshot,
    current: &session::ModelSelection,
    context_used: u64,
    context_window: u64,
) -> SessionStats {
    let mut stats = SessionStats {
        context_used,
        context_window,
        connection: current.connection.clone(),
        model: current.model.clone(),
        effort_wanted: current.effort_wanted.clone(),
        ..SessionStats::default()
    };
    // 逐轮取「当时生效的连接」：事件流里 session/model 折叠；第一条之前用会话初始选择。
    let mut round_connection = initial.connection.clone();
    let mut round_model = initial.model.clone();
    let mut cost = 0.0_f64;
    let mut priced = 0_u64;
    for event in events {
        match &event.kind {
            EventKind::TurnStart { .. } => stats.turns += 1,
            EventKind::StepStart { .. } => stats.steps += 1,
            EventKind::SessionModel {
                connection, model, ..
            } => {
                round_connection = connection.clone();
                round_model = model.clone();
            }
            EventKind::AssistantMessage { message, usage, .. } => {
                let Some(usage) = usage else { continue };
                stats.input_tokens += usage.input_tokens.unwrap_or(0);
                stats.output_tokens += usage.output_tokens.unwrap_or(0);
                stats.cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
                stats.cache_write_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
                let model_name = match &message.source {
                    model::MessageSource::Model { model, .. } => model.clone(),
                    _ => round_model.clone(),
                };
                let connection = connections
                    .iter()
                    .find(|connection| connection.id == round_connection);
                match connection
                    .and_then(|connection| effective_price(connection, catalog, &model_name))
                {
                    Some(price) => {
                        cost += usage_cost_usd(usage, &price);
                        priced += 1;
                    }
                    None => stats.unpriced = true,
                }
            }
            _ => {}
        }
    }
    stats.cost_usd = (priced > 0).then_some(cost);
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::Usage;
    use model::catalog::CatalogSnapshot;
    use serde_json::json;

    fn connection(price_rate: f64, user_price: Option<f64>) -> ConnectionInfo {
        ConnectionInfo {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            saved_at: 0,
            check: credentials::CheckState::Never,
            key_saved: true,
            preset: "deepseek".to_owned(),
            models: vec![credentials::ModelRow {
                id: "deepseek-flash".to_owned(),
                name: None,
                context_window: None,
                max_tokens: None,
                price: user_price,
            }],
            price_rate,
            auth_header: None,
        }
    }

    fn catalog() -> CatalogSnapshot {
        model::catalog::snapshot()
    }

    fn usage() -> Usage {
        Usage {
            input_tokens: Some(1_000_000),
            output_tokens: Some(500_000),
            cache_read_input_tokens: Some(200_000),
            cache_creation_input_tokens: Some(100_000),
        }
    }

    fn selection() -> session::ModelSelection {
        session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        }
    }

    fn assistant_message(model: &str) -> session::EventKind {
        let message = model::Message::assistant(
            vec![model::ContentBlock::Text {
                text: "ok".to_owned(),
            }],
            "deepseek",
            model,
        );
        session::EventKind::AssistantMessage {
            turn: 1,
            step: 1,
            message,
            stream: Vec::new(),
            usage: Some(usage()),
            interrupted: None,
            effort: Some("high".to_owned()),
            effort_wanted: None,
        }
    }

    #[test]
    fn user_price_wins_and_is_not_multiplied() {
        // 用户价 2 美元/百万：输入 1.3M × 2 + 输出 0.5M × 2。
        let price = effective_price(&connection(3.0, Some(2.0)), &catalog(), "deepseek-flash")
            .expect("用户价");
        assert_eq!(price.input, 2.0);
        assert_eq!(price.output, 2.0);
        let cost = usage_cost_usd(&usage(), &price);
        assert!((cost - (1_300_000.0 * 2.0 + 500_000.0 * 2.0) / 1e6).abs() < 1e-9);
    }

    #[test]
    fn catalog_price_times_rate_and_zero_rate_is_free() {
        let catalog = catalog();
        let base =
            effective_price(&connection(1.0, None), &catalog, "deepseek-flash").expect("标价");
        let marked =
            effective_price(&connection(2.0, None), &catalog, "deepseek-flash").expect("标价×倍率");
        assert!((marked.input - base.input * 2.0).abs() < 1e-9);
        let free =
            effective_price(&connection(0.0, None), &catalog, "deepseek-flash").expect("倍率 0");
        assert_eq!(free.input, 0.0);
        assert_eq!(usage_cost_usd(&usage(), &free), 0.0);
        assert!(effective_price(&connection(1.0, None), &catalog, "no-such-model").is_none());
    }

    #[test]
    fn session_totals_aggregate_and_mark_unpriced() {
        let events = vec![
            session::Event::new(1, 1, session::EventKind::TurnStart { turn: 1 }),
            session::Event::new(2, 2, session::EventKind::StepStart { turn: 1, step: 1 }),
            session::Event::new(3, 3, assistant_message("deepseek-flash")),
            session::Event::new(4, 4, assistant_message("unknown-model")),
        ];
        let stats = session_stats(
            &events,
            std::slice::from_ref(&connection(1.0, None)),
            &selection(),
            &catalog(),
            &selection(),
            123,
            1000,
        );
        assert_eq!(stats.turns, 1);
        assert_eq!(stats.steps, 1);
        assert_eq!(stats.input_tokens, 2_000_000);
        assert_eq!(stats.cache_read_tokens, 400_000);
        assert_eq!(stats.cache_write_tokens, 200_000);
        assert!(stats.unpriced, "有轮没价格：合计带 +");
        assert!(stats.cost_usd.unwrap() > 0.0);
        assert_eq!(stats.context_used, 123);
        assert_eq!(stats.context_window, 1000);
    }

    fn connection_with(
        id: &str,
        endpoint: &str,
        preset: &str,
        rate: f64,
        user_price: Option<f64>,
    ) -> ConnectionInfo {
        ConnectionInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: endpoint.to_owned(),
            saved_at: 0,
            check: credentials::CheckState::Never,
            key_saved: true,
            preset: preset.to_owned(),
            models: vec![credentials::ModelRow {
                id: "deepseek-flash".to_owned(),
                name: None,
                context_window: None,
                max_tokens: None,
                price: user_price,
            }],
            price_rate: rate,
            auth_header: None,
        }
    }

    /// T012.8a 审计：两轮跨连接（DeepSeek 用户价 → MiniMax 倍率 0=免费）按各自连接定价。
    #[test]
    fn stats_price_each_round_with_its_own_connection() {
        let deepseek = connection_with(
            "deepseek",
            "https://api.deepseek.com/anthropic",
            "deepseek",
            1.0,
            Some(2.0),
        );
        let minimax = connection_with(
            "minimax-cn",
            "https://api.minimaxi.com/anthropic",
            "minimax-cn",
            0.0,
            None,
        );
        let events = vec![
            session::Event::new(1, 1, session::EventKind::TurnStart { turn: 1 }),
            session::Event::new(2, 2, assistant_message("deepseek-flash")),
            session::Event::new(
                3,
                3,
                session::EventKind::SessionModel {
                    connection: "minimax-cn".to_owned(),
                    model: "MiniMax-M3.1-Flash".to_owned(),
                    effort_wanted: "low".to_owned(),
                },
            ),
            session::Event::new(4, 4, session::EventKind::TurnStart { turn: 2 }),
            session::Event::new(5, 5, assistant_message("MiniMax-M3.1-Flash")),
        ];
        let current = session::ModelSelection {
            connection: "minimax-cn".to_owned(),
            model: "MiniMax-M3.1-Flash".to_owned(),
            effort_wanted: "low".to_owned(),
        };
        let deepseek_selection = session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        };
        let stats = session_stats(
            &events,
            &[deepseek.clone(), minimax.clone()],
            &deepseek_selection,
            &catalog(),
            &current,
            0,
            0,
        );
        let cost = stats.cost_usd.expect("DeepSeek 轮有用户价，应该有花费");
        let deepseek_cost = (1_300_000.0 * 2.0 + 500_000.0 * 2.0) / 1e6;
        assert!(
            (cost - deepseek_cost).abs() < 1e-6,
            "每轮按自己连接的价格算：{cost} vs {deepseek_cost}"
        );
    }

    #[test]
    fn no_priced_round_hides_cost() {
        let events = vec![session::Event::new(
            1,
            1,
            assistant_message("unknown-model"),
        )];
        let stats = session_stats(
            &events,
            std::slice::from_ref(&connection(1.0, None)),
            &selection(),
            &catalog(),
            &selection(),
            0,
            1000,
        );
        assert_eq!(stats.cost_usd, None, "一个价格都没有就不显示花费");
        assert!(stats.unpriced);
    }

    #[test]
    fn json_shape_uses_camel_case() {
        let value = serde_json::to_value(SessionStats::default()).unwrap();
        assert!(value.get("inputTokens").is_some(), "{value}");
        assert!(value.get("contextUsed").is_some(), "{value}");
        assert!(value.get("costUsd").is_none(), "无价格不序列化");
        let _ = json!({});
    }
}
