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

mod connection;
mod routes;
mod trust;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use monitor::Monitor;
use service::RootHost;

use crate::routes::{AppState, router_with_state};

pub use connection::{AgentEventWire, Frame, opening_frames};
pub use routes::{CANVAS_FRAME_CSP, LOCAL_FILE_CSP, router};
pub use trust::{
    ConnectGrant, Trust, TrustError, authorize_api_request, authorize_connect, check_api_auth,
    check_bind_host, check_origin, reject_query_token,
};

/// 默认只绑 IPv4 回环。
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// 默认端口 0：由系统分配（测试/开发都从 ServeHandle 读实际端口）。
pub const DEFAULT_PORT: u16 = 0;

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
            #[expect(
                clippy::let_underscore_must_use,
                reason = "关闭信号接收端已退出说明服务已停；继续等任务收尾"
            )]
            let _ = signal.send(());
        }
        #[expect(
            clippy::let_underscore_must_use,
            reason = "任务结果在 shutdown 不再使用；等它结束即可"
        )]
        let _ = self.task.await;
    }

    /// 立刻停（测试用；不走取消收尾）。
    pub fn abort(self) {
        self.task.abort();
    }
}

/// 本进程的启动令牌（每个进程一个；uuid v4，不落盘、不进日志）。
pub fn launch_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 起服务：校验绑定地址 → TcpListener → axum::serve。
pub async fn serve(options: ServeOptions, host: RootHost) -> Result<ServeHandle, ServeError> {
    let ip = check_bind_host(&options.host)?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(ip, options.port)).await?;
    let addr = listener.local_addr()?;
    // web_fetch 要知道 AIOS 自己监听在哪，读网页时拒绝读自己。
    host.record_listen_address(addr);
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
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "关闭信号发送端已退出等价于立刻关闭"
                )]
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
