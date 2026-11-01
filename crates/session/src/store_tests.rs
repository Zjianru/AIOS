//! T006.5a：FORMAT_VERSION 升级（旧版本直接拒绝、不迁移）与只读读取（不截尾）。
//!
//! DSH 依据：`packages/core/session/src/types.ts:90`（SESSION_FORMAT_VERSION）、
//! `packages/session/session-persistence-jsonl/README.md:77,87,93`（读打开不改历史；
//! 尾行只在写句柄第一次 append 前截掉）、`src/storage.ts:317-334`（先修 torn tail 再写批）。

use std::fs;
use std::io::Write;

use serde_json::Value;

use crate::{EventKind, SessionError, SessionStore, TurnEndReason};

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
fn written_header_uses_format_version_2() {
    let (_data_dir, _cwd, store) = create_store();
    let text = fs::read_to_string(store.path()).unwrap();
    let header: Value =
        serde_json::from_str(text.lines().next().expect("日志有 header 行")).unwrap();
    assert_eq!(
        header["version"], 2,
        "事件字段照 DSH 改形后 FORMAT_VERSION 升到 2"
    );
}

#[test]
fn v1_log_is_rejected_without_migration() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    let path = store.path().to_path_buf();
    drop(store);

    // 手写一份 v1 日志：header version 1 + 旧格式（扁平字段）事件。
    let legacy = format!(
        "{{\"type\":\"session\",\"version\":1,\"id\":\"{id}\",\"createdAt\":1,\"cwd\":\"/tmp\"}}\n\
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
                version: 1,
                supported: 2
            }
        ),
        "旧版本日志要直接拒绝，实际 {error}"
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
                version: 1,
                supported: 2
            }
        ),
        "只读路径同样拒绝旧版本，实际 {read_error}"
    );
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
