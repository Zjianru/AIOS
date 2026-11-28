//! status 带会话的工作目录：页面拿它把工作过程里的文件路径显示成相对路径。

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
    // 产品路径保持传入时的写法（不替用户 canonicalize/建目录）；这里只核
    // status 带回的确实就是根会话的 cwd，工具侧自己按同一路径解析。
    let expected = data.to_str().expect("数据目录是 UTF-8");
    assert_eq!(
        status.cwd.as_deref(),
        Some(expected),
        "status 要带回根会话的工作目录（不替产品改写路径）"
    );
}
