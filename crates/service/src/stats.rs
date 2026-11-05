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
    /// T012 F1：缓存读/写单价；None = 这一类没有价格（有用量就算无价格，不拿输入价顶）。
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// 这个连接的某个模型，现在生效的价格（None = 无价格，不按 0 算）。
pub fn effective_price(
    connection: &ConnectionInfo,
    catalog: &CatalogSnapshot,
    model: &str,
) -> Option<EffectivePrice> {
    // T012 F1：逐字段取价——用户填了的格原样用（不乘倍率），没填的格用目录标价 × 连接倍率；
    // 某格两边都没有就是 None（有这一类用量时这一轮无价格）。
    let user = connection.models.iter().find(|row| row.id == model);
    let provider = if connection.preset.is_empty() || connection.preset == "custom" {
        model::catalog::provider_id_for_endpoint(&connection.endpoint)
    } else {
        connection.preset.clone()
    };
    let info = catalog
        .models_for(&provider)
        .into_iter()
        .find(|info| info.id == model);
    let rate = connection.price_rate;
    let user_input = user.and_then(|row| row.price_input);
    // 旧连接只有一个数（alias 到输入价）：那个数同时是输出价。
    let user_output = user.and_then(|row| row.price_output).or(user_input);
    let user_cache_read = user.and_then(|row| row.price_cache_read);
    let user_cache_write = user.and_then(|row| row.price_cache_write);
    let catalog_input = info.as_ref().and_then(|info| info.price_input);
    let catalog_output = info
        .as_ref()
        .and_then(|info| info.price_output)
        .or(catalog_input);
    let catalog_cache_read = info.as_ref().and_then(|info| info.price_cache_read);
    let catalog_cache_write = info.as_ref().and_then(|info| info.price_cache_write);
    let input = user_input.or(catalog_input.map(|value| value * rate))?;
    let output = user_output
        .or(catalog_output.map(|value| value * rate))
        .unwrap_or(input);
    Some(EffectivePrice {
        input,
        output,
        cache_read: user_cache_read.or(catalog_cache_read.map(|value| value * rate)),
        cache_write: user_cache_write.or(catalog_cache_write.map(|value| value * rate)),
    })
}

/// 一条用量按价格折算的美元花费（T012 F1：四类 token 各自乘单价 / 1e6）。
///
/// 有缓存用量、对应那一类却没有单价时返回 None（这一轮无价格，不拿输入价顶）。
pub fn usage_cost_usd(usage: &Usage, price: &EffectivePrice) -> Option<f64> {
    let input_tokens = usage.input_tokens.unwrap_or(0);
    let output_tokens = usage.output_tokens.unwrap_or(0);
    let cache_read_tokens = usage.cache_read_input_tokens.unwrap_or(0);
    let cache_write_tokens = usage.cache_creation_input_tokens.unwrap_or(0);
    let mut cost = input_tokens as f64 * price.input + output_tokens as f64 * price.output;
    if cache_read_tokens > 0 {
        cost += cache_read_tokens as f64 * price.cache_read?;
    }
    if cache_write_tokens > 0 {
        cost += cache_write_tokens as f64 * price.cache_write?;
    }
    Some(cost / 1_000_000.0)
}

/// 这一轮实际会发的档（T012 F2：服务算好给页面，页面不另抄档位表）。
pub fn actual_effort_of(
    connection: &credentials::ConnectionInfo,
    selection: &session::ModelSelection,
) -> String {
    model::actual_effort(
        &connection.endpoint,
        &selection.model,
        &model::Effort::new(selection.effort_wanted.clone()),
    )
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
    /// T012 F2：这一轮实际会发的档（由内核按模型档位规则算；页面不另抄表）。
    #[serde(rename = "effortActual")]
    pub effort_actual: String,
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
    // 逐轮取「当时生效的连接」：session/model 只算 pending，到下一个 TurnStart 才生效
    // （T012.7 允许轮中 select；本轮后续请求仍是旧连接，T012.8 审计）。
    let mut round_connection = current.connection.clone();
    let mut round_model = current.model.clone();
    let mut pending_selection: Option<(String, String)> = None;
    let mut cost = 0.0_f64;
    let mut priced = 0_u64;
    for event in events {
        match &event.kind {
            EventKind::TurnStart { .. } => {
                stats.turns += 1;
                if let Some((connection, model)) = pending_selection.take() {
                    round_connection = connection;
                    round_model = model;
                }
            }
            EventKind::StepStart { .. } => stats.steps += 1,
            EventKind::SessionModel {
                connection, model, ..
            } => {
                pending_selection = Some((connection.clone(), model.clone()));
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
                    Some(price) => match usage_cost_usd(usage, &price) {
                        Some(round_cost) => {
                            cost += round_cost;
                            priced += 1;
                        }
                        None => stats.unpriced = true,
                    },
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
                price_input: user_price,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
            }],
            price_rate,
            auth_header: None,
        }
    }

    fn catalog() -> CatalogSnapshot {
        model::catalog::snapshot()
    }

    /// 只有输入输出、没有缓存用量的用量（不计缓存价时用）。
    fn usage_no_cache() -> Usage {
        Usage {
            input_tokens: Some(1_000_000),
            output_tokens: Some(500_000),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
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
            usage: Some(usage_no_cache()),
            interrupted: None,
            effort: Some("high".to_owned()),
            effort_wanted: None,
        }
    }

    #[test]
    fn user_price_wins_and_is_not_multiplied() {
        // 单数用户价 2 美元/百万 = 输入价和输出价；缓存两格没填 → 有缓存用量时无价格。
        let price = effective_price(&connection(3.0, Some(2.0)), &catalog(), "deepseek-flash")
            .expect("用户价");
        assert_eq!(price.input, 2.0);
        assert_eq!(price.output, 2.0);
        // 用户没填缓存两格 → 用目录标价×倍率（deepseek-flash 有缓存读价、没有缓存写价）。
        assert_eq!(price.cache_read, Some(0.003 * 3.0));
        assert!(price.cache_write.is_none());
        let cost = usage_cost_usd(&usage_no_cache(), &price).expect("无缓存用量");
        assert!((cost - (1_000_000.0 * 2.0 + 500_000.0 * 2.0) / 1e6).abs() < 1e-9);
        // 用户价不乘倍率：rate=3 也一样。
        let with_rate = effective_price(&connection(3.0, Some(2.0)), &catalog(), "deepseek-flash")
            .expect("用户价");
        assert_eq!(with_rate.input, 2.0);
        assert_eq!(with_rate.output, 2.0);
    }

    #[test]
    fn catalog_price_times_rate_and_zero_rate_is_free() {
        let catalog = catalog();
        let base =
            effective_price(&connection(1.0, None), &catalog, "deepseek-flash").expect("标价");
        let marked =
            effective_price(&connection(2.0, None), &catalog, "deepseek-flash").expect("标价×倍率");
        assert!((marked.input - base.input * 2.0).abs() < 1e-9);
        // 四类标价都乘倍率。
        assert_eq!(base.cache_read, Some(0.003));
        assert_eq!(marked.cache_read, Some(0.006));
        assert!(base.cache_write.is_none(), "deepseek 快照没有缓存写单价");
        let free =
            effective_price(&connection(0.0, None), &catalog, "deepseek-flash").expect("倍率 0");
        assert_eq!(free.input, 0.0);
        assert_eq!(free.cache_read, Some(0.0));
        assert_eq!(usage_cost_usd(&usage_no_cache(), &free), Some(0.0));
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
            &catalog(),
            &selection(),
            123,
            1000,
        );
        assert_eq!(stats.turns, 1);
        assert_eq!(stats.steps, 1);
        assert_eq!(stats.input_tokens, 2_000_000);
        assert_eq!(stats.cache_read_tokens, 0);
        assert_eq!(stats.cache_write_tokens, 0);
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
                price_input: user_price,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
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
            // 会话初始选择（open 会把它写成第一条 durable 事实）。
            session::Event::new(
                1,
                1,
                session::EventKind::SessionModel {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
            ),
            session::Event::new(2, 2, session::EventKind::TurnStart { turn: 1 }),
            session::Event::new(3, 3, assistant_message("deepseek-flash")),
            session::Event::new(
                4,
                4,
                session::EventKind::SessionModel {
                    connection: "minimax-cn".to_owned(),
                    model: "MiniMax-M3.1-Flash".to_owned(),
                    effort_wanted: "low".to_owned(),
                },
            ),
            session::Event::new(5, 5, session::EventKind::TurnStart { turn: 2 }),
            session::Event::new(6, 6, assistant_message("MiniMax-M3.1-Flash")),
        ];
        let current = session::ModelSelection {
            connection: "minimax-cn".to_owned(),
            model: "MiniMax-M3.1-Flash".to_owned(),
            effort_wanted: "low".to_owned(),
        };
        let stats = session_stats(
            &events,
            &[deepseek.clone(), minimax.clone()],
            &catalog(),
            &current,
            0,
            0,
        );
        let cost = stats.cost_usd.expect("DeepSeek 轮有用户价，应该有花费");
        let deepseek_cost = (1_000_000.0 * 2.0 + 500_000.0 * 2.0) / 1e6;
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

    /// T012.8 审计：会话选择在轮中写入时，本轮后续 assistant/message 仍按旧连接定价（pending 到下一个 TurnStart）。
    #[test]
    fn stats_holds_selection_change_until_next_turn() {
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
            // 会话初始选择（修复后 open 会把它写成第一条 durable 事实）。
            session::Event::new(
                1,
                1,
                session::EventKind::SessionModel {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
            ),
            session::Event::new(2, 2, session::EventKind::TurnStart { turn: 1 }),
            session::Event::new(3, 3, assistant_message("deepseek-flash")),
            // 用户在轮中换了连接（T012.7 允许）：本轮后面的请求仍是旧模型/旧连接。
            session::Event::new(
                4,
                4,
                session::EventKind::SessionModel {
                    connection: "minimax-cn".to_owned(),
                    model: "MiniMax-M3.1-Flash".to_owned(),
                    effort_wanted: "low".to_owned(),
                },
            ),
            session::Event::new(5, 5, assistant_message("deepseek-flash")),
            session::Event::new(
                6,
                6,
                session::EventKind::TurnEnd {
                    turn: 1,
                    reason: session::TurnEndReason::Completed,
                },
            ),
            session::Event::new(7, 7, session::EventKind::TurnStart { turn: 2 }),
            session::Event::new(8, 8, assistant_message("MiniMax-M3.1-Flash")),
        ];
        let current = session::ModelSelection {
            connection: "minimax-cn".to_owned(),
            model: "MiniMax-M3.1-Flash".to_owned(),
            effort_wanted: "low".to_owned(),
        };
        let stats = session_stats(
            &events,
            &[deepseek.clone(), minimax.clone()],
            &catalog(),
            &current,
            0,
            0,
        );
        let deepseek_cost = (1_000_000.0 * 2.0 + 500_000.0 * 2.0) / 1e6;
        assert!(
            !stats.unpriced,
            "本轮两条都按 DeepSeek 定价，不应无价：{stats:?}"
        );
        assert!(
            (stats.cost_usd.unwrap_or(0.0) - deepseek_cost * 2.0).abs() < 1e-6,
            "轮中换连接不影响本轮定价：{:?} vs {}",
            stats.cost_usd,
            deepseek_cost * 2.0
        );
    }

    /// T012 F1（Claude 审核）：DoD 会话用量 × 快照价——四类单价各自算，缓存读是输入价的 1/50。
    /// DoD 会话：输入 755、缓存读 24970、输出 354（无缓存写）；deepseek-flash 快照价
    /// input 0.15 / output 0.6 / cache_read 0.003（美元/百万 token）。
    #[test]
    fn deepseek_session_cost_uses_four_class_prices() {
        let connection = connection(1.0, None);
        let price = effective_price(&connection, &catalog(), "deepseek-flash").expect("快照价");
        let usage = Usage {
            input_tokens: Some(755),
            output_tokens: Some(354),
            cache_read_input_tokens: Some(24_970),
            cache_creation_input_tokens: Some(0),
        };
        let expected = (755.0 * 0.15 + 24_970.0 * 0.003 + 354.0 * 0.6) / 1e6;
        let cost = usage_cost_usd(&usage, &price).expect("四类价都有");
        assert!(
            (cost - expected).abs() < 1e-12,
            "四类单价：期望 {expected}，实际 {cost}（输入 755 缓存读 24970 输出 435）"
        );
        // 旧算法（把缓存读乘输入价）会高约 10 倍：≈0.00417。
        assert!(
            (cost - 0.000_400_56).abs() < 1e-12,
            "DoD 会话四类价应为 $0.00040056：{cost}"
        );
    }

    /// T012 F1：有缓存写用量、对应那一类没有单价 → 这一轮无价格，不拿输入价顶。
    #[test]
    fn cache_write_usage_without_a_cache_write_price_is_unpriced() {
        let catalog = catalog();
        let connection = connection(1.0, None);
        let price = effective_price(&connection, &catalog, "deepseek-flash").expect("快照价");
        assert!(price.cache_write.is_none(), "deepseek 快照没有缓存写单价");
        let usage = Usage {
            input_tokens: Some(100),
            output_tokens: Some(100),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(5_000),
        };
        let events = vec![
            session::Event::new(
                1,
                1,
                session::EventKind::SessionModel {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
            ),
            session::Event::new(2, 2, session::EventKind::TurnStart { turn: 1 }),
            session::Event::new(
                3,
                3,
                session::EventKind::AssistantMessage {
                    turn: 1,
                    step: 1,
                    message: model::Message::assistant(
                        vec![model::ContentBlock::Text {
                            text: "ok".to_owned(),
                        }],
                        "deepseek",
                        "deepseek-flash",
                    ),
                    stream: Vec::new(),
                    usage: Some(usage),
                    interrupted: None,
                    effort: Some("high".to_owned()),
                    effort_wanted: None,
                },
            ),
        ];
        let stats = session_stats(
            &events,
            std::slice::from_ref(&connection),
            &catalog,
            &selection(),
            0,
            0,
        );
        assert!(stats.unpriced, "缓存写没有单价 → 这一轮无价格：{stats:?}");
        assert!(stats.cost_usd.is_none(), "无价格不按 0 算：{stats:?}");
    }

    /// T012 F1：用户只填缓存读价——输入/输出仍按目录标价×倍率，缓存读用用户价。
    #[test]
    fn user_cache_read_price_fills_only_that_slot() {
        let mut conn = connection(2.0, None);
        conn.models[0].price_cache_read = Some(100.0);
        let price =
            effective_price(&conn, &catalog(), "deepseek-flash").expect("标价+用户缓存读价");
        assert_eq!(price.input, 0.15 * 2.0, "输入按标价×倍率");
        assert_eq!(price.output, 0.6 * 2.0, "输出按标价×倍率");
        assert_eq!(price.cache_read, Some(100.0), "缓存读用用户价（不乘倍率）");
        assert_eq!(price.cache_write, None, "缓存写没有标价也没有用户价");
        // 只填缓存读价时，有缓存写用量仍是无价格。
        let usage = Usage {
            input_tokens: Some(1000),
            output_tokens: Some(100),
            cache_read_input_tokens: Some(200),
            cache_creation_input_tokens: Some(50),
        };
        assert!(
            usage_cost_usd(&usage, &price).is_none(),
            "缓存写没有价 → 无价格"
        );
        let without_write = Usage {
            cache_creation_input_tokens: Some(0),
            ..usage
        };
        let cost = usage_cost_usd(&without_write, &price).expect("缓存写为 0");
        assert!(cost > 0.0);
    }
}
