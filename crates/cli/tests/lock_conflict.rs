//! T006.18 红测试：两进程写锁冲突——一个进程持有会话写锁时，aios-dev run --resume 必须被拒，
//! 且错误码可辨（DSH 原文：`session/writer-held`，packages/api/session-controller/src/agent.ts:209-211
//! 把 SessionAlreadyOwnedError 映射成该错误码；details {sessionId}）。
//!
//! 这是真两进程测试：测试进程持有 SessionStore 写锁，子进程起 aios-dev run --resume。

use std::process::{Command, Stdio};

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn aios_dev() -> &'static str {
    env!("CARGO_BIN_EXE_aios-dev")
}

fn seed_keyring(data_dir: &std::path::Path, endpoint: &str) -> std::path::PathBuf {
    let identity = data_dir.join("identity.txt");
    let keyring = credentials::Keyring::open(data_dir, Some(&identity)).expect("keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: endpoint.to_owned(),
            api_key: "test-key".to_owned(),
        })
        .expect("save connection");
    identity
}

fn run_resume(
    data_dir: &std::path::Path,
    cwd: &std::path::Path,
    id: &str,
    identity: &std::path::Path,
) -> std::process::Output {
    Command::new(aios_dev())
        .args([
            "run",
            "--resume",
            id,
            "--data-dir",
            data_dir.to_str().expect("data dir 是 UTF-8"),
            "--identity",
            identity.to_str().expect("identity 是 UTF-8"),
            "--cwd",
            cwd.to_str().expect("cwd 是 UTF-8"),
            "你好",
        ])
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .stdin(Stdio::null())
        .output()
        .expect("aios-dev 子进程应能启动")
}

#[test]
fn busy_session_is_rejected_with_writer_held_code() {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 测试进程先持有写锁，模拟「服务开着 Kind 会话」。
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let identity = seed_keyring(data_dir.path(), "http://127.0.0.1:1");

    let output = run_resume(data_dir.path(), cwd.path(), id.as_str(), &identity);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "被占用必须是独立退出码 2（忙碌），实际 stderr：{stderr}"
    );
    assert!(
        stderr.contains("session/writer-held"),
        "stderr 要带 DSH 的占用错误码 session/writer-held：{stderr}"
    );
    assert!(
        stderr.contains(id.as_str()),
        "错误里要能看出是哪个会话被占用：{stderr}"
    );
}

#[tokio::test]
async fn released_lock_allows_the_session_again() {
    // 锁释放后子进程会真发请求：wiremock 立刻回 401（不重试），避免连接退避的 15 秒。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({"error": {"message": "bad key", "type": "authentication_error"}}),
        ))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    drop(store); // 锁释放：第二个进程应能打开（然后模型请求被拒，而不是占用）。
    let identity = seed_keyring(data_dir.path(), &server.uri());

    let output = run_resume(data_dir.path(), cwd.path(), id.as_str(), &identity);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_ne!(
        output.status.code(),
        Some(2),
        "锁已释放，不应再报占用：{stderr}"
    );
    assert!(
        !stderr.contains("session/writer-held"),
        "锁已释放，不应出现占用错误码：{stderr}"
    );
}
