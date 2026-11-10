//! HTTP 路由与请求解析：/api、静态资源、画布外壳、/local-file，
//! 以及每次调用 / 连接操作的监控 ops。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::http::HeaderName;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header::{CONTENT_TYPE, HOST, LOCATION, SET_COOKIE};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use monitor::{Channel, Monitor, Record, Severity};
use serde::Deserialize;
use service::{RootHost, ServiceError};

use crate::connection::ws_handler;
use crate::trust::{
    ConnectGrant, Trust, TrustError, UNAUTHORIZED_BODY, authority_is_loopback,
    authorize_api_request, authorize_connect, check_api_auth, check_origin,
};

/// 路由状态：宿主 + 信任栅栏（Arc 让每个连接共享）。
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) host: Arc<RootHost>,
    pub(crate) trust: Trust,
    pub(crate) static_dir: Option<PathBuf>,
    pub(crate) monitor: Option<Monitor>,
}

#[derive(Debug, Deserialize)]
struct TokenQuery {
    #[serde(default)]
    token: Option<String>,
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

/// 静态 fallback 绝不能直接发的文件（画布外壳；只能走 canvas_frame_handler）。
fn is_canvas_frame_alias(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("canvas-frame.html"))
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
async fn canvas_frame_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
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
                (
                    HeaderName::from_static("content-security-policy"),
                    CANVAS_FRAME_CSP,
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => error_response(StatusCode::NOT_FOUND, "not found"),
    }
}

/// T015 fix B1：/local-file 的查询参数（页面 localFile.ts 发的就是 ?path=，整段 URL 编码）。
#[derive(Debug, Deserialize)]
struct LocalFileQuery {
    #[serde(default)]
    path: Option<String>,
}

/// 单个本地文件的上限（8 MiB）：给回答里的产物「在浏览器打开」，不是通用文件服务器。
const LOCAL_FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// /local-file 的隔离 CSP（T015 安全修复，照 Jupyter 的 AuthenticatedFileHandler）：
/// 脚本照跑，但 HTML/SVG 进唯一的不透明来源——拿不到 AIOS 的 cookie/同源存储，调不通
/// /api，也 fetch 不到同目录其它文件；frame-ancestors 顺手挡住被别的页面嵌。
pub const LOCAL_FILE_CSP: &str = "frame-ancestors 'self'; sandbox allow-scripts";

/// 本地文件的 MIME：只按扩展名给浏览器能直接渲染的几类，其余按二进制下载。
/// 扩展名大小写不敏感（生成的文件常见 .HTML）。
fn local_file_content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("txt" | "md" | "markdown" | "log" | "csv") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// GET /local-file?path=…：把会话工作目录里的普通文件只读回给浏览器（T015 fix B1）。
///
/// 认证和 /api 走同一条栅栏：Host 必须是本机、Origin/Fetch-Metadata 必须同源、只认签名 cookie，
/// query 里出现 token 一律 401。路径边界：
/// - 只收绝对路径（相对路径 400；页面也只生成绝对路径）；
/// - canonicalize 之后再和 canonicalize 过的会话工作目录比前缀：..、符号链接跳出目录都 403；
/// - 目录、不存在、读不到一律 404（不透露目录外有什么）；超过 8 MiB 413。
///
/// 响应头：nosniff、no-store（产物可能含隐私，不让浏览器缓存），以及所有响应（含错误）都加的
/// LOCAL_FILE_CSP。外壳 CSP 仍只由 canvas_frame_handler 下发：两条用途不同，这里那串不限制
/// script-src，所以自包含 HTML 的行内脚本和 CDN 图表照跑，只是进 opaque origin。
async fn local_file_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    Query(query): Query<LocalFileQuery>,
) -> Response {
    let mut response = local_file_response(state, headers, uri, query).await;
    response.headers_mut().insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(LOCAL_FILE_CSP),
    );
    response
}

/// /local-file 的查询、路径边界与文件响应体；响应头的统一收口在 local_file_handler。
async fn local_file_response(
    state: Arc<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    query: LocalFileQuery,
) -> Response {
    if let Err(error) = authorize_api_request(&state.trust, &headers, &uri) {
        return trust_error_response(error);
    }
    let Some(requested) = query.path.as_deref() else {
        return error_response(StatusCode::BAD_REQUEST, "missing path");
    };
    if !requested.starts_with('/') {
        return error_response(StatusCode::BAD_REQUEST, "path must be absolute");
    }
    // /tmp 与 /private/tmp（macOS）这种同一个目录的两种写法由两边的 canonicalize 收束。
    let Ok(root) = tokio::fs::canonicalize(state.host.cwd()).await else {
        return error_response(StatusCode::NOT_FOUND, "not found");
    };
    let Ok(path) = tokio::fs::canonicalize(requested).await else {
        return error_response(StatusCode::NOT_FOUND, "not found");
    };
    if !path.starts_with(&root) {
        return error_response(StatusCode::FORBIDDEN, "forbidden");
    }
    let Ok(metadata) = tokio::fs::metadata(&path).await else {
        return error_response(StatusCode::NOT_FOUND, "not found");
    };
    if !metadata.is_file() {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    if metadata.len() > LOCAL_FILE_MAX_BYTES {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, local_file_content_type(&path)),
                (HeaderName::from_static("x-content-type-options"), "nosniff"),
                (HeaderName::from_static("cache-control"), "no-store"),
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
    // T015.1 fix：画布外壳只能由 canvas_frame_handler 出（它带 CSP）。静态 fallback
    // 在大小写不敏感的文件系统（macOS 的 APFS）上也会把 /CANVAS-FRAME.HTML 解析到
    // 这个文件，所以按文件名（不分大小写）拒绝，而不是只 route 小写路径。
    if is_canvas_frame_alias(&path) {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
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

pub(crate) fn trust_error_response(error: TrustError) -> Response {
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
    let call: service::Call = match serde_json::from_value(value.clone()) {
        Ok(call) => call,
        Err(error) => {
            return bad_request_response(format!("bad request: {error}"));
        }
    };
    // monitor-report 是 Call 的变体；监控记录留在 web（Monitor 在这里），
    // 不再在解码前用方法名字符串截走。它不依赖根会话（页面报错时根可能正好不可用）。
    if let service::Call::MonitorReport(report) = &call {
        record_monitor_report(&state, report);
    }
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

/// 页面没接住的异常、事件流重连记 ops（内容限长，不阻塞页面）。
fn record_monitor_report(state: &AppState, report: &service::MonitorReportRequest) {
    let op = match report.kind.as_deref() {
        Some("page-reconnect") => "page-reconnect",
        _ => "page-error",
    };
    let message: String = report
        .message
        .as_deref()
        .unwrap_or_default()
        .chars()
        .take(512)
        .collect();
    record_ops(
        state,
        op,
        if op == "page-error" {
            Severity::Warn
        } else {
            Severity::Info
        },
        vec![("message".to_owned(), serde_json::json!(message))],
    );
}

/// 一条 ops 记录（attributes 里带 telemetry.op；body 不写请求/响应内容）。
pub(crate) fn record_ops(
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

/// 由宿主和信任栅栏建 axum Router（HTTP /api + WS /ws）。
pub fn router(host: RootHost, trust: Trust, static_dir: Option<PathBuf>) -> Router {
    router_with_state(Arc::new(AppState {
        host: Arc::new(host),
        trust,
        static_dir,
        monitor: None,
    }))
}

pub(crate) fn router_with_state(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .route("/connect", get(connect_handler))
        .route("/api", post(api_handler))
        .route("/ws", get(ws_handler))
        .route("/canvas-frame", get(canvas_frame_handler))
        // T015.1 fix：web/public 里的同名文件会被静态 fallback 原样 200（无 CSP），
        // 和官方路径行为不一致；显式接住，让两条路径走同一个 handler。
        .route("/canvas-frame.html", get(canvas_frame_handler))
        // T015 fix B1：回答里本地绝对路径的只读出口（cookie + Host，只放会话工作目录）。
        .route("/local-file", get(local_file_handler))
        .fallback(get(static_handler))
        .with_state(state)
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
                provider: "deepseek".to_owned(),
                base_url: "http://127.0.0.1:1".to_owned(),
                model: "deepseek-flash".to_owned(),
                max_tokens: 2048,
                context_window: DEFAULT_CONTEXT_WINDOW,
                effort: Effort::new("high"),
                tool_update: None,
            },
            config: AgentConfig::default(),
            credentials: Some(std::sync::Arc::new(TestCredential)),
            identity: Some(identity.to_path_buf()),
            model_factory: None,
            append_hook_factory: None,
            keyring_changed: None,
            actions: std::sync::Arc::new(service::ActionRegistry::empty()),
            exa_base_url: None,
            web_listen: None,
            task_runtime: None,
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
    use crate::connection::StreamOps;
    use axum::body::Bytes;

    fn model_config() -> model::DeepSeekConfig {
        model::DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: model::DEFAULT_CONTEXT_WINDOW,
            effort: model::Effort::new("high"),
            tool_update: None,
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
            actions: std::sync::Arc::new(service::ActionRegistry::empty()),
            exa_base_url: None,
            web_listen: None,
            task_runtime: None,
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

    /// monitor-report 进 Call 后仍走 /api 一条路——限长后记 page-error，
    /// 同时照 T011.4 记一条 api-call（以前字符串截走时跳过了）。
    #[tokio::test]
    async fn monitor_report_is_a_call_and_records_page_error() {
        let (dir, state, headers) = setup().await;
        let response = call(
            &state,
            &headers,
            serde_json::json!({
                "method": "monitor-report",
                "kind": "page-error",
                "message": "x".repeat(600),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        assert_eq!(json["method"], "monitor-report");
        assert_eq!(json["ok"], true);
        drop(state);

        let mut text = String::new();
        for entry in std::fs::read_dir(dir.path().join("monitor")).expect("monitor 目录") {
            text.push_str(&std::fs::read_to_string(entry.expect("目录项").path()).expect("读监控"));
        }
        let records: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON"))
            .collect();
        let errors: Vec<&serde_json::Value> = records
            .iter()
            .filter(|record| record["attributes"]["telemetry.op"] == "page-error")
            .collect();
        assert_eq!(errors.len(), 1, "一条 page-error：{text}");
        let message = errors[0]["attributes"]["message"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(message.chars().count(), 512, "超长消息截到 512 字");
        let api_calls: Vec<&serde_json::Value> = records
            .iter()
            .filter(|record| {
                record["attributes"]["telemetry.op"] == "api-call"
                    && record["attributes"]["method"] == "monitor-report"
            })
            .collect();
        assert_eq!(
            api_calls.len(),
            1,
            "monitor-report 也记一条 api-call：{text}"
        );
    }
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

    /// T015.1 fix：静态 fallback 不分大小写都不许直接发画布外壳（它必须带 CSP）。
    #[test]
    fn static_fallback_refuses_canvas_frame_shell_under_any_case() {
        assert!(is_canvas_frame_alias(Path::new("/srv/canvas-frame.html")));
        assert!(is_canvas_frame_alias(Path::new("/srv/CANVAS-FRAME.HTML")));
        assert!(is_canvas_frame_alias(Path::new("./Canvas-Frame.Html")));
        assert!(!is_canvas_frame_alias(Path::new("/srv/index.html")));
        assert!(!is_canvas_frame_alias(Path::new(
            "/srv/canvas-frame.html.bak"
        )));
    }
}

#[cfg(test)]
mod t015_local_file_tests {
    use super::*;
    use std::net::IpAddr;

    fn model_config() -> model::DeepSeekConfig {
        model::DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: model::DEFAULT_CONTEXT_WINDOW,
            effort: model::Effort::new("high"),
            tool_update: None,
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

    struct Fixture {
        dir: tempfile::TempDir,
        work: PathBuf,
        state: Arc<AppState>,
        headers: HeaderMap,
        trust: Trust,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("临时目录");
        let work = dir.path().join("work");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&work).expect("工作目录");
        std::fs::create_dir_all(&data).expect("数据目录");
        let options = service::RootSessionOptions {
            data_dir: data,
            cwd: work.clone(),
            model: model_config(),
            config: agent::AgentConfig::default(),
            credentials: None,
            identity: Some(dir.path().join("identity.txt")),
            model_factory: None,
            append_hook_factory: None,
            keyring_changed: None,
            actions: std::sync::Arc::new(service::ActionRegistry::empty()),
            exa_base_url: None,
            web_listen: None,
            task_runtime: None,
        };
        let host = service::RootHost::open(options, model::DEFAULT_CONTEXT_WINDOW).await;
        let trust = Trust::new(
            IpAddr::from([127, 0, 0, 1]),
            1417,
            "tok-1".to_owned(),
            [7u8; 32],
        );
        let headers = auth_headers(&trust);
        let state = Arc::new(AppState {
            host: Arc::new(host),
            trust: trust.clone(),
            static_dir: None,
            monitor: None,
        });
        Fixture {
            dir,
            work,
            state,
            headers,
            trust,
        }
    }

    async fn call(
        state: &Arc<AppState>,
        headers: &HeaderMap,
        uri: &str,
        path: Option<&str>,
    ) -> Response {
        local_file_handler(
            State(Arc::clone(state)),
            headers.clone(),
            uri.parse().expect("uri"),
            Query(LocalFileQuery {
                path: path.map(str::to_owned),
            }),
        )
        .await
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf8")
    }

    #[tokio::test]
    async fn serves_html_inside_work_dir_without_caching() {
        let fixture = fixture().await;
        let file = fixture.work.join("answer.html");
        std::fs::write(&file, "<!doctype html><h1>本地文件正文</h1>").expect("写文件");
        let response = call(
            &fixture.state,
            &fixture.headers,
            "/local-file",
            Some(file.to_str().expect("路径")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).expect("content-type"),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            response
                .headers()
                .get("x-content-type-options")
                .expect("nosniff"),
            "nosniff"
        );
        assert_eq!(
            response.headers().get("cache-control").expect("cache"),
            "no-store"
        );
        assert!(body_text(response).await.contains("本地文件正文"));
    }

    #[tokio::test]
    async fn serves_path_with_space_and_chinese() {
        let fixture = fixture().await;
        let file = fixture.work.join("报告 第 1 版.txt");
        std::fs::write(&file, "带空格和中文的文件").expect("写文件");
        let response = call(
            &fixture.state,
            &fixture.headers,
            "/local-file",
            Some(file.to_str().expect("路径")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).expect("content-type"),
            "text/plain; charset=utf-8"
        );
        assert_eq!(body_text(response).await, "带空格和中文的文件");
    }

    #[tokio::test]
    async fn rejects_outside_directory_missing_relative_and_missing_param() {
        let fixture = fixture().await;
        let outside = fixture.dir.path().join("outside.html");
        std::fs::write(&outside, "outside").expect("写文件");
        let nested = fixture.work.join("nested");
        std::fs::create_dir_all(&nested).expect("目录");
        let escape = fixture.work.join("../outside.html");
        let missing = fixture.work.join("missing.html");
        let cases = [
            (Some(outside.to_str().expect("路径")), StatusCode::FORBIDDEN),
            (Some(escape.to_str().expect("路径")), StatusCode::FORBIDDEN),
            (Some(nested.to_str().expect("路径")), StatusCode::NOT_FOUND),
            (Some(missing.to_str().expect("路径")), StatusCode::NOT_FOUND),
            (Some("answer.html"), StatusCode::BAD_REQUEST),
            (None, StatusCode::BAD_REQUEST),
        ];
        for (path, expected) in cases {
            let response = call(&fixture.state, &fixture.headers, "/local-file", path).await;
            assert_eq!(response.status(), expected, "path={path:?}");
        }
    }

    #[tokio::test]
    async fn rejects_external_host_header() {
        let fixture = fixture().await;
        let file = fixture.work.join("answer.html");
        std::fs::write(&file, "<h1>x</h1>").expect("写文件");
        let mut headers = fixture.headers.clone();
        headers.insert(HOST, "evil.example:1417".parse().expect("Host"));
        let response = call(
            &fixture.state,
            &headers,
            "/local-file",
            Some(file.to_str().expect("路径")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_file_is_404() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = fixture().await;
        let file = fixture.work.join("unreadable.html");
        std::fs::write(&file, "<h1>secret</h1>").expect("写文件");
        let permissions = std::fs::metadata(&file).expect("metadata").permissions();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).expect("chmod 000");
        // root（或忽略权限位的文件系统）读得到 000 文件，这条用例没有可断言的拒绝面：跳过。
        let denied = std::fs::read(&file).is_err();
        let response = call(
            &fixture.state,
            &fixture.headers,
            "/local-file",
            Some(file.to_str().expect("路径")),
        )
        .await;
        std::fs::set_permissions(&file, permissions).expect("恢复权限");
        if denied {
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn rejects_symlink_escape() {
        let fixture = fixture().await;
        let outside = fixture.dir.path().join("outside.html");
        std::fs::write(&outside, "outside").expect("写文件");
        let link = fixture.work.join("link.html");
        std::os::unix::fs::symlink(&outside, &link).expect("建符号链接");
        let response = call(
            &fixture.state,
            &fixture.headers,
            "/local-file",
            Some(link.to_str().expect("路径")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn requires_cookie_and_rejects_query_token() {
        let fixture = fixture().await;
        let file = fixture.work.join("answer.html");
        std::fs::write(&file, "<h1>x</h1>").expect("写文件");
        let path = file.to_str().expect("路径");
        let mut host_only = HeaderMap::new();
        host_only.insert(HOST, "127.0.0.1:1417".parse().expect("Host"));
        let response = call(&fixture.state, &host_only, "/local-file", Some(path)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = call(
            &fixture.state,
            &fixture.headers,
            "/local-file?token=tok-1",
            Some(path),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        // 同源 cookie + Host 齐全时通过（前面两个用例已覆盖 200，这里确认带 token 的 query 才被拒）。
        let _ = fixture.trust;
    }

    #[tokio::test]
    async fn rejects_oversized_file() {
        let fixture = fixture().await;
        let file = fixture.work.join("big.bin");
        std::fs::write(&file, vec![b'x'; LOCAL_FILE_MAX_BYTES as usize + 1]).expect("写文件");
        let response = call(
            &fixture.state,
            &fixture.headers,
            "/local-file",
            Some(file.to_str().expect("路径")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// T015 安全修复：/local-file 给每种文件类型都带同一条隔离 CSP（照 Jupyter 的
    /// AuthenticatedFileHandler；不按扩展名分），原来的 nosniff/no-store 不丢。
    #[tokio::test]
    async fn file_responses_carry_isolation_csp_for_all_types() {
        let fixture = fixture().await;
        for (name, body, content_type) in [
            (
                "answer.html",
                "<!doctype html><p>x</p>",
                "text/html; charset=utf-8",
            ),
            (
                "shape.svg",
                "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>x</text></svg>",
                "image/svg+xml",
            ),
            ("notes.txt", "text", "text/plain; charset=utf-8"),
            ("pixel.png", "\u{89}PNG\r\n\u{1a}\n", "image/png"),
            ("doc.pdf", "%PDF-1.4\n%%EOF\n", "application/pdf"),
        ] {
            let file = fixture.work.join(name);
            std::fs::write(&file, body).expect("写文件");
            let response = call(
                &fixture.state,
                &fixture.headers,
                "/local-file",
                Some(file.to_str().expect("路径")),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{name}");
            assert_eq!(
                response.headers().get(CONTENT_TYPE).expect("content-type"),
                content_type,
                "{name}"
            );
            assert_eq!(
                csp_of(&response),
                "frame-ancestors 'self'; sandbox allow-scripts",
                "{name}"
            );
            assert_eq!(
                response
                    .headers()
                    .get("x-content-type-options")
                    .expect("nosniff"),
                "nosniff",
                "{name}"
            );
            assert_eq!(
                response.headers().get("cache-control").expect("no-store"),
                "no-store",
                "{name}"
            );
        }
    }

    /// 同一个处理器的错误响应也带 CSP：认证失败、路径不存在都不能绕开这条响应策略。
    #[tokio::test]
    async fn rejection_responses_carry_isolation_csp() {
        let fixture = fixture().await;
        let missing = fixture.work.join("missing.html");
        let response = call(
            &fixture.state,
            &fixture.headers,
            "/local-file",
            Some(missing.to_str().expect("路径")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            csp_of(&response),
            "frame-ancestors 'self'; sandbox allow-scripts"
        );

        let mut host_only = HeaderMap::new();
        host_only.insert(HOST, "127.0.0.1:1417".parse().expect("Host"));
        let response = call(
            &fixture.state,
            &host_only,
            "/local-file",
            Some(missing.to_str().expect("路径")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            csp_of(&response),
            "frame-ancestors 'self'; sandbox allow-scripts"
        );
    }

    fn csp_of(response: &Response) -> &str {
        response
            .headers()
            .get(HeaderName::from_static("content-security-policy"))
            .expect("CSP")
            .to_str()
            .expect("CSP 文本")
    }

    #[test]
    fn content_type_follows_extension() {
        for (name, expected) in [
            ("a.html", "text/html; charset=utf-8"),
            ("a.HTML", "text/html; charset=utf-8"),
            ("a.css", "text/css; charset=utf-8"),
            ("a.js", "text/javascript; charset=utf-8"),
            ("a.txt", "text/plain; charset=utf-8"),
            ("a.md", "text/plain; charset=utf-8"),
            ("a.json", "application/json"),
            ("a.png", "image/png"),
            ("a.bin", "application/octet-stream"),
        ] {
            assert_eq!(local_file_content_type(Path::new(name)), expected, "{name}");
        }
    }
}
