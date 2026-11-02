//! T006.5a：FORMAT_VERSION 升级（旧版本直接拒绝、不迁移）与只读读取（不截尾）。
//!
//! DSH 依据：`packages/core/session/src/types.ts:90`（SESSION_FORMAT_VERSION）、
//! `packages/session/session-persistence-jsonl/README.md:77,87,93`（读打开不改历史；
//! 尾行只在写句柄第一次 append 前截掉）、`src/storage.ts:317-334`（先修 torn tail 再写批）。

use std::fs;
use std::io::Write;

use serde_json::Value;

use crate::{EventKind, SessionError, SessionId, SessionStore, TurnEndReason};

fn create_store() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    (data_dir, cwd, store)
}

/// 往日志文件末尾追加半行，模拟掉电时最后一条记录只写了一半。
fn append_torn_line(store: &SessionStore) {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(store.path())
        .unwrap();
    file.write_all(b"{\"type\":\"turn/end\",\"seq\":99,")
        .unwrap();
    file.sync_data().unwrap();
}

#[test]
fn written_header_uses_format_version_7() {
    let (_data_dir, _cwd, store) = create_store();
    let text = fs::read_to_string(store.path()).unwrap();
    let header: Value =
        serde_json::from_str(text.lines().next().expect("日志有 header 行")).unwrap();
    assert_eq!(
        header["version"], 7,
        "job/registered、job/settled 事件加入后 FORMAT_VERSION 升到 7"
    );
}

#[test]
fn previous_format_versions_are_rejected_without_migration() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    let path = store.path().to_path_buf();
    drop(store);

    // 手写 v1–v5 日志：header 版本不对 + 旧格式事件；读取一律直接拒绝。
    for version in [1, 2, 3, 4, 5, 6] {
        let legacy = format!(
            "{{\"type\":\"session\",\"version\":{version},\"id\":\"{id}\",\"createdAt\":1,\"cwd\":\"/tmp\"}}\n\
             {{\"seq\":1,\"time\":1,\"type\":\"turn/start\",\"turn\":1}}\n"
        );
        fs::write(&path, legacy.as_bytes()).unwrap();
        let before = fs::read(&path).unwrap();

        let error = match SessionStore::open(data_dir.path(), &id) {
            Ok(_) => panic!("旧版本日志不能被接受"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                SessionError::UnsupportedVersion {
                    version: got,
                    supported: 7
                } if got == version
            ),
            "v{version} 日志要直接拒绝，实际 {error}"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "拒绝旧版本不能迁移或改写它"
        );

        let read_error = SessionStore::open_read_only(data_dir.path(), &id).unwrap_err();
        assert!(
            matches!(
                read_error,
                SessionError::UnsupportedVersion {
                    version: got,
                    supported: 7
                } if got == version
            ),
            "只读路径同样拒绝 v{version}，实际 {read_error}"
        );
    }
}

#[test]
fn read_only_open_leaves_torn_tail_untouched() {
    let (data_dir, _cwd, mut store) = create_store();
    let id = store.id().clone();
    store.append(&[EventKind::TurnStart { turn: 1 }]).unwrap();
    append_torn_line(&store);
    let path = store.path().to_path_buf();
    drop(store);
    let before = fs::read(&path).unwrap();

    let snapshot = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        snapshot.events().len(),
        1,
        "只读只读到最后一个完整行之前的事件"
    );
    assert_eq!(snapshot.id(), &id);
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "只读打开不能截断或改写文件"
    );
}

#[test]
fn write_open_defers_torn_tail_truncation_until_first_append() {
    let (data_dir, _cwd, mut store) = create_store();
    let id = store.id().clone();
    store.append(&[EventKind::TurnStart { turn: 1 }]).unwrap();
    append_torn_line(&store);
    let path = store.path().to_path_buf();
    drop(store);
    let torn = fs::read(&path).unwrap();

    let mut reopened = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        torn,
        "写打开也不马上截尾，等第一次 append（DSH storage.ts:317-334）"
    );
    reopened
        .append(&[EventKind::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Completed,
        }])
        .unwrap();
    let after = fs::read(&path).unwrap();
    assert!(
        !String::from_utf8_lossy(&after).contains("\"seq\":99"),
        "第一次 append 前要把半行尾截掉"
    );
    assert_eq!(reopened.events().len(), 2);
    assert_eq!(reopened.events()[1].seq, 2, "截尾后 seq 仍连续");
}

// ---------------------------------------------------------------------------
// T006.5b：单写者锁（<data-dir>/sessions/<id>.lock，非阻塞排他 flock）
// ---------------------------------------------------------------------------

const LOCK_CHILD_DATA: &str = "AIOS_LOCK_CHILD_DATA";
const LOCK_CHILD_ID: &str = "AIOS_LOCK_CHILD_ID";
const LOCK_CHILD_MARKER: &str = "AIOS_LOCK_CHILD_MARKER";
const LOCK_CHILD_MODE: &str = "AIOS_LOCK_CHILD_MODE";
const LOCK_CHILD_TEST: &str = "store_tests::writer_lock_child_helper";

/// 子进程侧（由父测试用 --exact 拉起）：取写锁、写 marker，然后按 mode 持有或崩溃退出。
#[test]
fn writer_lock_child_helper() {
    let Ok(data) = std::env::var(LOCK_CHILD_DATA) else {
        return; // 正常测试进程里是 no-op
    };
    let id = SessionId::parse(&std::env::var(LOCK_CHILD_ID).unwrap()).unwrap();
    let marker = std::env::var(LOCK_CHILD_MARKER).unwrap();
    let mode = std::env::var(LOCK_CHILD_MODE).unwrap_or_default();
    let store = SessionStore::open(std::path::Path::new(&data), &id).expect("子进程取写锁");
    fs::write(&marker, b"locked").unwrap();
    if mode == "abort" {
        // 模拟崩溃：进程直接死，内核随进程释放 flock（DSH：崩溃持有者立即让出）。
        std::process::abort();
    }
    std::thread::sleep(std::time::Duration::from_millis(1200));
    drop(store);
}

fn lock_path_of(store: &SessionStore) -> std::path::PathBuf {
    std::path::Path::new(store.path()).with_extension("lock")
}

fn spawn_lock_child(
    data: &std::path::Path,
    id: &crate::SessionId,
    marker: &std::path::Path,
    mode: &str,
) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", LOCK_CHILD_TEST, "--nocapture"])
        .env(LOCK_CHILD_DATA, data)
        .env(LOCK_CHILD_ID, id.as_str())
        .env(LOCK_CHILD_MARKER, marker)
        .env(LOCK_CHILD_MODE, mode)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn 子进程")
}

fn wait_for_file(path: &std::path::Path) {
    for _ in 0..400 {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("子进程没有在超时内写出 marker");
}

fn wait_until_writable(data: &std::path::Path, id: &crate::SessionId) -> SessionStore {
    for _ in 0..200 {
        if let Ok(store) = SessionStore::open(data, id) {
            return store;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("持锁进程退出后仍拿不到写锁");
}

#[test]
fn write_lock_is_exclusive_and_released_on_drop() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    let lock_path = lock_path_of(&store);
    assert!(
        lock_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".lock"),
        "锁文件名照 DSH：<id>.lock"
    );
    assert!(
        lock_path.exists(),
        "写打开要创建 <data-dir>/sessions/<id>.lock"
    );
    let error = match SessionStore::open(data_dir.path(), &id) {
        Ok(_) => panic!("第二个写者必须被拒"),
        Err(error) => error,
    };
    assert!(
        matches!(error, SessionError::AlreadyOwned { .. }),
        "被占用要可识别（照 DSH SessionAlreadyOwnedError）：{error}"
    );
    // 只读打开不受写锁限制（DSH 只有 writer lock）。
    let snapshot = SessionStore::open_read_only(data_dir.path(), &id).expect("只读打开允许");
    assert_eq!(snapshot.id(), &id);
    drop(store);
    let reopened = SessionStore::open(data_dir.path(), &id).expect("释放后可以重新取锁");
    assert_eq!(reopened.id(), &id);
    drop(reopened);
    assert!(lock_path.exists(), "释放不删锁文件（照 DSH lease 语义）");
}

#[test]
fn two_process_writers_conflict_and_release_on_exit() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    drop(store); // 只保留物化好的会话文件
    let marker = data_dir.path().join("lock-child-holding");
    let mut child = spawn_lock_child(data_dir.path(), &id, &marker, "hold");
    wait_for_file(&marker);
    let error = match SessionStore::open(data_dir.path(), &id) {
        Ok(_) => panic!("跨进程写锁必须互斥"),
        Err(error) => error,
    };
    assert!(
        matches!(error, SessionError::AlreadyOwned { .. }),
        "跨进程被占用也要可识别：{error}"
    );
    assert!(child.wait().unwrap().success(), "子进程正常退出");
    let reopened = wait_until_writable(data_dir.path(), &id);
    drop(reopened);
}

#[test]
fn crashed_writer_lock_is_released_with_process() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    drop(store);
    let marker = data_dir.path().join("lock-child-aborted");
    let mut child = spawn_lock_child(data_dir.path(), &id, &marker, "abort");
    wait_for_file(&marker);
    let status = child.wait().unwrap();
    assert!(!status.success(), "子进程崩溃退出（abort）");
    // 崩溃的持有者不应留下死锁：内核随进程释放 flock。
    let reopened = wait_until_writable(data_dir.path(), &id);
    drop(reopened);
}

/// T006.7 fix：相对 data-dir 打开/创建时也把对外路径绝对化（spill locator/日志路径）。
#[test]
fn store_absolutizes_relative_data_dir() {
    let cwd = std::env::current_dir().unwrap();
    let dir = tempfile::tempdir_in(&cwd).unwrap();
    let relative = dir.path().strip_prefix(&cwd).unwrap().to_path_buf();
    let work = tempfile::tempdir().unwrap();
    let store = SessionStore::create(&relative, work.path()).unwrap();
    assert!(
        store.path().is_absolute(),
        "相对 data-dir 的日志路径也要绝对：{:?}",
        store.path()
    );
    let id = store.id().clone();
    drop(store);
    let reopened = SessionStore::open(&relative, &id).unwrap();
    assert!(reopened.path().is_absolute(), "写打开同样绝对化");
}
