use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use credentials::{CheckResult, CheckState, ConnectionInfo, Keyring, NewConnection};
use tempfile::TempDir;

const TEST_KEY: &str = "sk-test-0123456789abcdef";

struct Fixture {
    _dir: TempDir,
    data_dir: PathBuf,
    identity_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let data_dir = dir.path().join("data");
        let identity_path = dir.path().join("keyring/identity.txt");
        Self {
            _dir: dir,
            data_dir,
            identity_path,
        }
    }

    fn open(&self) -> Keyring {
        Keyring::open(&self.data_dir, Some(&self.identity_path)).expect("open keyring")
    }
}

fn connection() -> NewConnection {
    NewConnection {
        id: "deepseek".to_owned(),
        name: "DeepSeek".to_owned(),
        model: "deepseek-flash".to_owned(),
        endpoint: "https://api.deepseek.com".to_owned(),
        api_key: TEST_KEY.to_owned(),
    }
}

#[track_caller]
fn saved(view: &credentials::KeyringView) -> &ConnectionInfo {
    assert_eq!(view.connections.len(), 1, "应该有一条连接：{view:?}");
    &view.connections[0]
}

#[test]
fn 存进去后描述正确且不含正文() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    let info = keyring.save_connection(connection()).expect("save");
    assert_eq!(info.id, "deepseek");
    assert_eq!(info.model, "deepseek-flash");
    assert_eq!(info.check, CheckState::Never);

    let view = keyring.view();
    let stored = saved(&view);
    assert_eq!(stored.endpoint, "https://api.deepseek.com");
    assert!(view.key_file.problem.is_none(), "钥匙文件应可用：{view:?}");
    assert!(!view.first_use);
    let json = serde_json::to_string(&view).expect("serialize view");
    assert!(!json.contains(TEST_KEY), "视图里不能出现 key 正文：{json}");
}

#[test]
fn 重开数据目录后仍能签发() {
    let fixture = Fixture::new();
    {
        let keyring = fixture.open();
        keyring.save_connection(connection()).expect("save");
        keyring
            .record_check("deepseek", CheckResult::Ok { at: 10 })
            .expect("check");
    }
    let reopened = fixture.open();
    let view = reopened.view();
    assert!(
        saved(&view).check == CheckState::Ok { at: 10 },
        "验证状态要持久化：{view:?}"
    );

    let issued = reopened.issue("deepseek", "kind-turn", Some("run-1"), None);
    assert!(issued.is_ok(), "签发应成功：{issued:?}");
    let issued = issued.expect("issue");
    assert_eq!(issued.connection, "deepseek");
    assert_eq!(issued.secret, TEST_KEY);
    let debug = format!("{issued:?}");
    assert!(!debug.contains(TEST_KEY), "Debug 不能泄露 key：{debug}");
}

#[test]
fn 换钥匙位置后还能读且旧位置不在了() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");
    assert!(fixture.identity_path.exists(), "open 应生成钥匙文件");

    let new_path = fixture._dir.path().join("moved/new-identity.txt");
    keyring.relocate_identity(&new_path).expect("relocate");
    assert!(new_path.exists(), "新位置应有钥匙文件");
    assert!(!fixture.identity_path.exists(), "旧位置不应留着");

    let reopened = Keyring::open(&fixture.data_dir, None).expect("reopen from config");
    assert_eq!(reopened.identity_path(), new_path);
    let issued = reopened.issue("deepseek", "kind-turn", None, None);
    assert!(issued.is_ok(), "换位置后应还能签发：{issued:?}");
}

#[test]
fn 钥匙文件挪走后描述报读不出且签发失败() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");

    let moved = fixture._dir.path().join("elsewhere/identity.txt");
    std::fs::create_dir_all(moved.parent().expect("parent")).expect("mkdir");
    let moved_ok = std::fs::rename(&fixture.identity_path, &moved).is_ok();
    assert!(moved_ok, "open 应生成钥匙文件（当前骨架没有）");

    let reopened = fixture.open();
    let view = reopened.view();
    assert!(
        view.key_file.problem.is_some(),
        "钥匙文件不在应报读不出：{view:?}"
    );
    let issued = reopened.issue("deepseek", "kind-turn", None, None);
    assert!(issued.is_err(), "钥匙文件不在时签发必须失败");
}

#[test]
fn 指回新位置恢复重新开始清空() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");

    let moved = fixture._dir.path().join("elsewhere/identity.txt");
    std::fs::create_dir_all(moved.parent().expect("parent")).expect("mkdir");
    let moved_ok = std::fs::rename(&fixture.identity_path, &moved).is_ok();
    assert!(moved_ok, "open 应生成钥匙文件（当前骨架没有）");

    let reopened = fixture.open();
    assert!(reopened.view().key_file.problem.is_some());
    reopened.repoint_identity(&moved).expect("repoint");
    let view = reopened.view();
    assert!(view.key_file.problem.is_none(), "指回后应恢复：{view:?}");
    assert!(reopened.issue("deepseek", "kind-turn", None, None).is_ok());

    reopened.restart().expect("restart");
    let view = reopened.view();
    assert!(
        view.connections.is_empty(),
        "重新开始后连接应为空：{view:?}"
    );
    assert!(view.first_use, "重新开始后回到第一次使用：{view:?}");
    assert!(
        view.key_file.problem.is_none(),
        "重新开始后钥匙文件能读：{view:?}"
    );
    let issued = reopened.issue("deepseek", "kind-turn", None, None);
    assert!(issued.is_err(), "重新开始后没有连接可签发");
}

#[test]
fn 删掉连接后签发失败且描述里没有了() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");
    assert!(keyring.delete_connection("deepseek").expect("delete"));
    assert!(!keyring.delete_connection("deepseek").expect("delete twice"));
    assert!(keyring.view().connections.is_empty());
    let issued = keyring.issue("deepseek", "kind-turn", None, None);
    assert!(issued.is_err(), "删掉后签发必须失败（原因是凭证）");
}

#[test]
fn 每次签发一条记录且记录里没有正文() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");
    assert!(
        keyring
            .issue("deepseek", "kind-turn", Some("run-1"), None)
            .is_ok()
    );
    assert!(
        keyring
            .issue("deepseek", "compaction", Some("run-1"), None)
            .is_ok()
    );

    let issues = keyring.issues();
    assert_eq!(issues.len(), 2, "两次签发应有两条记录：{issues:?}");
    assert_eq!(issues[0].purpose, "compaction", "最近的在前：{issues:?}");
    assert_eq!(issues[0].connection, "deepseek");
    assert_eq!(issues[0].run.as_deref(), Some("run-1"));
    assert!(keyring.view().issued_today >= 2);
    let json = serde_json::to_string(&issues).expect("serialize issues");
    assert!(!json.contains(TEST_KEY), "签发记录里不能出现 key：{json}");
}

#[test]
fn 存储变化会通知订阅者() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    let changes = keyring.subscribe();
    keyring.save_connection(connection()).expect("save");
    assert!(
        changes.recv_timeout(Duration::from_secs(1)).is_ok(),
        "保存后应收到变更通知"
    );
    keyring.delete_connection("deepseek").expect("delete");
    assert!(
        changes.recv_timeout(Duration::from_secs(1)).is_ok(),
        "删除后应收到变更通知"
    );
}

#[test]
fn 两个实例在换位置后都恢复且能签发() {
    let fixture = Fixture::new();
    let first = fixture.open();
    first.save_connection(connection()).expect("save");
    // 服务里 model_factory/设置/凭证来源可能各持有自己的 Keyring 句柄；先打开第二个实例。
    let second = fixture.open();

    let new_path = fixture._dir.path().join("moved/shared-identity.txt");
    first.relocate_identity(&new_path).expect("relocate");

    assert_eq!(
        second.identity_path(),
        new_path,
        "第二个实例必须看到配置里的新位置，不能缓存旧路径"
    );
    let view = second.view();
    assert!(
        view.key_file.problem.is_none(),
        "换位置后第二个实例应能读：{view:?}"
    );
    let issued = second.issue("deepseek", "kind-turn", None, None);
    assert!(issued.is_ok(), "换位置后第二个实例应还能签发：{issued:?}");

    // 指回同样要不重启恢复：钥匙文件再从 new_path 挪走，second 指回，先打开的 first 也要看到。
    let moved_again = fixture._dir.path().join("moved-again/identity.txt");
    std::fs::create_dir_all(moved_again.parent().expect("parent")).expect("mkdir");
    std::fs::rename(&new_path, &moved_again).expect("move identity");
    second.repoint_identity(&moved_again).expect("repoint");
    assert_eq!(
        first.identity_path(),
        moved_again,
        "指回后先打开的实例也必须看到新位置"
    );
    assert!(
        first.view().key_file.problem.is_none(),
        "指回后 first 应能读：{:?}",
        first.view().key_file
    );
    assert!(
        first.issue("deepseek", "kind-turn", None, None).is_ok(),
        "指回后 first 应还能签发"
    );
}

#[test]
fn 并发写会等锁而不是丢更新() {
    let fixture = Fixture::new();
    let keyring = Arc::new(fixture.open());
    std::fs::create_dir_all(&fixture.data_dir).expect("data dir");
    let lock_path = fixture.data_dir.join("keyring.lock");
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open lock");
    held.lock().expect("hold keyring lock");

    let (sender, receiver) = mpsc::channel();
    let worker = {
        let keyring = Arc::clone(&keyring);
        thread::spawn(move || {
            let result = keyring.save_connection(connection());
            sender.send(result.is_ok()).expect("send result");
        })
    };
    assert!(
        receiver.recv_timeout(Duration::from_millis(150)).is_err(),
        "外部持有 keyring.lock 时，保存不能直接完成（会丢更新）"
    );
    drop(held);
    assert!(
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("worker result"),
        "释放锁后保存应成功"
    );
    worker.join().expect("worker join");
}

#[test]
fn 签发值用完清零() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");
    let mut issued = keyring
        .issue("deepseek", "kind-turn", None, None)
        .expect("issue");
    assert_eq!(issued.secret, TEST_KEY);
    issued.zeroize();
    assert!(
        issued.secret.is_empty() || issued.secret.as_bytes().iter().all(|byte| *byte == 0),
        "显式清零后不能还是原文：{issued:?}"
    );
}

#[test]
fn into_secret返回原值且交出所有权() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");
    let issued = keyring
        .issue("deepseek", "kind-turn", None, None)
        .expect("issue");
    let secret = issued.into_secret();
    assert_eq!(secret, TEST_KEY);
}

#[test]
fn 跨进程持锁时保存会等待() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(&fixture.data_dir).expect("data dir");
    let lock_path = fixture.data_dir.join("keyring.lock");
    let sentinel = fixture.data_dir.join("lock-ready");
    let script = "import fcntl,pathlib,sys,time
f=open(sys.argv[1],'a+')
fcntl.flock(f,fcntl.LOCK_EX)
pathlib.Path(sys.argv[2]).write_text('locked')
time.sleep(0.6)
";
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&lock_path)
        .arg(&sentinel)
        .spawn()
        .expect("spawn lock holder");
    for _ in 0..100 {
        if sentinel.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(sentinel.exists(), "子进程应已持有 keyring.lock");

    let keyring = Arc::new(fixture.open());
    let (sender, receiver) = mpsc::channel();
    let worker = {
        let keyring = Arc::clone(&keyring);
        thread::spawn(move || {
            sender
                .send(keyring.save_connection(connection()).is_ok())
                .expect("send");
        })
    };
    assert!(
        receiver.recv_timeout(Duration::from_millis(150)).is_err(),
        "另一个进程持有 keyring.lock 时，保存必须等待"
    );
    let status = child.wait().expect("wait child");
    assert!(status.success(), "lock holder 应正常退出");
    assert!(
        receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("worker result"),
        "跨进程锁释放后保存应成功"
    );
    worker.join().expect("join");
}

#[test]
fn 签发记录带归属且视图有界() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");
    keyring
        .issue("deepseek", "kind-turn", Some("run-1"), Some(3))
        .expect("issue");
    let issues = keyring.issues();
    assert_eq!(issues[0].run.as_deref(), Some("run-1"), "记录要带 run");
    assert_eq!(issues[0].turn, Some(3), "记录要带 turn");
    for index in 0..25 {
        keyring
            .issue("deepseek", "kind-turn", Some("run-1"), Some(3 + index))
            .expect("issue");
    }
    assert!(
        keyring.view().issues.len() <= 20,
        "view 只返回最近有界条数，不能全量"
    );
}
