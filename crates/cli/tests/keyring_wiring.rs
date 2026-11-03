//! T008.2：CLI 不再读 DEEPSEEK_API_KEY；连接来自 Keyring。

use std::process::Command;

fn aios_dev() -> &'static str {
    env!("CARGO_BIN_EXE_aios-dev")
}

fn run(data_dir: &std::path::Path, identity: &std::path::Path) -> std::process::Output {
    Command::new(aios_dev())
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
        .output()
        .expect("运行 aios-dev")
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
    assert!(stderr.contains("接模型") || stderr.contains("Keyring"), "stderr={stderr}");
}

#[test]
fn 连接存在时不再要求环境变量() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().join("data");
    let identity = dir.path().join("identity.txt");
    let keyring = credentials::Keyring::open(&data_dir, Some(&identity)).expect("keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "http://127.0.0.1:1".to_owned(),
            api_key: "test-key".to_owned(),
        })
        .expect("save");
    let output = run(&data_dir, &identity);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("DEEPSEEK_API_KEY"), "不能要求环境变量：{stderr}");
    assert!(!stderr.contains("还没有接模型"), "连接存在不应说没接模型：{stderr}");
}
