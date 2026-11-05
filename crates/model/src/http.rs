//! T012.1：出站 HTTP 的唯一出处。
//!
//! 所有请求（模型、验证、目录、余额、余量、汇率）都用这里造的 client；socket 级细节不散落在业务里。
//! 环境里的代理只在这一处读；本机地址（localhost、127.0.0.0/8、::1）不走代理（故事 9）。

use std::sync::OnceLock;

use reqwest::{Client, Proxy, Url};

use crate::{ErrorCode, ModelError};

/// 出站 HTTP 读取的环境（测试从上层注入，不改进程环境变量）。
#[derive(Debug, Clone, Default)]
pub struct HttpEnv {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub all_proxy: Option<String>,
    pub no_proxy: Option<String>,
}

impl HttpEnv {
    /// 从进程环境读一次（生产入口）。
    pub fn from_process() -> Self {
        Self::from_source(|key| std::env::var(key).ok())
    }

    /// 从给定来源读（测试注入用；键名与真实环境变量一致）。
    pub fn from_source(source: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            http_proxy: first_non_empty(&source, &["http_proxy", "HTTP_PROXY"]),
            https_proxy: first_non_empty(&source, &["https_proxy", "HTTPS_PROXY"]),
            all_proxy: first_non_empty(&source, &["all_proxy", "ALL_PROXY"]),
            no_proxy: first_non_empty(&source, &["no_proxy", "NO_PROXY"]),
        }
    }
}

fn first_non_empty(source: &impl Fn(&str) -> Option<String>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        source(name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn proxy_url(raw: &str) -> Option<Url> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    let with_scheme = if text.contains("://") {
        text.to_owned()
    } else {
        format!("http://{text}")
    };
    Url::parse(&with_scheme).ok()
}

/// 本机地址按主机名/IP 判：localhost、127.0.0.0/8、::1（故事 9）。
fn is_loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    // url 2.x 对 IPv6 的 host_str 带方括号（"[::1]"），先去掉再判。
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(address)) => address.is_loopback(),
        Ok(std::net::IpAddr::V6(address)) => {
            address.is_loopback()
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|address| address.is_loopback())
        }
        Err(_) => false,
    }
}

fn no_proxy_matches(url: &Url, no_proxy: Option<&str>) -> bool {
    let Some(no_proxy) = no_proxy else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    no_proxy.split(',').any(|entry| {
        let entry = entry.trim().trim_start_matches('.');
        if entry.is_empty() {
            return false;
        }
        if entry == "*" {
            return true;
        }
        host.eq_ignore_ascii_case(entry)
            || host
                .to_ascii_lowercase()
                .ends_with(&format!(".{}", entry.to_ascii_lowercase()))
    })
}

fn build_with_env(env: HttpEnv) -> Result<Client, ModelError> {
    let http = env.http_proxy.as_deref().and_then(proxy_url);
    let https = env.https_proxy.as_deref().and_then(proxy_url);
    let all = env.all_proxy.as_deref().and_then(proxy_url);
    let no_proxy = env.no_proxy.clone();
    Client::builder()
        // 代理只从 HttpEnv 来，避免 reqwest 自己再读一遍环境、绕开本机判断。
        .no_proxy()
        .proxy(Proxy::custom(move |url: &Url| {
            if is_loopback(url) {
                return None;
            }
            if no_proxy_matches(url, no_proxy.as_deref()) {
                return None;
            }
            match url.scheme() {
                "https" => https.clone().or_else(|| all.clone()),
                "http" => http.clone().or_else(|| all.clone()),
                _ => None,
            }
        }))
        .build()
        .map_err(|error| {
            ModelError::new(
                ErrorCode::Transport,
                format!("构造出站 HTTP client 失败: {error}"),
            )
        })
}

/// 用给定环境造一个 client（测试/未来需要显式环境的调用方）。
pub fn client_with_env(env: HttpEnv) -> Result<Client, ModelError> {
    build_with_env(env)
}

/// 进程内唯一的出站 client（按进程环境读一次代理配置）。
pub fn client() -> Result<Client, ModelError> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    let client = CLIENT
        .get_or_init(|| client_with_env(HttpEnv::from_process()).map_err(|error| error.message));
    match client {
        Ok(client) => Ok(client.clone()),
        Err(message) => Err(ModelError::new(
            ErrorCode::Transport,
            format!("构造出站 HTTP client 失败: {message}"),
        )),
    }
}
