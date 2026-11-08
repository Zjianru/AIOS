//! status 带会话的工作目录：页面拿它把工作过程里的文件路径显示成相对路径。

use std::fs;

use model::DeepSeekConfig;
use service::{Call, Response, RootHost, StatusRequest};
use tempfile::tempdir;

#[tokio::test]
async fn status_carries_session_cwd() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    let mut options = crate::common::options(&data);
    options.model = DeepSeekConfig::default();
    let host = RootHost::open(options, 1_000).await;
    let response = host
        .dispatch(Call::Status(StatusRequest::default()))
        .await
        .expect("根会话可用时 status 不能被拒绝");
    let Response::Status(status) = response else {
        panic!("status 必须回 StatusResponse");
    };
    let expected = fs::canonicalize(&data).expect("数据目录");
    assert_eq!(
        status.cwd.as_deref(),
        Some(expected.to_str().expect("数据目录是 UTF-8")),
        "页面要用与工具解析同一份工作目录来相对化路径"
    );
}
