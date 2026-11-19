//! T068.3：serve 收到 SIGTERM 时把任务后台命令一起收掉（进程级测试）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::common::aios_dev;

const THIN_FIXTURE: &str = include_str!("../../../../web/e2e/fixtures/t030-thin-bash.jsonl");

fn patch_thin_log(target: &Path) -> PathBuf {
    let mut lines = Vec::new();
    for line in THIN_FIXTURE.lines() {
        let mut value: Value = serde_json::from_str(line).expect("夹具行是 JSON");
        let mut encoded: Option<String> = None;
        {
            let content = value["data"]["message"]["content"]
                .as_array_mut()
                .expect("content 数组");
            for block in content {
                if block["name"] != "bash" {
                    continue;
                }
                let mut args: Value =
                    serde_json::from_str(block["arguments"].as_str().expect("arguments 字符串"))
                        .expect("bash 参数 JSON");
                args["command"] = json!(
                    "echo $$ > serve-pid; trap 'exit 0' TERM; while [ ! -f serve-done ]; do :; done"
                );
                args["run_in_background"] = json!(true);
                let args_text = args.to_string();
                block["arguments"] = json!(args_text.clone());
                encoded = Some(args_text);
            }
        }
        if let Some(args_text) = encoded
            && let Some(chunks) = value["data"]["stream"].as_array_mut()
        {
            for chunk in chunks {
                if let Some(args_list) = chunk["args"].as_array_mut() {
                    for arg in args_list {
                        if arg
                            .as_str()
                            .is_some_and(|text| text.contains("bash-output.txt"))
                        {
                            *arg = json!(args_text.clone());
                        }
                    }
                }
            }
        }
        lines.push(value.to_string());
    }
    let path = target.join("thin.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").expect("写 thin 回放");
    path
}

fn seed_task(data_dir: &Path, cwd: &Path, title: &str) {
    let store = tasks::Store::open(data_dir.join("tasks.sqlite")).expect("tasks store");
    store
        .create_task_with(
            tasks::NewTask {
                title: title.to_owned(),
                requirement: "起一个后台命令，然后报回".to_owned(),
                dod: "命令能被退出收尾停掉".to_owned(),
                cwd: cwd.to_path_buf(),
                conversation: tasks::ConversationId::new("kind-seeded"),
                model: tasks::ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "default".to_owned(),
                },
                tools: service::THIN_WRITE_BASH_TOOLS
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
            },
            tasks::TaskPolicy {
                write_scope: Some("target.py".to_owned()),
                worktree: false,
                bash: true,
            },
            1_700_000_000_000,
        )
        .expect("建任务");
}

fn seed_keyring(data_dir: &Path, identity: &Path, endpoint: &str) {
    credentials::Keyring::open(data_dir, Some(identity))
        .expect("keyring")
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: endpoint.to_owned(),
            api_key: "test-key".to_owned(),
            ..Default::default()
        })
        .expect("save connection");
}

fn pgid_alive(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &format!("-{pid}")])
        .status()
        .expect("kill -0")
        .success()
}

#[tokio::test]
async fn serve_sigterm_kills_task_background_commands() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let cwd = dir.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(cwd.join("hello.txt"), "hello\n").unwrap();
    let title = "T068 SIGTERM 任务";
    let thin = patch_thin_log(dir.path());
    let routes = dir.path().join("routes.json");
    std::fs::write(
        &routes,
        serde_json::to_string(&json!({
            "routes": [{
                "contains": title,
                "file": thin.file_name().unwrap().to_string_lossy(),
                "paceMs": 20
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let script = replay::ReplayScript::from_routes_file(&routes).expect("回放脚本");
    let replay_server = replay::start_replay_server(script).await.expect("起回放");

    let identity = dir.path().join("identity.txt");
    seed_keyring(&data_dir, &identity, &replay_server.url());
    seed_task(&data_dir, &cwd, title);

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let home = tempfile::tempdir().expect("home");
    let mut child = Command::new(aios_dev())
        .env("HOME", home.path())
        .args([
            "serve",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--identity",
            identity.to_str().unwrap(),
            "--cwd",
            cwd.to_str().unwrap(),
            "--port",
            "0",
            "--web-dist",
            repo_root.join("web/dist").to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起 serve");

    let pid_file = cwd.join("serve-pid");
    let mut ready = false;
    for _ in 0..1500 {
        if pid_file.exists() {
            ready = true;
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            let mut stderr = String::new();
            if let Some(mut handle) = child.stderr.take() {
                drop(std::io::Read::read_to_string(&mut handle, &mut stderr));
            }
            panic!("serve 提前退出 {status:?}：{stderr}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ready, "任务后台命令没起来");
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(pgid_alive(pid));

    // SIGTERM 走 serve 的收尾路径。
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill -TERM");
    let mut status = None;
    for _ in 0..750 {
        if let Some(exited) = child.try_wait().expect("try_wait") {
            status = Some(exited);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if status.is_none() {
        drop(child.kill());
        panic!("serve 收到 SIGTERM 后没退出");
    }
    for _ in 0..500 {
        if !pgid_alive(pid) {
            replay_server.shutdown();
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(child.kill());
    replay_server.shutdown();
    panic!("SIGTERM 后任务后台进程组 {pid} 还在");
}
