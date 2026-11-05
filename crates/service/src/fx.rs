//! T012.8：汇率（USD → CNY）。照 Magpie `fx/fx.go:26,37,43`：open.er-api、12 小时缓存、
//! 拉不到用上一次、从没拉到过用 7.2。
//!
//! 缓存写 `<数据目录>/fx.json`（原子写，照 settings）；不用进程内全局状态，测试互不干扰。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 默认汇率来源（Magpie fx.go:26）。
pub const FX_URL: &str = "https://open.er-api.com/v6/latest/USD";
/// 从没拉到过时的后备汇率（spec：7.2）。
pub const FALLBACK_RATE: f64 = 7.2;
/// 缓存有效期：12 小时（spec）。
const CACHE_TTL_SECS: u64 = 12 * 60 * 60;
/// 缓存文件名（数据目录下）。
pub const FX_FILE: &str = "fx.json";

/// 汇率信息：1 USD = rate CNY；fetchedAt = 拉到的时间（Unix 秒，0 = 从没拉到过）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxInfo {
    pub rate: f64,
    #[serde(rename = "fetchedAt")]
    pub fetched_at: u64,
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FX_FILE)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn read_cache(data_dir: &Path) -> Option<FxInfo> {
    let bytes = std::fs::read(path(data_dir)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_cache(data_dir: &Path, info: &FxInfo) {
    use std::io::Write;
    let Ok(json) = serde_json::to_vec(info) else {
        return;
    };
    if std::fs::create_dir_all(data_dir).is_err() {
        return;
    }
    let target = path(data_dir);
    let tmp = target.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&json)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&tmp, &target)
    };
    let _ = write();
}

/// 现在用的汇率：12h 内用缓存；过期拉网络；拉不到用上一次；从没拉到过用 7.2。
pub async fn usd_to_cny(data_dir: &Path) -> FxInfo {
    usd_to_cny_with(data_dir, FX_URL).await
}

/// 同 `usd_to_cny`，但汇率来源可注入（测试用）。
pub async fn usd_to_cny_with(data_dir: &Path, url: &str) -> FxInfo {
    let now = now_secs();
    let cached = read_cache(data_dir);
    if let Some(cached) = &cached
        && now.saturating_sub(cached.fetched_at) < CACHE_TTL_SECS
    {
        return cached.clone();
    }
    if let Some(fresh) = fetch(url).await {
        write_cache(data_dir, &fresh);
        return fresh;
    }
    // 拉不到：用上一次；从没拉到过就 7.2（fetchedAt=0）。
    cached.unwrap_or(FxInfo {
        rate: FALLBACK_RATE,
        fetched_at: 0,
    })
}

async fn fetch(url: &str) -> Option<FxInfo> {
    let client = model::http::client().ok()?;
    let response = client
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    // service 的 reqwest 没开 json feature：自己读 bytes 解析（与 model 侧同法）。
    let bytes = response.bytes().await.ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let rate = value.pointer("/rates/CNY")?.as_f64()?;
    Some(FxInfo {
        rate,
        fetched_at: now_secs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn fetches_rate_and_caches_for_twelve_hours() {
        let server = MockServer::start().await;
        Mock::given(path("/latest/USD"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "rates": {"CNY": 7.3}
            })))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().expect("dir");
        let url = format!("{}/latest/USD", server.uri());
        let fresh = usd_to_cny_with(dir.path(), &url).await;
        assert_eq!(fresh.rate, 7.3);
        assert!(fresh.fetched_at > 0);
        assert!(dir.path().join(FX_FILE).exists(), "缓存已写");
        // 12h 内：坏 URL 也用缓存。
        let cached = usd_to_cny_with(dir.path(), "http://127.0.0.1:1/nope").await;
        assert_eq!(cached.rate, 7.3);
    }

    #[tokio::test]
    async fn stale_cache_falls_back_to_last_and_never_fetched_uses_seven_point_two() {
        let dir = tempfile::tempdir().expect("dir");
        // 从没拉到过 + 网络失败：7.2 / fetchedAt=0。
        let never = usd_to_cny_with(dir.path(), "http://127.0.0.1:1/nope").await;
        assert_eq!(never.rate, FALLBACK_RATE);
        assert_eq!(never.fetched_at, 0);
        // 有过缓存（13 小时前）+ 网络失败：用上一次。
        let stale = FxInfo {
            rate: 6.9,
            fetched_at: now_secs().saturating_sub(CACHE_TTL_SECS + 60),
        };
        write_cache(dir.path(), &stale);
        let fallback = usd_to_cny_with(dir.path(), "http://127.0.0.1:1/nope").await;
        assert_eq!(fallback.rate, 6.9);
    }
}
