use std::path::PathBuf;
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

    let issued = reopened.issue("deepseek", "kind-turn", Some("run-1"));
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
    let issued = reopened.issue("deepseek", "kind-turn", None);
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
    let issued = reopened.issue("deepseek", "kind-turn", None);
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
    assert!(reopened.issue("deepseek", "kind-turn", None).is_ok());

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
    let issued = reopened.issue("deepseek", "kind-turn", None);
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
    let issued = keyring.issue("deepseek", "kind-turn", None);
    assert!(issued.is_err(), "删掉后签发必须失败（原因是凭证）");
}

#[test]
fn 每次签发一条记录且记录里没有正文() {
    let fixture = Fixture::new();
    let keyring = fixture.open();
    keyring.save_connection(connection()).expect("save");
    assert!(
        keyring
            .issue("deepseek", "kind-turn", Some("run-1"))
            .is_ok()
    );
    assert!(
        keyring
            .issue("deepseek", "compaction", Some("run-1"))
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
