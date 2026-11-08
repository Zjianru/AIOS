//! T012.8b：余额（DeepSeek）与套餐余量（MiniMax Coding Plan）。
//!
//! 形状来源：MiniMax 用真机探测样本（/tmp/t012-probe/q.json：
//! `model_remains[].current_interval_remaining_percent` / `current_weekly_remaining_percent`，
//! base_resp.status_code=0）；DeepSeek balance 按公开 API 形状
//! （`balance_infos[].currency` / `total_balance`），T012.10 真 key 轮校验。
//!
//! 查不到不报错：字段为空（spec 故事 5「查不到只是不显示」）。

use credentials::ConnectionInfo;
use serde::{Deserialize, Serialize};

/// 厂商币种的余额（原样用厂商给的币种）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceInfo {
    pub currency: String,
    pub amount: f64,
}

/// 套餐余量（MiniMax Coding Plan：5 小时窗口 / 每周的剩余百分比）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanQuota {
    /// 当前 5 小时窗口剩余百分比（0–100）。
    #[serde(
        rename = "intervalRemainingPercent",
        skip_serializing_if = "Option::is_none"
    )]
    pub interval_remaining_percent: Option<f64>,
    /// 每周剩余百分比（0–100）。
    #[serde(
        rename = "weeklyRemainingPercent",
        skip_serializing_if = "Option::is_none"
    )]
    pub weekly_remaining_percent: Option<f64>,
}

/// 一次查询的结果（哪一项查不到就为空）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Quota {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<BalanceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanQuota>,
}

/// 这个连接支持哪种查询（预设/地址判断；自定义地址不查）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    DeepSeekBalance,
    MiniMaxPlan,
}

fn kind_of(connection: &ConnectionInfo) -> Option<Kind> {
    // 预设优先；custom 只按已知厂商域名判断（relay 的 host 里带 minimax 字样不算）。
    match connection.preset.as_str() {
        "deepseek" => return Some(Kind::DeepSeekBalance),
        "minimax" | "minimax-cn" => return Some(Kind::MiniMaxPlan),
        _ => {}
    }
    let host = endpoint_host(&connection.endpoint)?;
    match host.as_str() {
        "api.deepseek.com" => Some(Kind::DeepSeekBalance),
        "api.minimax.io" | "api.minimaxi.com" | "api.minimax.cn" => Some(Kind::MiniMaxPlan),
        _ => None,
    }
}

/// 从 endpoint 里取主机名（小写；不引 url 依赖，够判断已知厂商域名）。
fn endpoint_host(endpoint: &str) -> Option<String> {
    let rest = endpoint.split("://").nth(1)?;
    let authority = rest.split('/').next()?;
    let authority = authority.rsplit('@').next()?;
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// 从 Anthropic base（…/anthropic）推厂商 API 根地址。
fn api_root(endpoint: &str) -> String {
    let base = endpoint.trim_end_matches('/');
    base.strip_suffix("/anthropic").unwrap_or(base).to_owned()
}

/// 查询余额/余量：只从环境请求一次（用共享 client，本机地址不走代理）；失败返回空。
pub async fn fetch(connection: &ConnectionInfo, key: &str) -> Quota {
    let Some(kind) = kind_of(connection) else {
        return Quota::default();
    };
    let Ok(client) = model::http::client() else {
        return Quota::default();
    };
    let root = api_root(&connection.endpoint);
    // T012.8b P2：连接上学到过单头（T012.6）就只带那个；没学过按各自默认。
    let learned = connection.auth_header.as_deref();
    let (url, request) = match kind {
        Kind::DeepSeekBalance => {
            let mut request = client
                .get(format!("{root}/user/balance"))
                .timeout(std::time::Duration::from_secs(5));
            request = match learned {
                Some("x-api-key") => request.header("x-api-key", key),
                _ => request.header("authorization", format!("Bearer {key}")),
            };
            (format!("{root}/user/balance"), request)
        }
        Kind::MiniMaxPlan => {
            let mut request = client
                .get(format!("{root}/v1/token_plan/remains"))
                .timeout(std::time::Duration::from_secs(5));
            request = match learned {
                Some("x-api-key") => request.header("x-api-key", key),
                Some("authorization") => request.header("authorization", format!("Bearer {key}")),
                _ => request
                    .header("x-api-key", key)
                    .header("authorization", format!("Bearer {key}")),
            };
            (format!("{root}/v1/token_plan/remains"), request)
        }
    };
    let _ = url;
    let Ok(response) = request.send().await else {
        return Quota::default();
    };
    if !response.status().is_success() {
        return Quota::default();
    }
    let Ok(bytes) = response.bytes().await else {
        return Quota::default();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Quota::default();
    };
    match kind {
        Kind::DeepSeekBalance => Quota {
            balance: parse_deepseek_balance(&value),
            plan: None,
        },
        Kind::MiniMaxPlan => Quota {
            balance: None,
            plan: parse_minimax_plan(&value),
        },
    }
}

/// DeepSeek：`{"balance_infos":[{"currency":"CNY","total_balance":"110.00"}]}`（取第一条）。
pub fn parse_deepseek_balance(value: &serde_json::Value) -> Option<BalanceInfo> {
    let item = value.get("balance_infos")?.as_array()?.first()?;
    let currency = item.get("currency")?.as_str()?.to_owned();
    let amount = match item.get("total_balance")? {
        serde_json::Value::String(text) => text.parse::<f64>().ok()?,
        serde_json::Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    Some(BalanceInfo { currency, amount })
}

/// MiniMax：`model_remains[]` 里优先 `model_name=general`，取两个剩余百分比。
pub fn parse_minimax_plan(value: &serde_json::Value) -> Option<PlanQuota> {
    let remains = value.get("model_remains")?.as_array()?;
    let item = remains
        .iter()
        .find(|item| item.get("model_name").and_then(|name| name.as_str()) == Some("general"))
        .or_else(|| remains.first())?;
    let interval = item
        .get("current_interval_remaining_percent")
        .and_then(serde_json::Value::as_f64);
    let weekly = item
        .get("current_weekly_remaining_percent")
        .and_then(serde_json::Value::as_f64);
    (interval.is_some() || weekly.is_some()).then_some(PlanQuota {
        interval_remaining_percent: interval,
        weekly_remaining_percent: weekly,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_balance_shape_from_public_api() {
        let value = serde_json::json!({
            "is_available": true,
            "balance_infos": [{"currency": "CNY", "total_balance": "110.00", "topped_up_balance": "100.00"}]
        });
        let balance = parse_deepseek_balance(&value).expect("balance");
        assert_eq!(balance.currency, "CNY");
        assert!((balance.amount - 110.0).abs() < 1e-9);
        assert!(parse_deepseek_balance(&serde_json::json!({})).is_none());
    }

    #[test]
    fn minimax_plan_shape_from_probe_sample() {
        // 真机探测样本（/tmp/t012-probe/q.json）的字段子集。
        let value = serde_json::json!({
            "model_remains": [
                {"model_name": "general", "current_interval_remaining_percent": 99, "current_weekly_remaining_percent": 95},
                {"model_name": "video", "current_interval_remaining_percent": 100, "current_weekly_remaining_percent": 100}
            ],
            "base_resp": {"status_code": 0, "status_msg": "success"}
        });
        let plan = parse_minimax_plan(&value).expect("plan");
        assert_eq!(plan.interval_remaining_percent, Some(99.0));
        assert_eq!(plan.weekly_remaining_percent, Some(95.0));
        assert!(parse_minimax_plan(&serde_json::json!({"model_remains": []})).is_none());
    }

    #[test]
    fn api_root_strips_anthropic_suffix() {
        assert_eq!(
            api_root("https://api.minimaxi.com/anthropic"),
            "https://api.minimaxi.com"
        );
        assert_eq!(
            api_root("https://api.deepseek.com/anthropic/"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            api_root("https://relay.example/api"),
            "https://relay.example/api"
        );
    }

    fn connection_info(id: &str, endpoint: &str) -> ConnectionInfo {
        ConnectionInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: endpoint.to_owned(),
            saved_at: 0,
            check: credentials::CheckState::Never,
            key_saved: true,
            preset: id.to_owned(),
            models: None,
            auth_header: None,
        }
    }

    /// T012.8b P2：custom 地址 host 里带 minimax 字样不算 MiniMax（按预设/已知厂商域名判断）。
    #[test]
    fn custom_relay_with_minimax_in_hostname_is_not_queried() {
        let connection = connection_info("custom", "https://relay.minimax-proxy.com/anthropic");
        assert!(
            kind_of(&connection).is_none(),
            "custom 地址不按 host 里的字样猜厂商"
        );
    }

    /// T012.8b P2：MiniMax 余量用连接上学到的单头（T012.6 的 auth_header）。
    #[tokio::test]
    async fn minimax_quota_uses_learned_auth_header() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/v1/token_plan/remains"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model_remains": [{"model_name": "general", "current_interval_remaining_percent": 99}],
                "base_resp": {"status_code": 0}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut connection = connection_info("minimax-cn", &format!("{}/anthropic", server.uri()));
        connection.auth_header = Some("authorization".to_owned());
        let quota = fetch(&connection, "test-key").await;
        assert!(quota.plan.is_some(), "余量解析成功");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].headers.contains_key("authorization"),
            "带学到的 authorization"
        );
        assert!(
            !requests[0].headers.contains_key("x-api-key"),
            "学到的单头时不带另一个头"
        );
    }
}
