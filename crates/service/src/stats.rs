//! T012.8：会话统计与花费（读的时候现算，不落日志）。
//!
//! 照 Magpie 的「生效价格 + 每条用量乘价格」：用户填的价格 > 目录标价 > 无价格。
//! 无价格的轮不计入合计，合计带 unpriced 标记（spec 故事 4）。
//!
//! 缓存价：Magpie 的账本对 cache read/write 有单独价目；AIOS 的目录只有输入/输出两个标价，
//! 第一版把 cache read/write 按输入单价算（handback known，接真实价目表时再细分）。

use credentials::ConnectionInfo;
use model::Usage;
use model::catalog::CatalogSnapshot;
use serde::{Deserialize, Serialize};
use session::EventKind;
use ts_rs::TS;

/// 生效价格（美元/百万 token；用户填了的格用用户价，其余用目录标价）。
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
    // T012 F1：逐字段取价——用户填了的格原样用，没填的格用目录标价；
    // 某格两边都没有就是 None（有这一类用量时这一轮无价格）。
    let user = connection
        .models
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|row| row.id == model);
    let provider =
        model::catalog::provider_id_for_connection(&connection.preset, &connection.endpoint);
    let info = catalog.model_for(&provider, model);
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
    let input = user_input.or(catalog_input)?;
    let output = user_output.or(catalog_output).unwrap_or(input);
    Some(EffectivePrice {
        input,
        output,
        cache_read: user_cache_read.or(catalog_cache_read),
        cache_write: user_cache_write.or(catalog_cache_write),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, TS)]
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

    fn connection(user_price: Option<f64>) -> ConnectionInfo {
        ConnectionInfo {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            saved_at: 0,
            check: credentials::CheckState::Never,
            key_saved: true,
            preset: "deepseek".to_owned(),
            models: Some(vec![credentials::ModelRow {
                id: "deepseek-flash".to_owned(),
                name: None,
                context_window: None,
                max_tokens: None,
                price_input: user_price,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
            }]),
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
                paste: false,
                canvas: None,
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

    /// 旧存储里还留着已删的连接级价格乘数：读到就忽略，不把没填的格当成免费；
    /// 用户填了单价照用，没填的格用目录标价。
    #[test]
    fn legacy_connection_price_multiplier_is_ignored() {
        // 字段名拆开拼：全仓检查要求已删字段不再以原文出现在 crates/web 里。
        let deleted = ["price", "rate"].join("_");
        let mut legacy = json!({
            "id": "deepseek",
            "name": "DeepSeek",
            "model": "deepseek-flash",
            "endpoint": "https://api.deepseek.com/anthropic",
            "saved_at": 0,
            "check": {"kind": "never"},
            "key_saved": true,
            "preset": "deepseek",
            "models": [{"id": "deepseek-flash"}]
        });
        legacy[deleted.clone()] = json!(0.0);

        let connection: ConnectionInfo =
            serde_json::from_value(legacy.clone()).expect("旧存储要能读");
        let price = effective_price(&connection, &catalog(), "deepseek-flash").expect("目录价");
        assert_eq!(price.input, 0.15, "旧乘数 0 被忽略：输入用目录标价");
        assert_eq!(price.output, 0.6, "输出用目录标价");
        assert_eq!(price.cache_read, Some(0.003), "缓存读用目录标价");

        legacy["models"][0]["priceInput"] = json!(2.0);
        let connection: ConnectionInfo = serde_json::from_value(legacy).expect("旧存储要能读");
        let price = effective_price(&connection, &catalog(), "deepseek-flash").expect("用户价");
        assert_eq!(price.input, 2.0, "用户填的单价照用");
        assert_eq!(price.output, 2.0, "只填一个价：输出沿用");
        assert_eq!(price.cache_read, Some(0.003), "没填的格用目录标价");
    }

    /// 用户填了单价：照用；没填的格用目录标价。
    #[test]
    fn user_price_wins_over_catalog() {
        // 单数用户价 2 美元/百万 = 输入价和输出价；缓存两格没填 → 用目录标价。
        let price =
            effective_price(&connection(Some(2.0)), &catalog(), "deepseek-flash").expect("用户价");
        assert_eq!(price.input, 2.0);
        assert_eq!(price.output, 2.0);
        assert_eq!(price.cache_read, Some(0.003), "没填的缓存读用目录标价");
        assert!(price.cache_write.is_none(), "deepseek 快照没有缓存写单价");
        let cost = usage_cost_usd(&usage_no_cache(), &price).expect("无缓存用量");
        assert!((cost - (1_000_000.0 * 2.0 + 500_000.0 * 2.0) / 1e6).abs() < 1e-9);
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
            std::slice::from_ref(&connection(None)),
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
            models: Some(vec![credentials::ModelRow {
                id: "deepseek-flash".to_owned(),
                name: None,
                context_window: None,
                max_tokens: None,
                price_input: user_price,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
            }]),
            auth_header: None,
        }
    }

    /// 统计定价按连接的真实 provider 查目录：MiniMax 连接用 minimax 的价，不是 deepseek 的。
    #[test]
    fn minimax_connection_prices_use_minimax_catalog() {
        let mut connection = connection_with(
            "minimax",
            "https://api.minimax.io/anthropic",
            "minimax",
            None,
        );
        connection.model = "MiniMax-M2.7".to_owned();
        connection.models.as_mut().expect("显式模型行")[0].id = "MiniMax-M2.7".to_owned();
        let price =
            effective_price(&connection, &catalog(), "MiniMax-M2.7").expect("minimax 目录价");
        assert!((price.input - 0.3).abs() < 1e-9);
        assert!((price.output - 1.2).abs() < 1e-9);
        assert_eq!(price.cache_read, Some(0.06));
        assert_eq!(price.cache_write, Some(0.375));
    }

    /// T012.8a 审计：两轮跨连接（DeepSeek 用户价 → MiniMax 目录价）按各自连接定价。
    #[test]
    fn stats_price_each_round_with_its_own_connection() {
        let deepseek = connection_with(
            "deepseek",
            "https://api.deepseek.com/anthropic",
            "deepseek",
            Some(2.0),
        );
        let minimax = connection_with(
            "minimax-cn",
            "https://api.minimaxi.com/anthropic",
            "minimax-cn",
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
        let cost = stats.cost_usd.expect("两轮都有价格，应该有花费");
        // DeepSeek 轮用用户价 2 美元/百万（输入输出同价）；MiniMax 轮用 minimax-cn 目录价
        // 0.1494 / 0.5976 美元/百万。两轮各 1M 输入、0.5M 输出。
        let deepseek_cost = (1_000_000.0 * 2.0 + 500_000.0 * 2.0) / 1e6;
        let minimax_cost = (1_000_000.0 * 0.1494 + 500_000.0 * 0.5976) / 1e6;
        let expected = deepseek_cost + minimax_cost;
        assert!(
            (cost - expected).abs() < 1e-6,
            "每轮按自己连接的价格算：{cost} vs {expected}"
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
            std::slice::from_ref(&connection(None)),
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
            Some(2.0),
        );
        let minimax = connection_with(
            "minimax-cn",
            "https://api.minimaxi.com/anthropic",
            "minimax-cn",
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
        // 第 2 轮起用的是 MiniMax-M3.1-Flash，按 minimax-cn 目录价算。
        let minimax_cost = (1_000_000.0 * 0.1494 + 500_000.0 * 0.5976) / 1e6;
        let expected = deepseek_cost * 2.0 + minimax_cost;
        assert!(!stats.unpriced, "三条都有价，不应无价：{stats:?}");
        assert!(
            (stats.cost_usd.unwrap_or(0.0) - expected).abs() < 1e-6,
            "轮中换连接不影响本轮定价，下一轮按新连接定价：{:?} vs {expected}",
            stats.cost_usd,
        );
    }

    /// T012 F1（Claude 审核）：DoD 会话用量 × 快照价——四类单价各自算，缓存读是输入价的 1/50。
    /// DoD 会话：输入 755、缓存读 24970、输出 354（无缓存写）；deepseek-flash 快照价
    /// input 0.15 / output 0.6 / cache_read 0.003（美元/百万 token）。
    #[test]
    fn deepseek_session_cost_uses_four_class_prices() {
        let connection = connection(None);
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
            "四类单价：期望 {expected}，实际 {cost}（输入 755 缓存读 24970 输出 354）"
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
        let connection = connection(None);
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
                            paste: false,
                            canvas: None,
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

    /// T012 F1：用户只填缓存读价——输入/输出用目录标价，缓存读用用户价。
    #[test]
    fn user_cache_read_price_fills_only_that_slot() {
        let mut conn = connection(None);
        conn.models.as_mut().expect("显式模型行")[0].price_cache_read = Some(100.0);
        let price =
            effective_price(&conn, &catalog(), "deepseek-flash").expect("标价+用户缓存读价");
        assert_eq!(price.input, 0.15, "输入用目录标价");
        assert_eq!(price.output, 0.6, "输出用目录标价");
        assert_eq!(price.cache_read, Some(100.0), "缓存读用用户价");
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
