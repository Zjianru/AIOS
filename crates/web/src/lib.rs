//! T006.17：Web 薄适配器（axum HTTP + WebSocket）与信任栅栏（接缝 4）。
//!
//! 载体无关的宿主在 crates/service；这里只做线上形状：
//! - HTTP：POST /api，body 是 service::Call 的 JSON，回 service::Response（或错误 JSON）；
//! - WebSocket：GET /ws?token=..&lastSeq=..，先发 ready，再按 lastSeq 补齐事件，然后发 live 事件；
//! - 信任栅栏照 DSH（client/connection README 第 41、47 行；host/webserver README 第 39 行）：
//!   只绑本机回环地址（0.0.0.0/非回环一律拒绝启动）、每个进程一个启动令牌、Host 头只认本机
//!   （回环地址/localhost/绑定 IP 字面量）；失败 403（Host 不对）/401（无令牌或令牌不对）。
//!
//! DSH 对照：ready 项形状 {type:'ready', clientId, host:{home}}、事件在 ready 之后、
//! 重连带最后序号重新跟随（connection README「Connection generation」节、session-controller
//! follow :607）；AIOS 第一版 ready 带 sessionId、事件帧是 {type:'event', event:{seq,type,data}}。

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use serde::{Deserialize, Serialize};
use service::{FollowRequest, FollowResponse, ServiceEvent, ServiceHost};

/// 默认只绑 IPv4 回环。
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// 默认端口 0：由系统分配（测试/开发都从 ServeHandle 读实际端口）。
pub const DEFAULT_PORT: u16 = 0;

/// 信任栅栏的配置：绑定地址 + 本进程启动令牌。
#[derive(Debug, Clone)]
pub struct Trust {
    pub bind_ip: IpAddr,
    pub bind_port: u16,
    pub token: String,
}

impl Trust {
    pub fn new(bind_ip: IpAddr, bind_port: u16, token: String) -> Self {
        Self {
            bind_ip,
            bind_port,
            token,
        }
    }
}

/// 信任栅栏的拒绝原因；HTTP 面上 Host 不对 403、令牌不对 401。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TrustError {
    #[error("不支持的绑定地址：{0}（只允许本机回环地址，拒绝 0.0.0.0/非回环）")]
    UnsupportedBind(String),
    #[error("Host 头不是本机：{0}")]
    BadHost(String),
    #[error("缺少或错误的启动令牌")]
    Unauthorized,
    #[error("信任栅栏尚未实现（骨架）")]
    NotImplemented,
}

impl TrustError {
    /// 线上的状态码：Host/Origin 信任失败 403，令牌失败 401。
    pub fn status(&self) -> u16 {
        match self {
            Self::BadHost(_) | Self::UnsupportedBind(_) => 403,
            Self::Unauthorized => 401,
            Self::NotImplemented => 500,
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
    #[error("Web 适配器尚未实现（骨架）")]
    NotImplemented,
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
}

/// 起好服务后拿到的句柄（端口可能是系统分配的，测试从 addr 读）。
pub struct ServeHandle {
    pub addr: SocketAddr,
    pub token: String,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl ServeHandle {
    /// 停服务（测试用；T006.18 会接上正在跑一轮的取消收尾）。
    pub fn abort(self) {
        self.task.abort();
    }
}

/// 线上帧：ready 必须先于事件；事件帧包着 service 的日志事件（seq/type/data 来自日志事实）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Frame {
    /// 连接建立后的第一帧（DSH 的 {type:'ready',...}）。
    Ready {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    /// 日志事件帧（DSH 的 {type:'event', event: SessionWireEvent}）。
    Event { event: ServiceEvent },
}

/// 本进程的启动令牌（每个进程一个；uuid v4，不落盘、不进日志）。
pub fn launch_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 绑定地址只允许本机回环：0.0.0.0/::/非回环字面量都拒绝启动（DSH webserver README 第 39 行）。
pub fn check_bind_host(_host: &str) -> Result<IpAddr, TrustError> {
    Err(TrustError::NotImplemented)
}

/// 校验一次请求：Host 头必须是本机（回环/localhost/绑定 IP），令牌必须匹配。
pub fn check_request(
    _trust: &Trust,
    _host_header: Option<&str>,
    _token: Option<&str>,
) -> Result<(), TrustError> {
    Err(TrustError::NotImplemented)
}

/// 打开连接时的帧序列：ready 第一帧，然后按 lastSeq 补齐的事件（重连带序号）。
pub fn opening_frames(_session_id: &str, _follow: FollowResponse) -> Vec<Frame> {
    Vec::new()
}

/// 由宿主和信任栅栏建 axum Router（HTTP /api + WS /ws）。
pub fn router(_host: ServiceHost, _trust: Trust) -> Router {
    Router::new()
}

/// 起服务：校验绑定地址 → TcpListener → axum::serve。
pub async fn serve(_options: ServeOptions, _host: ServiceHost) -> Result<ServeHandle, ServeError> {
    Err(ServeError::NotImplemented)
}

/// 连接内事件流（骨架占位；实现里是 WS 的 on_upgrade 循环）。
pub async fn event_stream(_host: Arc<ServiceHost>, _last_seq: u64) -> FollowResponse {
    FollowResponse {
        events: Vec::new(),
        last_seq: 0,
    }
}

/// 发消息需要的 follow 请求（供适配器使用，避免上层直接拼类型）。
pub fn follow_request(last_seq: u64) -> FollowRequest {
    FollowRequest { last_seq }
}
