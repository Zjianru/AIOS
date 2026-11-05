//! T012.6：适配器在内存里学到的连接事实（按 连接 × 模型 × 协议；协议只有 Messages 一种）。
//!
//! - 关不掉思考 / 必须思考：厂商 400 后学到，本轮按最低非关档重发、后续请求直接最低档；
//! - 鉴权头：两个头一起被拒后学到的单头。持久化（写回连接的 `auth_header`）由
//!   `CredentialSource::remember_auth_header` 往上回报，适配器不直接读 Keyring。
//!
//! 存在内存里，重启后再学一次（spec「学到的东西」；Magpie `gateway.go` 的 fits/markUnfit 同形）。

// T012.6 骨架：类型与接缝先落，接线（stream_inner 重发/鉴权头轮换）在实现提交里；
// 骨架阶段这些 helper 还没有调用方，先允许 dead_code，实现后删掉这行。
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// 鉴权头取值；与 `ConnectionInfo.auth_header` 的约定一致。
pub(crate) const AUTH_X_API_KEY: &str = "x-api-key";
pub(crate) const AUTH_AUTHORIZATION: &str = "authorization";

/// 一次请求带哪些鉴权头（Magpie `provider.AuthHeader`：默认两个都发）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthChoice {
    /// 默认：x-api-key + Authorization: Bearer 都发。
    Both,
    XApiKey,
    Authorization,
}

impl AuthChoice {
    pub(crate) fn from_learned(header: &str) -> Self {
        match header {
            AUTH_AUTHORIZATION => Self::Authorization,
            AUTH_X_API_KEY => Self::XApiKey,
            _ => Self::Both,
        }
    }

    /// 对应的 `auth_header` 字符串（回报产品层用）。
    pub(crate) fn learned_name(self) -> Option<&'static str> {
        match self {
            Self::Both => None,
            Self::XApiKey => Some(AUTH_X_API_KEY),
            Self::Authorization => Some(AUTH_AUTHORIZATION),
        }
    }
}

/// 适配器实例共享的「学到的东西」。
#[derive(Debug, Default)]
pub(crate) struct Learned {
    /// (连接 base_url, 模型)：厂商拒绝关掉思考 / 要求必须思考。
    thinking_required: Mutex<BTreeSet<(String, String)>>,
    /// 连接 base_url：只发某一个鉴权头（学到的）。
    auth_header: Mutex<BTreeMap<String, String>>,
}

impl Learned {
    pub(crate) fn thinking_required(&self, base_url: &str, model: &str) -> bool {
        self.thinking_required
            .lock()
            .map(|set| set.contains(&(base_url.to_owned(), model.to_owned())))
            .unwrap_or(false)
    }

    pub(crate) fn learn_thinking_required(&self, base_url: &str, model: &str) {
        if let Ok(mut set) = self.thinking_required.lock() {
            set.insert((base_url.to_owned(), model.to_owned()));
        }
    }

    pub(crate) fn auth_header(&self, base_url: &str) -> Option<String> {
        self.auth_header
            .lock()
            .ok()
            .and_then(|headers| headers.get(base_url).cloned())
    }

    pub(crate) fn learn_auth_header(&self, base_url: &str, header: &str) {
        if let Ok(mut headers) = self.auth_header.lock() {
            headers.insert(base_url.to_owned(), header.to_owned());
        }
    }
}
