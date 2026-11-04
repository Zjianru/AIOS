use std::net::SocketAddr;

use crate::{ReplayError, ReplayScript};

/// 起好的回放服务（地址 + 后台任务）。
pub struct ReplayServer {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl ReplayServer {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn shutdown(self) {
        self.task.abort();
    }
}

/// 起一个本机回放服务（skeleton：先返回未实现；红测试后实现）。
pub async fn start_replay_server(_script: ReplayScript) -> Result<ReplayServer, ReplayError> {
    Err(ReplayError::Format("回放服务还没实现".to_owned()))
}
