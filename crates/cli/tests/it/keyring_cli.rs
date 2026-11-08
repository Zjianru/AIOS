use std::io::Write;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

use crate::common::aios_dev;

const TEST_KEY: &str = "sk-cli-test-0123456789";

fn run(args: &[&str], stdin: &str) -> Output {
    let home = tempfile::tempdir().expect("隔离 HOME");
    let mut child = Command::new(aios_dev())
        .env("HOME", home.path())
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aios-dev");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait")
}

fn paths(dir: &TempDir) -> (String, String) {
    (
        dir.path().join("data").display().to_string(),
        dir.path().join("identity.txt").display().to_string(),
    )
}

#[test]
fn 从标准输入存_key_列出不含正文() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (data, identity) = paths(&dir);
    let set = run(
        &[
            "keyring",
            "--data-dir",
            &data,
            "--identity",
            &identity,
            "set",
            "--id",
            "deepseek",
            "--name",
            "DeepSeek",
            "--model",
            "deepseek-flash",
            "--endpoint",
            "https://api.deepseek.com",
        ],
        &format!("{TEST_KEY}\n"),
    );
    assert!(set.status.success(), "存 key 应成功：{set:?}");

    let list = run(
        &[
            "keyring",
            "--data-dir",
            &data,
            "--identity",
            &identity,
            "list",
        ],
        "",
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(list.status.success(), "list 应成功：{list:?}");
    assert!(stdout.contains("DeepSeek"), "list 应列出连接：{stdout}");
    assert!(
        stdout.contains("deepseek-flash"),
        "list 应列出模型：{stdout}"
    );
    assert!(
        !stdout.contains(TEST_KEY),
        "list 不能出现 key 正文：{stdout}"
    );
    assert!(
        !String::from_utf8_lossy(&list.stderr).contains(TEST_KEY),
        "stderr 也不能出现 key"
    );
}

#[test]
fn 换位置_删除_看记录() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (data, identity) = paths(&dir);
    let set = run(
        &[
            "keyring",
            "--data-dir",
            &data,
            "--identity",
            &identity,
            "set",
            "--id",
            "deepseek",
            "--name",
            "DeepSeek",
            "--model",
            "deepseek-flash",
            "--endpoint",
            "https://api.deepseek.com",
        ],
        &format!("{TEST_KEY}\n"),
    );
    assert!(set.status.success(), "存 key 应成功：{set:?}");

    let moved = dir.path().join("moved-identity.txt");
    let relocated = run(
        &[
            "keyring",
            "--data-dir",
            &data,
            "--identity",
            &identity,
            "relocate",
            &moved.display().to_string(),
        ],
        "",
    );
    assert!(relocated.status.success(), "换位置应成功：{relocated:?}");
    assert!(moved.exists(), "新位置应有钥匙文件");
    assert!(!std::path::Path::new(&identity).exists(), "旧位置不应留着");

    let issues = run(
        &[
            "keyring",
            "--data-dir",
            &data,
            "--identity",
            &identity,
            "issues",
        ],
        "",
    );
    assert!(issues.status.success(), "签发记录应成功：{issues:?}");
    let stdout = String::from_utf8_lossy(&issues.stdout);
    assert!(!stdout.contains(TEST_KEY), "签发记录不能出现 key：{stdout}");

    let deleted = run(
        &[
            "keyring",
            "--data-dir",
            &data,
            "--identity",
            &identity,
            "delete",
            "deepseek",
        ],
        "",
    );
    assert!(deleted.status.success(), "删除应成功：{deleted:?}");
    let list = run(
        &[
            "keyring",
            "--data-dir",
            &data,
            "--identity",
            &identity,
            "list",
        ],
        "",
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(!stdout.contains("DeepSeek"), "删除后不应再列出：{stdout}");
}

#[test]
fn 命令行给_key_被拒() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (data, identity) = paths(&dir);
    let output = run(
        &[
            "keyring",
            "--data-dir",
            &data,
            "--identity",
            &identity,
            "set",
            "--id",
            "deepseek",
            "--name",
            "DeepSeek",
            "--model",
            "deepseek-flash",
            "--endpoint",
            "https://api.deepseek.com",
            "--key",
            TEST_KEY,
        ],
        "",
    );
    assert!(
        !output.status.success(),
        "给 --key 参数必须失败：{output:?}"
    );
}
