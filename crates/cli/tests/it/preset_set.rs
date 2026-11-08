//! T012.4：CLI keyring set --preset 用预设地址，连接写成新形状。

use crate::common::aios_dev;

#[test]
fn preset_set_writes_new_shape() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let identity = data_dir.path().join("identity.txt");
    let home = tempfile::tempdir().expect("隔离 HOME");
    let output = std::process::Command::new(aios_dev())
        .env("HOME", home.path())
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .args([
            "keyring",
            "--data-dir",
            data_dir.path().to_str().unwrap(),
            "--identity",
            identity.to_str().unwrap(),
            "set",
            "--preset",
            "minimax-cn",
            "--id",
            "minimax",
            "--name",
            "MiniMax",
            "--model",
            "MiniMax-M3.1-Flash",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    output
        .stdin
        .as_ref()
        .expect("stdin")
        .write_all(b"test-key\n")
        .expect("write key");
    let output = output.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let keyring = credentials::Keyring::open(data_dir.path(), Some(&identity)).expect("keyring");
    let view = keyring.view();
    let connection = &view.connections[0];
    assert_eq!(connection.preset, "minimax-cn");
    assert_eq!(
        connection.endpoint, "https://api.minimaxi.com/anthropic",
        "预设地址"
    );
    assert_eq!(
        connection
            .models
            .as_deref()
            .and_then(|models| models.first())
            .map(|row| row.id.as_str()),
        Some("MiniMax-M3.1-Flash")
    );
    assert_eq!(connection.primary_model(), "MiniMax-M3.1-Flash");
}
