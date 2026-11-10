//! T012.1：出站 HTTP 的唯一出处。
//!
//! 所有请求（模型、验证、目录、余额、余量、汇率）都用这里造的 client；socket 级细节不散落在业务里。
//! 环境里的代理只在这一处读；本机地址（localhost、127.0.0.0/8、::1）不走代理（故事 9）。
//! 网页抓取判断走不走代理也调 `HttpEnv::proxy_for`，和 client 用同一条规则。
//! 代理地址解析失败直接报错，不把「坏地址」当成「没配代理」而静默直连。

use std::sync::OnceLock;

use reqwest::redirect::Policy;
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

    /// 这条 URL 该走哪个代理：本机地址和 `no_proxy` 命中直连，其余按协议挑。
    ///
    /// 和 client 的 `Proxy::custom` 共用 `ProxySelection::for_url`。代理地址解析失败要报错，
    /// 不能当成「没配代理」而静默直连。
    pub fn proxy_for(&self, url: &Url) -> Result<Option<Url>, ModelError> {
        Ok(ProxySelection::parse(self)?.for_url(url))
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

/// 解析一个代理变量。解析失败要报错（指名哪个变量），不能当成「没配代理」——那会静默直连。
/// 报错只带变量名和解析原因，不带原值：代理地址里可能有密码。
fn proxy_url(name: &str, raw: Option<&str>) -> Result<Option<Url>, ModelError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let text = raw.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let with_scheme = if text.contains("://") {
        text.to_owned()
    } else {
        format!("http://{text}")
    };
    Url::parse(&with_scheme).map(Some).map_err(|error| {
        ModelError::new(
            ErrorCode::Transport,
            format!("代理地址解析失败（{name}）：{error}"),
        )
    })
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

/// 重定向策略：跟随（模型请求）或不跟（带工具凭证的网页请求，重定向由调用方决定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redirects {
    Follow,
    Never,
}

fn build_with_env(env: HttpEnv) -> Result<Client, ModelError> {
    build(env, Redirects::Follow)
}

fn build_without_redirects(env: HttpEnv) -> Result<Client, ModelError> {
    build(env, Redirects::Never)
}

/// 已经解析好的代理设置：client 建一次，web_fetch 按 URL 选一次。
struct ProxySelection {
    http: Option<Url>,
    https: Option<Url>,
    all: Option<Url>,
    no_proxy: Option<String>,
}

impl ProxySelection {
    fn parse(env: &HttpEnv) -> Result<Self, ModelError> {
        Ok(Self {
            http: proxy_url("http_proxy", env.http_proxy.as_deref())?,
            https: proxy_url("https_proxy", env.https_proxy.as_deref())?,
            all: proxy_url("all_proxy", env.all_proxy.as_deref())?,
            no_proxy: env.no_proxy.clone(),
        })
    }

    fn for_url(&self, url: &Url) -> Option<Url> {
        if is_loopback(url) {
            return None;
        }
        if no_proxy_matches(url, self.no_proxy.as_deref()) {
            return None;
        }
        match url.scheme() {
            "https" => self.https.clone().or_else(|| self.all.clone()),
            "http" => self.http.clone().or_else(|| self.all.clone()),
            _ => None,
        }
    }
}

fn build(env: HttpEnv, redirects: Redirects) -> Result<Client, ModelError> {
    let proxies = ProxySelection::parse(&env)?;
    let mut builder = Client::builder()
        // 代理只从 HttpEnv 来，避免 reqwest 自己再读一遍环境、绕开本机判断。
        .no_proxy()
        .proxy(Proxy::custom(move |url: &Url| proxies.for_url(url)));
    if redirects == Redirects::Never {
        builder = builder.redirect(Policy::none());
    }
    builder.build().map_err(|error| {
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

/// 用给定环境造一个不跟重定向的 client；网页抓取要自己按跳检查地址和同源。
pub fn client_with_env_without_redirects(env: HttpEnv) -> Result<Client, ModelError> {
    build_without_redirects(env)
}

/// 进程内唯一的出站 client（按进程环境读一次代理配置）。
pub fn client() -> Result<Client, ModelError> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    cached_client(&CLIENT, || client_with_env(HttpEnv::from_process()))
}

/// 出站 client，但不跟重定向：带工具凭证的请求（Exa 验证、网页搜索）把重定向留给调用方判断。
///
/// 代理与本机地址的规则和 client() 相同；reqwest 的重定向策略建在 client 上，
/// 不能按请求覆盖，所以另缓存一份。
pub fn client_without_redirects() -> Result<Client, ModelError> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    cached_client(&CLIENT, || build_without_redirects(HttpEnv::from_process()))
}

fn cached_client(
    cell: &'static OnceLock<Result<Client, String>>,
    build: impl FnOnce() -> Result<Client, ModelError>,
) -> Result<Client, ModelError> {
    let client = cell.get_or_init(|| build().map_err(|error| error.message));
    match client {
        Ok(client) => Ok(client.clone()),
        Err(message) => Err(ModelError::new(
            ErrorCode::Transport,
            format!("构造出站 HTTP client 失败: {message}"),
        )),
    }
}

#[cfg(test)]
mod redirect_tests {
    use super::client_without_redirects;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 带工具凭证的请求不吃重定向：302 原样回到调用方，钥匙不会被带到别处。
    #[tokio::test]
    async fn client_without_redirects_returns_the_redirect() {
        let server = MockServer::start().await;
        Mock::given(path("/search"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://elsewhere.example/"),
            )
            .mount(&server)
            .await;
        let client = client_without_redirects().expect("出站 client");
        let response = client
            .post(format!("{}/search", server.uri()))
            .send()
            .await
            .expect("请求");
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    }
}
