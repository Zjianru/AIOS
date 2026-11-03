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

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{
    HeaderMap, StatusCode,
    header::{CONTENT_TYPE, HOST},
};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use service::{
    ContextUsage, FollowRequest, FollowResponse, HostInfo, PROTOCOL_VERSION, RootHost, RootState,
    ServiceError, ServiceEvent,
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
    /// 每进程一份的 cookie 签名钥匙；只放内存，不落盘、不进日志。
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
    pub fn new(bind_ip: IpAddr, bind_port: u16, token: String) -> Self {
        let mut session_secret = [0u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut session_secret)
            .expect("ring 系统随机数不可用");
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
    /// 同源托管的 web 构建产物目录（T007.2，通常是仓库的 web/dist）；None 时不托管静态文件。
    pub static_dir: Option<PathBuf>,
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
    },
    /// 日志事件帧（DSH 的 {type:'event', event: SessionWireEvent}）。
    Event { event: ServiceEvent },
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

/// 校验一次请求：Host 头必须是本机（回环/localhost/绑定 IP），令牌必须匹配。
pub fn check_request(
    trust: &Trust,
    host_header: Option<&str>,
    token: Option<&str>,
) -> Result<(), TrustError> {
    let host = host_header.ok_or_else(|| TrustError::BadHost("<missing>".to_owned()))?;
    if !authority_is_loopback(host, trust.bind_ip) {
        return Err(TrustError::BadHost(host.to_owned()));
    }
    let token = token.ok_or(TrustError::Unauthorized)?;
    if !token_matches(&trust.token, token) {
        return Err(TrustError::Unauthorized);
    }
    Ok(())
}

/// 打开连接时的帧序列：ready 第一帧（带宿主信息），然后按 lastSeq 补齐的事件（重连带序号）。
pub fn opening_frames(info: &HostInfo, follow: FollowResponse) -> Vec<Frame> {
    let mut frames = Vec::with_capacity(follow.events.len() + 1);
    frames.push(Frame::Ready {
        session_id: info.session_id.clone(),
        protocol_version: PROTOCOL_VERSION,
        root: info.root.clone(),
        context: info.context,
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

fn trust_check(
    state: &AppState,
    headers: &HeaderMap,
    token: Option<&str>,
) -> Result<(), TrustError> {
    let host_header = headers.get(HOST).and_then(|value| value.to_str().ok());
    check_request(&state.trust, host_header, token)
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

fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": message.to_owned()})),
    )
        .into_response()
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
    let call: service::Call = match serde_json::from_slice(&body) {
        Ok(call) => call,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("bad request: {error}"));
        }
    };
    match state.host.dispatch(call).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(ServiceError::RootUnavailable(root)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "ROOT_UNAVAILABLE",
                "root": root,
                "protocolVersion": PROTOCOL_VERSION,
            })),
        )
            .into_response(),
        Err(error) => error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
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
        return (StatusCode::SEE_OTHER, [(LOCATION, "./".to_owned())]).into_response();
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
    let info = state.host.info();
    let mut live = state.host.subscribe_events();
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
    }))
}

fn router_with_state(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .route("/connect", get(connect_handler))
        .route("/api", post(api_handler))
        .route("/ws", get(ws_handler))
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
        trust: Trust::new(ip, addr.port(), options.token.clone()),
        static_dir: options.static_dir.clone(),
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
