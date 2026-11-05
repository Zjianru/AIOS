//! T012.3 R2：CLI 的 --effort 收任意非空字符串（模型档位在发送时适配）。

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn arbitrary_effort_is_sent() {
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
            ..Default::default()
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
            "weird-vendor-level",
            "你好",
        ])
        .output()
        .expect("运行 aios-dev");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        server.received_requests().await.expect("请求记录").len(),
        1,
        "任意非空档都要发出请求；exit={:?} stderr={stderr}",
        output.status.code()
    );
}
