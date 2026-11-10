//! 直连时钉住 DNS：解析和地址分类在同一处，全是公网地址才交给 reqwest 连接。
//!
//! reqwest 只为域名调用解析器；URL 里直接写 IP 的请求由 `fetch` 在发请求前判。
//! 解析器拿不到端口，也不判 AIOS 自己：AIOS 只监听回环地址，回环已由分类器拒绝。

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use super::url_policy::{BlockedAddress, FetchError, classify_address};

/// 一次域名查询的结果：只留 IP，端口由 reqwest 按 URL 补。
type LookupFuture = Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send>>;
type Lookup = Arc<dyn Fn(String) -> LookupFuture + Send + Sync>;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// 直连的域名解析器：查一次、逐个分类，全过才把地址交给 reqwest 连接。
#[derive(Clone)]
pub(super) struct PublicResolver {
    lookup: Lookup,
}

impl PublicResolver {
    pub(super) fn new() -> Self {
        Self {
            lookup: Arc::new(system_lookup),
        }
    }

    /// 测试里换成假查询；分类照做。
    #[cfg(test)]
    pub(super) fn with_lookup<F, Fut>(lookup: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'static,
    {
        Self {
            lookup: Arc::new(move |host| Box::pin(lookup(host))),
        }
    }
}

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let lookup = Arc::clone(&self.lookup);
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses = lookup(host)
                .await
                .map_err(|error| Box::new(ResolveFailure::Lookup(error)) as BoxError)?;
            if addresses.is_empty() {
                return Err(Box::new(ResolveFailure::Empty) as BoxError);
            }
            let mut resolved = Vec::with_capacity(addresses.len());
            for address in addresses {
                if let Some(blocked) = classify_address(address) {
                    return Err(Box::new(ResolveFailure::Blocked(blocked)) as BoxError);
                }
                resolved.push(SocketAddr::new(address, 0));
            }
            Ok(Box::new(resolved.into_iter()) as Addrs)
        })
    }
}

fn system_lookup(host: String) -> LookupFuture {
    Box::pin(async move {
        // 分类只看 IP，端口给 0；reqwest 会按 URL 的协议和端口补上。
        Ok(tokio::net::lookup_host((host.as_str(), 0))
            .await?
            .map(|address| address.ip())
            .collect())
    })
}

/// 解析器交给 reqwest 的失败：查不出来，或者查到的地址被分类拦下。
#[derive(Debug)]
pub(super) enum ResolveFailure {
    Lookup(std::io::Error),
    Empty,
    Blocked(BlockedAddress),
}

impl ResolveFailure {
    pub(super) fn to_fetch_error(&self) -> FetchError {
        match self {
            Self::Lookup(_) | Self::Empty => FetchError::Resolve,
            Self::Blocked(blocked) => FetchError::Blocked(*blocked),
        }
    }
}

impl std::fmt::Display for ResolveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lookup(error) => write!(f, "链接的域名解析不出来：{error}"),
            Self::Empty => f.write_str("链接的域名解析不出来"),
            Self::Blocked(blocked) => f.write_str(blocked.message()),
        }
    }
}

impl std::error::Error for ResolveFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lookup(error) => Some(error),
            Self::Empty | Self::Blocked(_) => None,
        }
    }
}
