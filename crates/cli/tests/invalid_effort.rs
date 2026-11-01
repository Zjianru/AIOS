//! CLI 在发请求之前拒绝非法推理强度（测试清单第 11 条）。

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn invalid_effort_is_rejected_before_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let data_dir = std::env::temp_dir().join(format!("aios-cli-effort-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_aios-dev"))
        .args([
            "run",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--cwd",
            ".",
            "--effort",
            "bogus",
            "你好",
        ])
        .env("DEEPSEEK_API_KEY", "test-key")
        .env("DEEPSEEK_BASE_URL", server.uri())
        .output()
        .expect("运行 aios-dev");
    assert!(!output.status.success(), "非法推理强度必须非 0 退出");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("推理强度"), "stderr={stderr}");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "非法推理强度不能发请求"
    );
}
