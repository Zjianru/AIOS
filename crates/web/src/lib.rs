//! T006.17：Web 薄适配器（axum HTTP + WebSocket）与信任栅栏（接缝 4）。
//!
//! 载体无关的宿主在 crates/service；这里只做线上形状：
//! - HTTP：POST /api，body 是 service::Call 的 JSON，回 service::Response（或错误 JSON）；/api 只认 cookie；
//! - WebSocket：GET /ws?lastSeq=..（只认 cookie），先发 ready，再按 lastSeq 补齐事件，然后发 live 事件；
//! - 信任栅栏照 DSH（client/connection README 第 41、47 行；host/webserver README 第 39 行）：
//!   只绑本机回环地址（0.0.0.0/非回环一律拒绝启动）、每个进程一个启动令牌（只在 / 与 /connect 换签名 cookie）、Host 头只认本机
//!   （回环地址/localhost/绑定 IP 字面量）；失败 403（Host 不对）/401（无令牌或令牌不对）。
//!
//! DSH 对照：ready 项先于事件（connection README「Connection generation」）；重连带最后序号
//! 重新跟随（session-controller follow :607，AIOS 用 T006.16b 的 follow(lastSeq)）。

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::header::{CONTENT_TYPE, HOST, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::sync::Mutex;

use monitor::{Channel, Monitor, Record, Severity};
use serde::{Deserialize, Serialize};
use service::{
    ContextUsage, FollowRequest, FollowResponse, HostInfo, HostLocation, PROTOCOL_VERSION,
    RootHost, RootState, ServiceError, ServiceEvent,
};

/// 默认只绑 IPv4 回环。
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// 默认端口 0：由系统分配（测试/开发都从 ServeHandle 读实际端口）。
pub const DEFAULT_PORT: u16 = 0;

/// 信任栅栏的配置：绑定地址 + 本进程启动令牌 + cookie 签名钥匙。
#[derive(Clone)]
pub struct Trust {
    pub bind_ip: IpAddr,
    pub bind_port: u16,
    pub token: String,
    /// 每次启动从 Keyring 读入的 cookie 签名钥匙；只放内存，不落盘、不进日志。
    session_secret: [u8; 32],
}

impl std::fmt::Debug for Trust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trust")
            .field("bind_ip", &self.bind_ip)
            .field("bind_port", &self.bind_port)
            .field("token", &"<redacted>")
            .field("session_secret", &"<redacted>")
            .finish()
    }
}

impl Trust {
    pub fn new(bind_ip: IpAddr, bind_port: u16, token: String, session_secret: [u8; 32]) -> Self {
        Self {
            bind_ip,
            bind_port,
            token,
            session_secret,
        }
    }
}

/// T006 r3 #3：401 正文（照来源 writeUnauthorized 的意思）：没登录或登录失效时，
/// 重新打开 aios-dev serve 打印的地址；不出现来源名字，也不再说「令牌」（cookie 是唯一认证方式）。
const UNAUTHORIZED_BODY: &str = "还没有登录或登录已失效：请重新打开 aios-dev serve 打印的地址。";

/// 信任栅栏的拒绝原因；HTTP 面上 Host 不对 403、未登录/cookie 不对 401。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TrustError {
    #[error("不支持的绑定地址：{0}（只允许本机回环地址，拒绝 0.0.0.0/非回环）")]
    UnsupportedBind(String),
    #[error("Host 头不是本机：{0}")]
    BadHost(String),
    #[error("Origin/Fetch-Metadata 不是同源：{0}")]
    BadOrigin(String),
    #[error("{}", UNAUTHORIZED_BODY)]
    Unauthorized,
}

impl TrustError {
    /// 线上的状态码：Host/绑定信任失败 403，令牌失败 401。
    pub fn status(&self) -> u16 {
        match self {
            Self::BadHost(_) | Self::BadOrigin(_) | Self::UnsupportedBind(_) => 403,
            Self::Unauthorized => 401,
        }
    }
}

/// serve 的错误：绑定/监听失败或配置被信任栅栏拒绝。
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(transparent)]
    Trust(#[from] TrustError),
    #[error("监听失败：{0}")]
    Io(std::io::Error),
}

impl From<std::io::Error> for ServeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// serve 的输入（CLI 参数映射成普通数据；令牌由调用方生成，不读环境变量）。
#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub host: String,
    pub port: u16,
    pub token: String,
    /// 每次启动从 Keyring 读入的 cookie 签名钥匙（T008.F11）。
    pub session_secret: [u8; 32],
    /// 同源托管的 web 构建产物目录（T007.2，通常是仓库的 web/dist）；None 时不托管静态文件。
    pub static_dir: Option<PathBuf>,
    /// T011.4：本进程的监控记录器（None 时不记）。
    pub monitor: Option<Monitor>,
}

/// 起好服务后拿到的句柄（端口可能是系统分配的，测试从 addr 读）。
pub struct ServeHandle {
    pub addr: SocketAddr,
    pub token: String,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
    /// T006.18：优雅关闭信号；服务任务在 axum 停止收连接后取消正在跑的一轮再收尾。
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ServeHandle {
    /// 停服务（测试/CLI Ctrl-C）：先停收新连接，再取消正在跑的一轮、等日志收尾。
    pub async fn shutdown(mut self) {
        if let Some(signal) = self.shutdown.take() {
            let _ = signal.send(());
        }
        let _ = self.task.await;
    }

    /// 立刻停（测试用；不走取消收尾）。
    pub fn abort(self) {
        self.task.abort();
    }
}

/// 线上帧：ready 必须先于事件；事件帧包着 service 的日志事件（seq/type/data 来自日志事实）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Frame {
    /// 连接建立后的第一帧（DSH 的 {type:'ready',...}）；T007.2 起带协议版本、根会话状态、上下文占用。
    Ready {
        #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "protocolVersion")]
        protocol_version: u64,
        root: RootState,
        context: ContextUsage,
        /// T009.5：host: {home}（页面用来缩写路径）。
        host: HostLocation,
    },
    /// 日志事件帧（DSH 的 {type:'event', event: SessionWireEvent}）。
    Event { event: ServiceEvent },
    /// T007.4：不落日志的 AgentEvent（流式文本/用量/重试提示等），页面单独消费。
    Agent { event: AgentEventWire },
}

/// AgentEvent 的线上形状（只挑页面要的；只增不改）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentEventWire {
    TextDelta {
        text: String,
    },
    Usage {
        #[serde(
            rename = "inputTokens",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        input_tokens: Option<u64>,
        #[serde(
            rename = "outputTokens",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        output_tokens: Option<u64>,
        #[serde(
            rename = "cacheReadTokens",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        cache_read_tokens: Option<u64>,
        #[serde(
            rename = "cacheWriteTokens",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        cache_write_tokens: Option<u64>,
    },
    Retrying {
        attempt: u32,
        #[serde(rename = "maxAttempts")]
        max_attempts: u32,
        #[serde(rename = "delayMs")]
        delay_ms: u64,
        code: String,
    },
}

impl AgentEventWire {
    pub fn from_event(event: &agent::AgentEvent) -> Option<Self> {
        match event {
            agent::AgentEvent::AssistantTextDelta { text } => {
                Some(Self::TextDelta { text: text.clone() })
            }
            agent::AgentEvent::UsageReported { usage } => Some(Self::Usage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_input_tokens,
                cache_write_tokens: usage.cache_creation_input_tokens,
            }),
            agent::AgentEvent::Retrying {
                attempt,
                max_attempts,
                delay_ms,
                code,
            } => Some(Self::Retrying {
                attempt: *attempt,
                max_attempts: *max_attempts,
                delay_ms: *delay_ms,
                code: code.clone(),
            }),
            _ => None,
        }
    }
}

/// 本进程的启动令牌（每个进程一个；uuid v4，不落盘、不进日志）。
pub fn launch_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 绑定地址只允许本机回环：0.0.0.0/::/非回环字面量都拒绝启动（DSH webserver README 第 39 行）。
pub fn check_bind_host(host: &str) -> Result<IpAddr, TrustError> {
    let ip: IpAddr = host
        .trim()
        .parse()
        .map_err(|_| TrustError::UnsupportedBind(host.to_owned()))?;
    if ip.is_loopback() {
        Ok(ip)
    } else {
        Err(TrustError::UnsupportedBind(host.to_owned()))
    }
}

/// 从 Host 头里取主机部分（IPv6 字面量带方括号；其余按最后一个冒号拆端口）。
fn host_part(host_header: &str) -> &str {
    let header = host_header.trim();
    if let Some(rest) = header.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        return &rest[..end];
    }
    header
        .rsplit_once(':')
        .map(|(host, _port)| host)
        .unwrap_or(header)
}

/// Host 头是不是本机：localhost、任意回环地址、或等于监听绑定的 IP 字面量。
fn authority_is_loopback(host_header: &str, bind_ip: IpAddr) -> bool {
    let part = host_part(host_header);
    if part.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match part.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback() || ip == bind_ip,
        Err(_) => false,
    }
}

/// 定长比较令牌（不提前返回，避免长度/前缀时序侧信道）。
fn token_matches(expected: &str, provided: &str) -> bool {
    let expected = expected.as_bytes();
    let provided = provided.as_bytes();
    let mut diff = expected.len() ^ provided.len();
    for index in 0..expected.len().max(provided.len()) {
        diff |= usize::from(
            expected.get(index).copied().unwrap_or(0) ^ provided.get(index).copied().unwrap_or(0),
        );
    }
    diff == 0
}

/// 解析裸 authority（无 scheme 的 host[:port]，IPv6 用方括号）；返回小写主机与端口。
fn parse_authority(raw: &str) -> Option<(String, Option<u16>)> {
    let uri: axum::http::Uri = format!("http://{raw}").parse().ok()?;
    let host = uri
        .host()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host.is_empty() || host.contains('[') || host.contains(']') {
        return None;
    }
    Some((host, uri.port_u16()))
}

/// Host/Origin 同源比较：http(s) scheme + 规范化 hostname + 端口（缺省按 scheme 归一）。
fn origin_is_same_authority(origin: &str, host: &str) -> bool {
    let origin_uri: axum::http::Uri = match origin.parse() {
        Ok(uri) => uri,
        Err(_) => return false,
    };
    let scheme = match origin_uri.scheme_str() {
        Some("http") => "http",
        Some("https") => "https",
        _ => return false,
    };
    let Some(origin_host) = origin_uri.host() else {
        return false;
    };
    let origin_host = origin_host.to_ascii_lowercase();
    let Some((host_name, host_port)) = parse_authority(host) else {
        return false;
    };
    let default_port = if scheme == "https" { 443 } else { 80 };
    let origin_port = origin_uri.port_u16().unwrap_or(default_port);
    let host_port = host_port.unwrap_or(default_port);
    origin_host == host_name && origin_port == host_port
}

/// T006.F2：/connect 换 cookie 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectGrant {
    pub set_cookie: String,
    pub location: String,
}

const COOKIE_PREFIX: &str = "aios-auth-";
const COOKIE_MAX_AGE_SECS: i64 = 30 * 24 * 60 * 60;

#[derive(Debug, Serialize, Deserialize)]
struct CookiePayload {
    v: u8,
    authority: String,
    #[serde(rename = "issuedAt")]
    issued_at: i64,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let high = (bytes[index] as char).to_digit(16)?;
        let low = (bytes[index + 1] as char).to_digit(16)?;
        out.push(u8::try_from((high << 4) | low).ok()?);
        index += 2;
    }
    Some(out)
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, data);
    hex_encode(digest.as_ref())
}

fn hmac_sign(secret: &[u8; 32], body: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret);
    hex_encode(ring::hmac::sign(&key, body.as_bytes()).as_ref())
}

fn hmac_verify(secret: &[u8; 32], body: &str, signature: &str) -> bool {
    let Some(signature) = hex_decode(signature) else {
        return false;
    };
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret);
    ring::hmac::verify(&key, body.as_bytes(), &signature).is_ok()
}

/// Host 头归一成 cookie authority：小写 hostname + 端口，HTTP 缺省端口 80。
fn authority_of(host_header: &str) -> Option<String> {
    let (host, port) = parse_authority(host_header)?;
    Some(format!("{host}:{}", port.unwrap_or(80)))
}

fn cookie_name(authority: &str) -> String {
    let hash = sha256_hex(authority.as_bytes());
    format!("{COOKIE_PREFIX}{}", &hash[..16])
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Unix 秒的 HTTP-date（RFC 7231），不引 chrono/time。
fn http_date(secs: i64) -> String {
    const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTH_NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let weekday = (days + 4).rem_euclid(7) as usize;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAY_NAMES[weekday],
        day,
        MONTH_NAMES[(month - 1) as usize],
        year,
        hour,
        minute,
        second
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (
        year + i64::from(month <= 2),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

fn build_set_cookie(name: &str, value: &str, expires_at: i64) -> String {
    format!(
        "{name}={value}; Max-Age={COOKIE_MAX_AGE_SECS}; Path=/; Expires={}; HttpOnly; SameSite=Strict",
        http_date(expires_at)
    )
}

fn encode_cookie(secret: &[u8; 32], payload: &CookiePayload) -> String {
    let body = hex_encode(&serde_json::to_vec(payload).expect("cookie payload 序列化"));
    let signature = hmac_sign(secret, &body);
    format!("v1.{body}.{signature}")
}

fn decode_cookie(secret: &[u8; 32], value: &str) -> Option<CookiePayload> {
    let mut parts = value.split('.');
    let version = parts.next()?;
    let body = parts.next()?;
    let signature = parts.next()?;
    if version != "v1" || parts.next().is_some() || !hmac_verify(secret, body, signature) {
        return None;
    }
    let bytes = hex_decode(body)?;
    let payload: CookiePayload = serde_json::from_slice(&bytes).ok()?;
    if payload.v != 1 {
        return None;
    }
    Some(payload)
}

fn read_cookie(header: &str, name: &str) -> Option<String> {
    for segment in header.split(';') {
        let (key, value) = segment.trim().split_once('=')?;
        if key == name {
            return Some(value.to_owned());
        }
    }
    None
}

fn valid_cookie(trust: &Trust, authority: &str, value: &str) -> Option<CookiePayload> {
    let payload = decode_cookie(&trust.session_secret, value)?;
    let now = now_secs();
    if payload.authority != authority
        || payload.issued_at > now
        || payload.expires_at <= now
        || payload.expires_at <= payload.issued_at
        || payload.expires_at - payload.issued_at > COOKIE_MAX_AGE_SECS
    {
        return None;
    }
    Some(payload)
}

/// T006.F2：GET /connect?token= 换签名 cookie。
pub fn authorize_connect(
    trust: &Trust,
    host_header: Option<&str>,
    token: Option<&str>,
) -> Result<ConnectGrant, TrustError> {
    let host = host_header.ok_or_else(|| TrustError::BadHost("<missing>".to_owned()))?;
    if !authority_is_loopback(host, trust.bind_ip) {
        return Err(TrustError::BadHost(host.to_owned()));
    }
    let token = token.ok_or(TrustError::Unauthorized)?;
    if !token_matches(&trust.token, token) {
        return Err(TrustError::Unauthorized);
    }
    let authority = authority_of(host).ok_or_else(|| TrustError::BadHost(host.to_owned()))?;
    let issued_at = now_secs();
    let expires_at = issued_at.saturating_add(COOKIE_MAX_AGE_SECS);
    let payload = CookiePayload {
        v: 1,
        authority: authority.clone(),
        issued_at,
        expires_at,
    };
    let value = encode_cookie(&trust.session_secret, &payload);
    Ok(ConnectGrant {
        set_cookie: build_set_cookie(&cookie_name(&authority), &value, expires_at),
        location: "./".to_owned(),
    })
}

/// T006.F2：query token 只允许出现在 / 与 /connect；其他路径出现 token 一律 401。
pub fn reject_query_token(uri: &axum::http::Uri) -> Result<(), TrustError> {
    if let Some(query) = uri.query() {
        for pair in query.split('&') {
            let key = pair.split('=').next().unwrap_or_default();
            if key == "token" {
                return Err(TrustError::Unauthorized);
            }
        }
    }
    Ok(())
}

/// T006.F2：/api、/ws 的统一认证入口：Origin 栅栏 → 拒绝 query token → cookie 认证。
pub fn authorize_api_request(
    trust: &Trust,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), TrustError> {
    check_origin(headers)?;
    reject_query_token(uri)?;
    check_api_auth(trust, headers)
}

/// T006.F2：/api、/ws 只认 cookie 的认证入口。
pub fn check_api_auth(trust: &Trust, headers: &HeaderMap) -> Result<(), TrustError> {
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| TrustError::BadHost("<missing>".to_owned()))?;
    if !authority_is_loopback(host, trust.bind_ip) {
        return Err(TrustError::BadHost(host.to_owned()));
    }
    let authority = authority_of(host).ok_or_else(|| TrustError::BadHost(host.to_owned()))?;
    let cookie_header = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .ok_or(TrustError::Unauthorized)?;
    let Some(value) = read_cookie(cookie_header, &cookie_name(&authority)) else {
        return Err(TrustError::Unauthorized);
    };
    if valid_cookie(trust, &authority, &value).is_none() {
        return Err(TrustError::Unauthorized);
    }
    Ok(())
}

/// T006.F2：浏览器请求的 Origin / Fetch-Metadata 栅栏。
pub fn check_origin(headers: &HeaderMap) -> Result<(), TrustError> {
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| TrustError::BadHost("<missing>".to_owned()))?;
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
    {
        return Err(TrustError::BadOrigin(
            "sec-fetch-site: cross-site".to_owned(),
        ));
    }
    let Some(origin) = headers.get("origin") else {
        return Ok(());
    };
    let origin = origin
        .to_str()
        .map_err(|_| TrustError::BadOrigin("<non-utf8>".to_owned()))?;
    if origin_is_same_authority(origin, host) {
        Ok(())
    } else {
        Err(TrustError::BadOrigin(origin.to_owned()))
    }
}

/// 打开连接时的帧序列：ready 第一帧（带宿主信息），然后按 lastSeq 补齐的事件（重连带序号）。
pub fn opening_frames(info: &HostInfo, follow: FollowResponse) -> Vec<Frame> {
    let mut frames = Vec::with_capacity(follow.events.len() + 1);
    frames.push(Frame::Ready {
        session_id: info.session_id.clone(),
        protocol_version: PROTOCOL_VERSION,
        root: info.root.clone(),
        context: info.context,
        host: info.host.clone(),
    });
    frames.extend(
        follow
            .events
            .into_iter()
            .map(|event| Frame::Event { event }),
    );
    frames
}

/// 路由状态：宿主 + 信任栅栏（Arc 让每个连接共享）。
#[derive(Clone)]
struct AppState {
    host: Arc<RootHost>,
    trust: Trust,
    static_dir: Option<PathBuf>,
    monitor: Option<Monitor>,
}

#[derive(Debug, Deserialize)]
struct TokenQuery {
    #[serde(default)]
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsQuery {
    #[serde(default, rename = "lastSeq")]
    last_seq: Option<u64>,
}

/// 静态资源只校 Host（页面先拿 HTML，令牌在 API/WS 上用）；防 DNS rebinding。
fn host_check(state: &AppState, headers: &HeaderMap) -> Result<(), TrustError> {
    let host_header = headers.get(HOST).and_then(|value| value.to_str().ok());
    let host = host_header.ok_or_else(|| TrustError::BadHost("<missing>".to_owned()))?;
    if authority_is_loopback(host, state.trust.bind_ip) {
        Ok(())
    } else {
        Err(TrustError::BadHost(host.to_owned()))
    }
}

/// 静态路径解析：文件存在就发文件；无扩展名的路由回 index.html（SPA fallback）；
/// 有扩展名但文件不存在 → 404。拒绝 ..、绝对路径与空段。
fn resolve_static(root: &Path, request_path: &str) -> Option<PathBuf> {
    let trimmed = request_path.trim_start_matches('/');
    if trimmed.split('/').any(|segment| segment == "..") {
        return None;
    }
    if trimmed.is_empty() || request_path.ends_with('/') {
        return Some(root.join("index.html"));
    }
    let candidate = root.join(trimmed);
    if candidate.is_file() {
        return Some(candidate);
    }
    let has_extension = Path::new(trimmed).extension().is_some();
    if has_extension {
        None
    } else {
        Some(root.join("index.html"))
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}

/// 画布外壳页的 CSP（T015.1：CSP 由响应头下发，逐字照 `WORK/T015/strategy.md` 第 5 节）。
/// 脚本只认五个 CDN 和行内；字体只认 Google Fonts；fetch/XHR/WebSocket 只能连字体那两个地址。
pub const CANVAS_FRAME_CSP: &str = "default-src 'none'; \
script-src 'unsafe-inline' cdnjs.cloudflare.com cdn.jsdelivr.net/npm/ unpkg.com cdn.tailwindcss.com code.jquery.com; \
style-src 'unsafe-inline' fonts.googleapis.com cdnjs.cloudflare.com cdn.jsdelivr.net/npm/ unpkg.com cdn.tailwindcss.com code.jquery.com; \
font-src data: fonts.gstatic.com; \
img-src data: blob:; \
media-src data: blob:; \
connect-src fonts.googleapis.com fonts.gstatic.com; \
frame-src 'none'; \
object-src 'none'; \
base-uri 'none'; \
frame-ancestors 'self'";

/// GET /canvas-frame：画布外壳页（T015.1）。不要 cookie，只校 Host；CSP 写在响应头里。
async fn canvas_frame_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = host_check(&state, &headers) {
        return trust_error_response(error);
    }
    let Some(root) = state.static_dir.as_deref() else {
        return error_response(StatusCode::NOT_FOUND, "not found");
    };
    match tokio::fs::read(root.join("canvas-frame.html")).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, "text/html; charset=utf-8"),
                (HeaderName::from_static("content-security-policy"), CANVAS_FRAME_CSP),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => error_response(StatusCode::NOT_FOUND, "not found"),
    }
}

/// GET 静态文件：同源托管 web/dist（T007.2）；没有 static_dir 时 404。
async fn static_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response {
    if let Err(error) = host_check(&state, &headers) {
        return trust_error_response(error);
    }
    let Some(root) = state.static_dir.as_deref() else {
        return error_response(StatusCode::NOT_FOUND, "not found");
    };
    let Some(path) = resolve_static(root, uri.path()) else {
        return error_response(StatusCode::NOT_FOUND, "not found");
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (StatusCode::OK, [(CONTENT_TYPE, content_type(&path))], bytes).into_response(),
        Err(_) => error_response(StatusCode::NOT_FOUND, "not found"),
    }
}

/// T009.1：ServiceError 的 HTTP 状态（保持 T008 现状：root/unavailable 409，其余 502）。
fn service_error_status(code: service::ServiceErrorCode) -> StatusCode {
    match code {
        service::ServiceErrorCode::RootUnavailable => StatusCode::CONFLICT,
        service::ServiceErrorCode::RequestInvalidJson
        | service::ServiceErrorCode::AttachmentInvalid
        | service::ServiceErrorCode::AttachmentNotText
        | service::ServiceErrorCode::RequestInvalidMessage => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    }
}

/// T009.1：/api 的错误响应（DSH RemoteError 形状：{code,message,details?}）。
fn api_error_response(error: ServiceError) -> Response {
    let status = service_error_status(error.code());
    (
        status,
        Json(service::ServiceErrorBody::from_service_error(&error)),
    )
        .into_response()
}

/// T009.1：请求体不合法（不是 ServiceError 变体）：单独一个码 request/invalid-json。
fn bad_request_response(message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(service::ServiceErrorBody {
            code: service::ServiceErrorCode::RequestInvalidJson
                .as_str()
                .to_owned(),
            message,
            details: None,
        }),
    )
        .into_response()
}

fn error_response(status: StatusCode, message: &str) -> Response {
    // T009.1：非 /api 的错误（静态 404、信任栅栏 401/403）照来源回纯文本，不再用旧的 error JSON 体。
    (status, message.to_owned()).into_response()
}

fn trust_error_response(error: TrustError) -> Response {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    error_response(status, error.to_string().as_str())
}

/// POST /api：Call JSON → dispatch → Response JSON。
async fn api_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    body: Bytes,
) -> Response {
    if let Err(error) = authorize_api_request(&state.trust, &headers, &uri) {
        return trust_error_response(error);
    }
    let started = std::time::Instant::now();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    let method = value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    // T011.6：页面没接住的异常/事件流重连经这里交给服务记 ops（内容限长，不阻塞页面）。
    if method == "monitor-report" {
        let kind = value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("page-error");
        let op = if kind == "page-reconnect" {
            "page-reconnect"
        } else {
            "page-error"
        };
        let message: String = value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(512)
            .collect();
        record_ops(
            &state,
            op,
            if op == "page-error" {
                Severity::Warn
            } else {
                Severity::Info
            },
            vec![("message".to_owned(), serde_json::json!(message))],
        );
        return (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response();
    }
    let call: service::Call = match serde_json::from_value(value.clone()) {
        Ok(call) => call,
        Err(error) => {
            return bad_request_response(format!("bad request: {error}"));
        }
    };
    let result = state.host.dispatch(call).await;
    let duration_ms = started.elapsed().as_millis() as u64;
    let (http_status, code, ok) = match &result {
        Ok(_) => (200_u16, "ok".to_owned(), true),
        Err(error) => (
            service_error_status(error.code()).as_u16(),
            error.code().as_str().to_owned(),
            false,
        ),
    };
    // T011.4：每次 /api 一条 api-call（不记请求体/响应体）；Keyring/连接操作另记一条 ops。
    // T012.3 P1：请求里带档（T012.7 起 session-send/session-model-select 会带）时，api-call 记下来。
    let mut attributes = vec![
        ("method".to_owned(), serde_json::json!(method)),
        ("status".to_owned(), serde_json::json!(http_status)),
        ("code".to_owned(), serde_json::json!(code)),
        ("durationMs".to_owned(), serde_json::json!(duration_ms)),
    ];
    // 请求里带档时记下来：session-send/session-model-select 用 effortWanted（camelCase），
    // 其他调用方若用 effort 也认。
    let effort = value
        .get("effort")
        .or_else(|| value.get("effortWanted"))
        .and_then(serde_json::Value::as_str);
    if let Some(effort) = effort {
        attributes.push(("effort".to_owned(), serde_json::json!(effort)));
    }
    record_ops(
        &state,
        "api-call",
        if ok { Severity::Info } else { Severity::Warn },
        attributes,
    );
    record_domain_op(&state, &method, &value, ok, &code, duration_ms);
    match result {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => api_error_response(error),
    }
}

/// 一条 ops 记录（attributes 里带 telemetry.op；body 不写请求/响应内容）。
fn record_ops(
    state: &AppState,
    op: &str,
    severity: Severity,
    attributes: Vec<(String, serde_json::Value)>,
) {
    let Some(monitor) = &state.monitor else {
        return;
    };
    let mut map = serde_json::Map::new();
    map.insert("telemetry.op".to_owned(), serde_json::json!(op));
    if let Some(session_id) = state.host.info().session_id {
        map.insert("session.id".to_owned(), serde_json::json!(session_id));
    }
    for (key, value) in attributes {
        map.insert(key, value);
    }
    let _ = monitor.record(Record::new(
        Channel::Ops,
        severity,
        map,
        serde_json::json!({}),
    ));
}

/// T011.4：Keyring/连接操作的 ops（只记动作、连接 id/model、结果、码、耗时）。
fn record_domain_op(
    state: &AppState,
    method: &str,
    body: &serde_json::Value,
    ok: bool,
    code: &str,
    duration_ms: u64,
) {
    let op = match method {
        "connection-save"
        | "connection-delete"
        | "connection-verify"
        | "connection-update"
        | "connection-revoke"
        | "session-model-select" => "connection",
        "keyring-relocate" | "keyring-reopen" | "keyring-restart" => "keyring",
        _ => return,
    };
    let action = match method {
        "session-model-select" => "model-select",
        other => other.strip_prefix("connection-").unwrap_or(other),
    };
    let mut attributes = vec![
        ("action".to_owned(), serde_json::json!(action)),
        (
            "result".to_owned(),
            serde_json::json!(if ok { "ok" } else { "failed" }),
        ),
        ("code".to_owned(), serde_json::json!(code)),
        ("durationMs".to_owned(), serde_json::json!(duration_ms)),
    ];
    if let Some(id) = body.get("id").and_then(serde_json::Value::as_str) {
        attributes.push(("connectionId".to_owned(), serde_json::json!(id)));
    }
    if let Some(model) = body.get("model").and_then(serde_json::Value::as_str) {
        attributes.push(("model".to_owned(), serde_json::json!(model)));
    }
    record_ops(
        state,
        op,
        if ok { Severity::Info } else { Severity::Warn },
        attributes,
    );
}

/// T011.4：事件流的连上/断开记录（reason 在 Drop 兜底写）。
struct StreamOps {
    monitor: Option<Monitor>,
    session_id: Option<String>,
    reason: Mutex<&'static str>,
}

impl StreamOps {
    fn open(state: &AppState) -> Self {
        let session_id = state.host.info().session_id;
        record_ops(
            state,
            "stream-open",
            Severity::Info,
            vec![("reason".to_owned(), serde_json::json!("requested"))],
        );
        Self {
            monitor: state.monitor.clone(),
            session_id,
            reason: Mutex::new("closed"),
        }
    }
}

impl Drop for StreamOps {
    fn drop(&mut self) {
        let Some(monitor) = &self.monitor else {
            return;
        };
        let mut map = serde_json::Map::new();
        map.insert("telemetry.op".to_owned(), serde_json::json!("stream-close"));
        if let Some(session_id) = &self.session_id {
            map.insert("session.id".to_owned(), serde_json::json!(session_id));
        }
        if let Ok(reason) = self.reason.lock() {
            map.insert("reason".to_owned(), serde_json::json!(*reason));
        }
        let _ = monitor.record(Record::new(
            Channel::Ops,
            Severity::Info,
            map,
            serde_json::json!({}),
        ));
    }
}

/// GET /ws：先过信任栅栏，再升级 WebSocket；连接内先发 ready 再发事件。
async fn ws_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Err(error) = authorize_api_request(&state.trust, &headers, &uri) {
        return trust_error_response(error);
    }
    let last_seq = query.last_seq.unwrap_or(0);
    upgrade.on_upgrade(move |socket| ws_session(socket, state, last_seq))
}

fn connect_response(grant: ConnectGrant) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(SET_COOKIE, grant.set_cookie), (LOCATION, grant.location)],
    )
        .into_response()
}

/// GET /connect?token=：启动令牌换签名 cookie（持有者随后用 cookie 访问 /api、/ws）。
async fn connect_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = check_origin(&headers) {
        return trust_error_response(error);
    }
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    match authorize_connect(&state.trust, host, query.token.as_deref()) {
        Ok(grant) => connect_response(grant),
        Err(error) => trust_error_response(error),
    }
}

/// GET /?token=：页面首屏换 cookie 后跳回不带令牌的地址；已有 cookie 也跳转。
async fn root_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TokenQuery>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response {
    if let Err(error) = check_origin(&headers) {
        return trust_error_response(error);
    }
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    if query.token.is_some() {
        return match authorize_connect(&state.trust, host, query.token.as_deref()) {
            Ok(grant) => connect_response(grant),
            Err(error) => trust_error_response(error),
        };
    }
    if check_api_auth(&state.trust, &headers).is_ok() {
        return static_handler(State(state), headers, uri).await;
    }
    error_response(StatusCode::UNAUTHORIZED, UNAUTHORIZED_BODY)
}

async fn send_value(socket: &mut WebSocket, value: &serde_json::Value) -> bool {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .is_ok()
}

async fn send_frame(socket: &mut WebSocket, frame: &Frame) -> bool {
    match serde_json::to_value(frame) {
        Ok(value) => send_value(socket, &value).await,
        Err(_) => false,
    }
}

/// 一个 WebSocket 连接：ready → 补齐（lastSeq 之后）→ live（seq 去重；Lagged 时重新 follow 补齐）。
async fn ws_session(mut socket: WebSocket, state: Arc<AppState>, last_seq: u64) {
    let _stream_ops = StreamOps::open(&state);
    let info = state.host.info();
    let mut live = state.host.subscribe_events();
    let mut agent_live = state.host.subscribe_agent_events();
    let root_available = info.root == RootState::Ok;
    let follow = if root_available {
        match state.host.follow(FollowRequest { last_seq }).await {
            Ok(follow) => follow,
            Err(error) => {
                let value = serde_json::json!({"type": "error", "message": error.to_string()});
                let _ = send_value(&mut socket, &value).await;
                return;
            }
        }
    } else {
        FollowResponse {
            events: Vec::new(),
            last_seq,
        }
    };
    let mut last = last_seq;
    if let Some(seq) = follow.events.last().map(|event| event.seq) {
        last = last.max(seq);
    }
    for frame in opening_frames(&info, follow) {
        if !send_frame(&mut socket, &frame).await {
            return;
        }
    }
    if !root_available {
        // T007.2：根会话不可用时页面根据 ready 显示说明页；连接保持到关闭/重连，
        // 修好后页面调 recheck 再重连。
        while let Some(Ok(message)) = socket.recv().await {
            if matches!(message, Message::Close(_)) {
                return;
            }
        }
        return;
    }
    loop {
        tokio::select! {
            received = live.recv() => match received {
                Ok(event) => {
                    if event.seq <= last {
                        continue;
                    }
                    last = event.seq;
                    if !send_frame(&mut socket, &Frame::Event { event }).await {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if let Ok(follow) = state.host.follow(FollowRequest { last_seq: last }).await {
                        for event in follow.events {
                            if event.seq <= last {
                                continue;
                            }
                            last = event.seq;
                            if !send_frame(&mut socket, &Frame::Event { event }).await {
                                return;
                            }
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            },
            agent_received = agent_live.recv() => match agent_received {
                Ok(event) => {
                    let Some(wire) = AgentEventWire::from_event(&event) else {
                        continue;
                    };
                    if !send_frame(&mut socket, &Frame::Agent { event: wire }).await {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(message)) => {
                    if matches!(message, Message::Close(_)) {
                        return;
                    }
                    continue;
                }
                _ => return,
            },
        }
    }
}

/// 由宿主和信任栅栏建 axum Router（HTTP /api + WS /ws）。
pub fn router(host: RootHost, trust: Trust, static_dir: Option<PathBuf>) -> Router {
    router_with_state(Arc::new(AppState {
        host: Arc::new(host),
        trust,
        static_dir,
        monitor: None,
    }))
}

fn router_with_state(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .route("/connect", get(connect_handler))
        .route("/api", post(api_handler))
        .route("/ws", get(ws_handler))
        .route("/canvas-frame", get(canvas_frame_handler))
        .fallback(get(static_handler))
        .with_state(state)
}

/// 起服务：校验绑定地址 → TcpListener → axum::serve。
pub async fn serve(options: ServeOptions, host: RootHost) -> Result<ServeHandle, ServeError> {
    let ip = check_bind_host(&options.host)?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(ip, options.port)).await?;
    let addr = listener.local_addr()?;
    let state = Arc::new(AppState {
        host: Arc::new(host),
        trust: Trust::new(
            ip,
            addr.port(),
            options.token.clone(),
            options.session_secret,
        ),
        static_dir: options.static_dir.clone(),
        monitor: options.monitor.clone(),
    });
    let app = router_with_state(Arc::clone(&state));
    let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_signal.await;
            })
            .await;
        // T006.18：停服务收尾——正在跑的一轮按取消收尾，日志完整（故事 13）。
        state.host.shutdown().await;
        result.map_err(|error| std::io::Error::other(error.to_string()))
    });
    Ok(ServeHandle {
        addr,
        token: options.token,
        task,
        shutdown: Some(shutdown),
    })
}

#[cfg(test)]
mod t007_static_tests {
    use super::*;

    #[test]
    fn resolve_static_serves_file_spa_and_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "console.log(1)").unwrap();
        assert_eq!(
            resolve_static(dir.path(), "/assets/app.js"),
            Some(dir.path().join("assets/app.js"))
        );
        assert_eq!(
            resolve_static(dir.path(), "/"),
            Some(dir.path().join("index.html"))
        );
        assert_eq!(
            resolve_static(dir.path(), "/conversation/42"),
            Some(dir.path().join("index.html")),
            "无扩展名路由回 SPA"
        );
        assert_eq!(resolve_static(dir.path(), "/assets/missing.js"), None);
        assert_eq!(resolve_static(dir.path(), "/../etc/passwd"), None);
        assert_eq!(
            content_type(Path::new("index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("app.JS")),
            "application/octet-stream"
        );
    }
}

#[cfg(test)]
mod cookie_red_tests {
    use super::*;

    fn trust() -> Trust {
        Trust::new(
            IpAddr::from([127, 0, 0, 1]),
            1417,
            "tok-1".to_owned(),
            [7u8; 32],
        )
    }

    fn host_headers(authority: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, authority.to_owned().parse().unwrap());
        headers
    }

    /// T006 r3 #3：401 正文照来源意思说「重新打开打印的地址」，不出现来源名字/令牌字样。
    #[test]
    fn unauthorized_body_reopens_printed_address() {
        let body = TrustError::Unauthorized.to_string();
        assert_eq!(
            body,
            "还没有登录或登录已失效：请重新打开 aios-dev serve 打印的地址。"
        );
        assert!(!body.contains("令牌"), "cookie 错时不再说令牌：{body}");
        assert!(!body.contains("dsh"), "不出现来源名字：{body}");
        assert_eq!(TrustError::Unauthorized.to_string(), UNAUTHORIZED_BODY);
    }

    #[test]
    fn dod7_expired_cookie_is_401() {
        let trust = trust();
        let authority = "127.0.0.1:1417";
        let now = now_secs();
        let payload = CookiePayload {
            v: 1,
            authority: authority.to_owned(),
            issued_at: now - 10,
            expires_at: now - 1,
        };
        let value = encode_cookie(&trust.session_secret, &payload);
        let mut headers = host_headers(authority);
        headers.insert(
            "cookie",
            format!("{}={value}", cookie_name(authority))
                .parse()
                .unwrap(),
        );
        assert_eq!(check_api_auth(&trust, &headers).unwrap_err().status(), 401);
    }

    #[test]
    fn dod7_payload_authority_mismatch_is_401() {
        let trust = trust();
        let authority = "127.0.0.1:1417";
        let now = now_secs();
        let payload = CookiePayload {
            v: 1,
            authority: "evil.example:80".to_owned(),
            issued_at: now,
            expires_at: now + 60,
        };
        let value = encode_cookie(&trust.session_secret, &payload);
        let mut headers = host_headers(authority);
        headers.insert(
            "cookie",
            format!("{}={value}", cookie_name(authority))
                .parse()
                .unwrap(),
        );
        assert_eq!(check_api_auth(&trust, &headers).unwrap_err().status(), 401);
    }
}

#[cfg(test)]
mod t012_attachment_http_tests {
    use super::*;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use std::net::IpAddr;
    use std::path::Path;

    use agent::AgentConfig;
    use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
    use service::{RootHost, RootSessionOptions};

    fn trust() -> Trust {
        Trust::new(
            IpAddr::from([127, 0, 0, 1]),
            1417,
            "tok-1".to_owned(),
            [7u8; 32],
        )
    }

    fn options(data_dir: &Path, identity: &Path) -> RootSessionOptions {
        RootSessionOptions {
            data_dir: data_dir.to_path_buf(),
            cwd: data_dir.to_path_buf(),
            model: DeepSeekConfig {
                base_url: "http://127.0.0.1:1".to_owned(),
                model: "deepseek-flash".to_owned(),
                max_tokens: 2048,
                context_window: DEFAULT_CONTEXT_WINDOW,
                effort: Effort::new("high"),
            },
            config: AgentConfig::default(),
            credentials: Some(std::sync::Arc::new(TestCredential)),
            identity: Some(identity.to_path_buf()),
            model_factory: None,
            append_hook_factory: None,
            keyring_changed: None,
        }
    }

    struct TestCredential;

    impl model::CredentialSource for TestCredential {
        fn issue(
            &self,
            _purpose: &str,
            _context: &model::CredentialContext,
        ) -> Result<String, model::ModelError> {
            Ok("test-key".to_owned())
        }
    }

    async fn host(dir: &Path) -> Arc<RootHost> {
        Arc::new(
            RootHost::open(
                options(dir, &dir.join("identity.txt")),
                DEFAULT_CONTEXT_WINDOW,
            )
            .await,
        )
    }

    /// 走真实的 api_handler（带 cookie/host/origin 认证头），返回 (HTTP 状态, JSON body)。
    async fn call_api(
        host: Arc<RootHost>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let trust = trust();
        let state = Arc::new(AppState {
            host,
            trust: trust.clone(),
            static_dir: None,
            monitor: None,
        });
        let grant =
            authorize_connect(&trust, Some("127.0.0.1:1417"), Some("tok-1")).expect("换 cookie");
        let cookie = grant
            .set_cookie
            .split(';')
            .next()
            .expect("Set-Cookie")
            .to_owned();
        let mut headers = HeaderMap::new();
        headers.insert("host", "127.0.0.1:1417".parse().unwrap());
        headers.insert("origin", "http://127.0.0.1:1417".parse().unwrap());
        headers.insert("cookie", cookie.parse().unwrap());
        let uri: axum::http::Uri = "/api".parse().unwrap();
        let response = api_handler(
            State(state),
            headers,
            uri,
            Bytes::from(serde_json::to_vec(&body).unwrap()),
        )
        .await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn send_over_http_accepts_pastes_and_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path()).await;
        let (status, put) = call_api(
            host.clone(),
            serde_json::json!({
                "method": "attachment-put",
                "name": "notes.txt",
                "data": "aGVsbG8=",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{put}");
        let id = put["id"].as_str().expect("id").to_owned();
        let (status, json) = call_api(
            host,
            serde_json::json!({
                "method": "send",
                "requestId": "req-t013-4-http",
                "content": "看这个",
                "pastes": ["第一段"],
                "attachments": [{"id": id, "name": "notes.txt", "bytes": 5}],
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "send 应受理：{json}");
        assert_eq!(json["method"], "send");
        assert_eq!(json["accepted"], true);
    }

    #[tokio::test]
    async fn send_over_http_rejects_an_empty_message() {
        let dir = tempfile::tempdir().unwrap();
        let (status, json) = call_api(
            host(dir.path()).await,
            serde_json::json!({
                "method": "send",
                "requestId": "req-t013-4-empty",
                "content": "",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        assert_eq!(json["code"], "request/invalid-message");
    }

    #[tokio::test]
    async fn send_over_http_rejects_an_unknown_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let (status, json) = call_api(
            host(dir.path()).await,
            serde_json::json!({
                "method": "send",
                "requestId": "req-t013-4-bad-attachment",
                "content": "带个文件",
                "attachments": [{
                    "id": "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                    "name": "missing.txt",
                    "bytes": 1,
                }],
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        assert_eq!(json["code"], "attachment/invalid");
    }

    #[tokio::test]
    async fn attachment_put_over_http_returns_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let (status, json) = call_api(
            host(dir.path()).await,
            serde_json::json!({
                "method": "attachment-put",
                "name": "notes.txt",
                "data": "aGVsbG8=",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["method"], "attachment-put");
        assert!(
            json["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("sha256:")),
            "{json}"
        );
        assert_eq!(json["bytes"], 5);
    }

    #[tokio::test]
    async fn attachment_put_over_http_rejects_bad_base64() {
        let dir = tempfile::tempdir().unwrap();
        let (status, json) = call_api(
            host(dir.path()).await,
            serde_json::json!({
                "method": "attachment-put",
                "name": "a.txt",
                "data": "AA=A",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        assert_eq!(json["code"], "attachment/invalid");
    }

    #[tokio::test]
    async fn attachment_put_over_http_rejects_binary() {
        let dir = tempfile::tempdir().unwrap();
        let (status, json) = call_api(
            host(dir.path()).await,
            serde_json::json!({
                "method": "attachment-put",
                "name": "bin.dat",
                "data": "AA==",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        assert_eq!(json["code"], "attachment/not-text");
    }

    #[tokio::test]
    async fn attachment_get_over_http_rejects_bad_id() {
        let dir = tempfile::tempdir().unwrap();
        let (status, json) = call_api(
            host(dir.path()).await,
            serde_json::json!({
                "method": "attachment-get",
                "id": "sha256:xx",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        assert_eq!(json["code"], "attachment/invalid");
    }
}

#[cfg(test)]
mod t009_error_tests {
    use super::*;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn api_error_envelope_uses_structured_body() {
        let cases: Vec<(ServiceError, StatusCode, &str)> = vec![
            (
                ServiceError::Agent(agent::AgentError::Compaction("x".to_owned())),
                StatusCode::BAD_GATEWAY,
                "agent/internal",
            ),
            (
                ServiceError::AnswerRejected("q1".to_owned()),
                StatusCode::BAD_GATEWAY,
                "question/answer-rejected",
            ),
            (
                ServiceError::RootUnavailable(service::RootState::Ok),
                StatusCode::CONFLICT,
                "root/unavailable",
            ),
            (
                ServiceError::SettingsOnBareHost,
                StatusCode::BAD_GATEWAY,
                "settings/unavailable",
            ),
            (
                ServiceError::InvalidPath("bad".to_owned()),
                StatusCode::BAD_GATEWAY,
                "request/invalid-path",
            ),
        ];
        for (error, status, code) in cases {
            let response = api_error_response(error);
            assert_eq!(response.status(), status);
            let json = body_json(response).await;
            assert_eq!(json["code"], code, "body={json}");
            assert!(json.get("error").is_none(), "不再有旧 error 字段：{json}");
            assert!(json["message"].as_str().is_some(), "body={json}");
        }
    }

    #[tokio::test]
    async fn invalid_json_uses_the_shared_code_table() {
        let response = bad_request_response("bad".to_owned());
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "request/invalid-json");
        assert!(json.get("error").is_none(), "不再有旧 error 字段：{json}");
        assert!(
            service::ServiceErrorCode::ALL
                .iter()
                .any(|code| code.as_str() == "request/invalid-json"),
            "invalid-json 必须进唯一码表 ALL"
        );
    }

    #[test]
    fn service_error_statuses_are_stable() {
        for code in service::ServiceErrorCode::ALL {
            let status = service_error_status(code);
            if matches!(code, service::ServiceErrorCode::RootUnavailable) {
                assert_eq!(status, StatusCode::CONFLICT);
            } else if matches!(
                code,
                service::ServiceErrorCode::RequestInvalidJson
                    | service::ServiceErrorCode::AttachmentInvalid
                    | service::ServiceErrorCode::AttachmentNotText
                    | service::ServiceErrorCode::RequestInvalidMessage
            ) {
                assert_eq!(status, StatusCode::BAD_REQUEST);
            } else {
                assert_eq!(status, StatusCode::BAD_GATEWAY);
            }
        }
    }
}

#[cfg(test)]
mod t011_ops_tests {
    use super::*;
    use axum::body::Bytes;

    fn model_config() -> model::DeepSeekConfig {
        model::DeepSeekConfig {
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: model::DEFAULT_CONTEXT_WINDOW,
            effort: model::Effort::new("high"),
        }
    }

    fn auth_headers(trust: &Trust) -> HeaderMap {
        let host = "127.0.0.1:1417";
        let cookie = authorize_connect(trust, Some(host), Some("tok-1"))
            .expect("换 cookie")
            .set_cookie
            .split(';')
            .next()
            .expect("cookie")
            .to_owned();
        let mut headers = HeaderMap::new();
        headers.insert(HOST, host.parse().expect("Host"));
        headers.insert("cookie", cookie.parse().expect("cookie"));
        headers
    }

    async fn setup() -> (tempfile::TempDir, Arc<AppState>, HeaderMap) {
        let dir = tempfile::tempdir().expect("临时目录");
        let identity = dir.path().join("identity.txt");
        let options = service::RootSessionOptions {
            data_dir: dir.path().to_path_buf(),
            cwd: dir.path().to_path_buf(),
            model: model_config(),
            config: agent::AgentConfig::default(),
            credentials: None,
            identity: Some(identity),
            model_factory: None,
            append_hook_factory: None,
            keyring_changed: None,
        };
        let host = service::RootHost::open(options, model::DEFAULT_CONTEXT_WINDOW).await;
        let monitor = Monitor::open_with_capacity(dir.path(), 1024).expect("监控");
        let trust = Trust::new(
            std::net::IpAddr::from([127, 0, 0, 1]),
            1417,
            "tok-1".to_owned(),
            [7_u8; 32],
        );
        let headers = auth_headers(&trust);
        let state = Arc::new(AppState {
            host: Arc::new(host),
            trust,
            static_dir: None,
            monitor: Some(monitor),
        });
        (dir, state, headers)
    }

    async fn call(state: &Arc<AppState>, headers: &HeaderMap, body: serde_json::Value) -> Response {
        api_handler(
            State(Arc::clone(state)),
            headers.clone(),
            axum::http::Uri::from_static("/api"),
            Bytes::from(body.to_string()),
        )
        .await
    }

    #[tokio::test]
    async fn api_and_domain_ops_are_recorded_without_bodies() {
        let (dir, state, headers) = setup().await;
        let keyring_new = dir.path().join("moved-identity.txt");
        let bodies = vec![
            serde_json::json!({"method": "status"}),
            serde_json::json!({"method": "follow", "lastSeq": 0}),
            serde_json::json!({"method": "events", "maxMessages": 5}),
            serde_json::json!({"method": "recheck"}),
            serde_json::json!({"method": "first-use"}),
            serde_json::json!({"method": "issues"}),
            serde_json::json!({"method": "model-settings"}),
            serde_json::json!({
                "method": "connection-save",
                "id": "mon-test",
                "name": "Monitor Test",
                "model": "deepseek-flash",
                "endpoint": "http://127.0.0.1:1",
                "apiKey": "sk-monitor-secret"
            }),
            serde_json::json!({"method": "connection-verify", "id": "missing-conn"}),
            serde_json::json!({"method": "connection-delete", "id": "missing-conn"}),
            serde_json::json!({"method": "connection-update", "id": "missing-conn", "endpoint": "http://127.0.0.1:1", "model": "deepseek-flash"}),
            serde_json::json!({"method": "connection-revoke", "id": "missing-conn"}),
            serde_json::json!({"method": "session-model-select", "model": "deepseek-pro", "effortWanted": "max"}),
            serde_json::json!({"method": "keyring-restart"}),
            serde_json::json!({"method": "keyring-relocate", "to": keyring_new.display().to_string()}),
            serde_json::json!({"method": "keyring-reopen", "at": keyring_new.display().to_string()}),
        ];
        let methods = [
            "status",
            "follow",
            "events",
            "recheck",
            "first-use",
            "issues",
            "model-settings",
            "connection-save",
            "connection-verify",
            "connection-delete",
            "connection-update",
            "connection-revoke",
            "session-model-select",
            "keyring-restart",
            "keyring-relocate",
            "keyring-reopen",
        ];
        for body in &bodies {
            let _ = call(&state, &headers, body.clone()).await;
        }
        // 事件流连上/断开也各记一条。
        drop(StreamOps::open(&state));
        drop(state);

        let mut files: Vec<_> = std::fs::read_dir(dir.path().join("monitor"))
            .expect("monitor 目录")
            .map(|entry| entry.expect("目录项").path())
            .collect();
        files.sort();
        let mut records = Vec::new();
        let mut text = String::new();
        for file in &files {
            let content = std::fs::read_to_string(file).expect("读监控");
            text.push_str(&content);
            for line in content.lines() {
                records.push(serde_json::from_str::<serde_json::Value>(line).expect("JSON"));
            }
        }
        let api_calls: Vec<&serde_json::Value> = records
            .iter()
            .filter(|record| record["attributes"]["telemetry.op"] == "api-call")
            .collect();
        assert_eq!(
            api_calls.len(),
            methods.len(),
            "每次 /api 一条 api-call：{text}"
        );
        for method in methods {
            assert_eq!(
                api_calls
                    .iter()
                    .filter(|record| record["attributes"]["method"] == method)
                    .count(),
                1,
                "方法 {method} 一条 api-call"
            );
        }
        for record in &api_calls {
            assert!(record["attributes"]["status"].as_u64().is_some());
            assert!(record["attributes"]["code"].as_str().is_some());
            assert!(record["attributes"]["durationMs"].as_u64().is_some());
        }
        // T012.3 P1：带档的请求在 api-call 里记 effort（session-model-select 用 effortWanted）。
        let effort_call = api_calls
            .iter()
            .find(|record| record["attributes"]["method"] == "session-model-select")
            .expect("session-model-select 一条 api-call");
        assert_eq!(
            effort_call["attributes"]["effort"], "max",
            "带档请求记 effort：{text}"
        );
        let verify = api_calls
            .iter()
            .find(|record| record["attributes"]["method"] == "connection-verify")
            .expect("verify 记录");
        assert_eq!(
            verify["attributes"]["code"], "connection/missing",
            "码与 T009 码表一致"
        );

        let connections: Vec<&serde_json::Value> = records
            .iter()
            .filter(|record| record["attributes"]["telemetry.op"] == "connection")
            .collect();
        assert_eq!(connections.len(), 6, "连接操作各一条");
        let keyring_ops: Vec<&serde_json::Value> = records
            .iter()
            .filter(|record| record["attributes"]["telemetry.op"] == "keyring")
            .collect();
        assert_eq!(keyring_ops.len(), 3, "Keyring 操作各一条");

        let streams: Vec<&serde_json::Value> = records
            .iter()
            .filter(|record| {
                record["attributes"]["telemetry.op"] == "stream-open"
                    || record["attributes"]["telemetry.op"] == "stream-close"
            })
            .collect();
        assert_eq!(streams.len(), 2, "事件流连上/断开各一条");

        assert!(
            !text.contains("sk-monitor-secret"),
            "监控里不能有请求体里的 key：{text}"
        );
        assert!(!text.contains("apiKey"), "监控里不能有请求体字段：{text}");
    }
}
