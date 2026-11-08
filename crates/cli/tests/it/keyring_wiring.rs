//! T008.2：CLI 不再读 DEEPSEEK_API_KEY；连接来自 Keyring。
//! T008.F10（review #10）：补反证——env 里设了 key 也不用、也不签发。

use std::process::{Command, Output};

use serde_json::json;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::aios_dev;

fn run_with_env(
    data_dir: &std::path::Path,
    identity: &std::path::Path,
    env_api_key: Option<&str>,
) -> Output {
    let home = tempfile::tempdir().expect("隔离 HOME");
    let mut command = Command::new(aios_dev());
    command
        .env("HOME", home.path())
        .args([
            "run",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--identity",
            identity.to_str().unwrap(),
            "--cwd",
            ".",
            "你好",
        ])
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("DEEPSEEK_BASE_URL")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY");
    if let Some(key) = env_api_key {
        command.env("DEEPSEEK_API_KEY", key);
    }
    command.output().expect("运行 aios-dev")
}

fn run(data_dir: &std::path::Path, identity: &std::path::Path) -> Output {
    run_with_env(data_dir, identity, None)
}

#[test]
fn 没有连接时提示接模型而不是环境变量() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().join("data");
    let identity = dir.path().join("identity.txt");
    let output = run(&data_dir, &identity);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "没有连接时必须失败");
    assert!(
        !stderr.contains("DEEPSEEK_API_KEY"),
        "不能提示环境变量：{stderr}"
    );
    assert!(
        stderr.contains("接模型") || stderr.contains("Keyring"),
        "stderr={stderr}"
    );
}

#[tokio::test]
async fn 连接存在时不再要求环境变量() {
    // 连接存在时子进程会真发请求：wiremock 立刻回 401（不重试），避免连接退避的 15 秒。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({"error": {"message": "bad key", "type": "authentication_error"}}),
        ))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().join("data");
    let identity = dir.path().join("identity.txt");
    let keyring = credentials::Keyring::open(&data_dir, Some(&identity)).expect("keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: "test-key".to_owned(),
            ..Default::default()
        })
        .expect("save");
    let output = run(&data_dir, &identity);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("DEEPSEEK_API_KEY"),
        "不能要求环境变量：{stderr}"
    );
    assert!(
        !stderr.contains("还没有接模型"),
        "连接存在不应说没接模型：{stderr}"
    );
}

const ENV_KEY: &str = "env-key-should-not-be-used";
const KEYRING_KEY: &str = "sk-keyring-f10";

#[test]
fn 环境变量有值但没有连接时仍提示接模型() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().join("data");
    let identity = dir.path().join("identity.txt");
    let output = run_with_env(&data_dir, &identity, Some(ENV_KEY));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "env 里有 key 也不能替代 Keyring 连接"
    );
    assert!(
        !stderr.contains(ENV_KEY),
        "env key 不能进入错误输出：{stderr}"
    );
    assert!(
        !stderr.contains("DEEPSEEK_API_KEY"),
        "env key 不能出现在提示里：{stderr}"
    );
    assert!(
        stderr.contains("接模型") || stderr.contains("Keyring"),
        "stderr={stderr}"
    );
    // env 有值也不签发：Keyring 没有连接时不能写 issue 记录。
    let keyring = credentials::Keyring::open(&data_dir, Some(&identity)).expect("keyring");
    assert!(
        keyring.issues().is_empty(),
        "env key 不能触发 Keyring 签发记录：{:?}",
        keyring.issues()
    );
}

#[tokio::test]
async fn 环境变量有值也不覆盖_keyring_的_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().join("data");
    let identity = dir.path().join("identity.txt");
    let keyring = credentials::Keyring::open(&data_dir, Some(&identity)).expect("keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.uri(),
            api_key: KEYRING_KEY.to_owned(),
            ..Default::default()
        })
        .expect("save");
    let output = run_with_env(&data_dir, &identity, Some(ENV_KEY));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(ENV_KEY),
        "env key 不能进入错误输出：{stderr}"
    );
    assert!(
        !stderr.contains("DEEPSEEK_API_KEY"),
        "env key 不能出现在提示里：{stderr}"
    );
    let requests = server.received_requests().await.expect("请求记录");
    assert!(!requests.is_empty(), "连接存在时必须发起模型请求");
    let header = requests[0]
        .headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert_eq!(header, KEYRING_KEY, "发出去的 key 必须来自 Keyring");
    let keyring = credentials::Keyring::open(&data_dir, Some(&identity)).expect("keyring");
    assert!(
        keyring
            .issues()
            .iter()
            .any(|issue| issue.purpose == "model-request" && issue.connection == "deepseek"),
        "签发记录属于 Keyring 连接：{:?}",
        keyring.issues()
    );
}
