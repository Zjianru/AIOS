//! CLI 在发请求之前拒绝空推理强度（测试清单第 11 条）。

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn empty_effort_is_rejected_before_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().expect("tempdir");
    let identity = data_dir.path().join("identity.txt");
    let keyring = credentials::Keyring::open(data_dir.path(), Some(&identity)).expect("keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: "test-key".to_owned(),
        })
        .expect("save connection");

    let home = tempfile::tempdir().expect("隔离 HOME");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_aios-dev"))
        .env("HOME", home.path())
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .args([
            "run",
            "--data-dir",
            data_dir.path().to_str().unwrap(),
            "--identity",
            identity.to_str().unwrap(),
            "--cwd",
            ".",
            "--effort",
            "",
            "你好",
        ])
        .output()
        .expect("运行 aios-dev");
    assert!(!output.status.success(), "空推理强度必须非 0 退出");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("推理强度"), "stderr={stderr}");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "空推理强度不能发请求"
    );
}
