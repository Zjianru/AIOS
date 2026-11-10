//! 读网页的 URL 与地址策略：协议/长度/凭证、解析后的地址分类、同源重定向规则。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use reqwest::Url;
use tokio::net::lookup_host;
use tools::ToolFailure;

/// 读网页的 URL 上限（照 DSH）。
const WEB_FETCH_MAX_URL_CHARS: usize = 2048;
/// 同源重定向最多跟 5 跳。
pub(super) const WEB_FETCH_MAX_REDIRECTS: u8 = 5;
/// 只看 URL 本身：长度、协议、账号密码、主机。
pub(super) fn parse_fetch_url(input: &str) -> Result<Url, FetchError> {
    if input.chars().count() > WEB_FETCH_MAX_URL_CHARS {
        return Err(FetchError::TooLong);
    }
    let url = Url::parse(input).map_err(|_| FetchError::InvalidUrl)?;
    validate_parsed_url(&url)?;
    Ok(url)
}

pub(super) fn validate_parsed_url(url: &Url) -> Result<(), FetchError> {
    if url.as_str().chars().count() > WEB_FETCH_MAX_URL_CHARS {
        return Err(FetchError::TooLong);
    }
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(FetchError::Scheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::Credentials);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(FetchError::InvalidUrl);
    }
    Ok(())
}

/// 解析目标主机，回环、内网、链路本地、未指定地址和 AIOS 自己的监听地址都拒绝。
pub(super) async fn check_fetch_address(
    url: &Url,
    self_addr: Option<&std::sync::OnceLock<SocketAddr>>,
) -> Result<(), FetchError> {
    let port = url.port_or_known_default().ok_or(FetchError::InvalidUrl)?;
    let self_addr = self_addr.and_then(|cell| cell.get().copied());
    let addresses = resolve_addresses(url, port).await?;
    for address in addresses {
        if let Some(blocked) = classify_address(address) {
            return Err(FetchError::Blocked(blocked));
        }
        if is_self_address(address, port, self_addr) {
            return Err(FetchError::SelfAddress);
        }
    }
    Ok(())
}

/// 域名解析成所有候选地址；解析不出来要报错，不静默跳过。
async fn resolve_addresses(url: &Url, port: u16) -> Result<Vec<IpAddr>, FetchError> {
    let host = url.host_str().ok_or(FetchError::InvalidUrl)?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(address) = host.parse::<IpAddr>() {
        return Ok(vec![address]);
    }
    let mut addresses = Vec::new();
    for resolved in lookup_host((host, port))
        .await
        .map_err(|_| FetchError::Resolve)?
    {
        addresses.push(resolved.ip());
    }
    if addresses.is_empty() {
        return Err(FetchError::Resolve);
    }
    Ok(addresses)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockedAddress {
    Loopback,
    Private,
    LinkLocal,
    Unspecified,
}

impl BlockedAddress {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Loopback => "这个链接指向本机地址，不能读",
            Self::Private => "这个链接指向内网地址，不能读",
            Self::LinkLocal => "这个链接指向链路本地地址，不能读",
            Self::Unspecified => "这个链接指向未指定地址，不能读",
        }
    }
}

pub(super) fn classify_address(address: IpAddr) -> Option<BlockedAddress> {
    match address {
        IpAddr::V4(address) => classify_v4(address),
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return classify_v4(mapped);
            }
            if address.is_loopback() {
                Some(BlockedAddress::Loopback)
            } else if address.is_unique_local() {
                Some(BlockedAddress::Private)
            } else if address.is_unicast_link_local() {
                Some(BlockedAddress::LinkLocal)
            } else if address.is_unspecified() {
                Some(BlockedAddress::Unspecified)
            } else {
                None
            }
        }
    }
}

fn classify_v4(address: Ipv4Addr) -> Option<BlockedAddress> {
    if address.is_loopback() {
        Some(BlockedAddress::Loopback)
    } else if address.is_private() {
        Some(BlockedAddress::Private)
    } else if address.is_link_local() {
        Some(BlockedAddress::LinkLocal)
    } else if address.is_unspecified() {
        Some(BlockedAddress::Unspecified)
    } else {
        None
    }
}

fn canonical_address(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        IpAddr::V4(address) => IpAddr::V4(address),
    }
}

fn is_self_address(address: IpAddr, port: u16, self_addr: Option<SocketAddr>) -> bool {
    self_addr.is_some_and(|self_addr| {
        self_addr.port() == port && canonical_address(address) == canonical_address(self_addr.ip())
    })
}

/// 只跟真正的重定向状态；304 之类不是跳转。
pub(super) fn is_followable_redirect(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// 同源 = 协议、主机、端口都相同（默认端口按协议算）。
pub(super) fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// 读网页失败的原因；文案里不带响应正文或凭证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FetchError {
    InvalidUrl,
    Scheme,
    Credentials,
    TooLong,
    Blocked(BlockedAddress),
    SelfAddress,
    Resolve,
    Client,
    Timeout,
    Connect,
    Transport,
    TooManyRedirects,
    CrossOrigin(String),
    UnsupportedContentType(String),
    Convert,
}

impl FetchError {
    pub(super) fn into_failure(self) -> ToolFailure {
        match self {
            Self::InvalidUrl => ToolFailure::plain("这个链接看不懂，不是合法的 URL"),
            Self::Scheme => ToolFailure::plain("web_fetch 只读 http/https 链接"),
            Self::Credentials => ToolFailure::plain("链接里不能带账号密码"),
            Self::TooLong => {
                ToolFailure::plain(format!("链接太长了（最多 {WEB_FETCH_MAX_URL_CHARS} 字）"))
            }
            Self::Blocked(blocked) => ToolFailure::plain(blocked.message()),
            Self::SelfAddress => {
                ToolFailure::plain("这个链接指向 AIOS 自己监听的地址和端口，不能读")
            }
            Self::Resolve => ToolFailure::plain("链接的域名解析不出来"),
            Self::Client => ToolFailure::plain("建不出读网页的 HTTP 客户端；稍后再试"),
            Self::Timeout => ToolFailure::plain("读网页超时了（30 秒）；稍后再试"),
            Self::Connect => ToolFailure::plain("连不上这个链接；检查网络后再试"),
            Self::Transport => ToolFailure::plain("读网页时网络出错了；稍后再试"),
            Self::TooManyRedirects => ToolFailure::plain("重定向次数太多（超过 5 次），没有读"),
            Self::CrossOrigin(target) => ToolFailure::plain(format!(
                "这个链接重定向到了另一个源（{target}），不能跟；请直接用那个链接再读一次"
            )),
            Self::UnsupportedContentType(content_type) => ToolFailure::plain(format!(
                "这个链接返回的内容类型不支持（{content_type}）；只能读 HTML 或文本"
            )),
            Self::Convert => ToolFailure::plain("这个网页的 HTML 转不成 Markdown"),
        }
    }
}
