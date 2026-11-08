//! 日志版本读 14–16（旧版原样读、续写 header 留原版本）、13- 与 17+ 拒绝且文件不动；只读读取（不截尾）。
//!
//! DSH 依据：`packages/core/session/src/types.ts:90`（SESSION_FORMAT_VERSION）、
//! `packages/session/session-persistence-jsonl/README.md:77,87,93`（读打开不改历史；
//! 尾行只在写句柄第一次 append 前截掉）、`src/storage.ts:317-334`（先修 torn tail 再写批）。

use std::fs;
use std::io::Write;

use model::Message;
use serde_json::Value;

use crate::{Event, EventKind, SessionError, SessionId, SessionStore, TurnEndReason};

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
fn written_header_uses_format_version_16() {
    let (_data_dir, _cwd, store) = create_store();
    let text = fs::read_to_string(store.path()).unwrap();
    let header: Value =
        serde_json::from_str(text.lines().next().expect("日志有 header 行")).unwrap();
    assert_eq!(
        header["version"], 16,
        "hook 取消原因让 FORMAT_VERSION 升到 16"
    );
}

/// 手写一份指定版本的日志（header + 一条 v15 兼容的 turn/start）。
fn legacy_log(id: &SessionId, version: u64) -> Vec<u8> {
    format!(
        "{{\"type\":\"session\",\"version\":{version},\"id\":\"{id}\",\"createdAt\":1,\"cwd\":\"/tmp\"}}\n\
         {{\"type\":\"turn/start\",\"seq\":1,\"time\":1,\"data\":{{\"turn\":1}}}}\n"
    )
    .into_bytes()
}

/// T013.1 测试清单 2：更新后的读取器能读 14 版日志；只读打开与续写都不改写原有行；
/// 续写后 header 还是 14；整份再读没有错。
#[test]
fn version_14_logs_are_read_as_is_and_can_be_continued() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    let path = store.path().to_path_buf();
    drop(store);
    let legacy = legacy_log(&id, 14);
    fs::write(&path, &legacy).unwrap();

    // 只读打开：能读、不改文件。
    let snapshot = SessionStore::open_read_only(data_dir.path(), &id).expect("读取器能读 14");
    assert_eq!(snapshot.events().len(), 1, "14 版日志的事件原样读出来");
    assert_eq!(fs::read(&path).unwrap(), legacy, "只读打开不改文件");

    // 读写打开 + 续写：header 留 14，原有行一字不改。
    let mut opened = SessionStore::open(data_dir.path(), &id).expect("写打开也接受 14");
    opened
        .append(&[EventKind::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Completed,
        }])
        .unwrap();
    drop(opened);
    let after = fs::read(&path).unwrap();
    assert!(after.starts_with(&legacy), "续写只追加：原有行一字不改");
    let text = String::from_utf8(after).unwrap();
    let header: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(header["version"], 14, "在 14 版日志上续写时 header 留 14");

    // 整份再读没有错，新事件也在。
    let snapshot = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(snapshot.events().len(), 2);
}

/// 升到 16 之后，15 版日志仍然原样读；续写时 header 留 15。
#[test]
fn version_15_logs_are_read_as_is_and_can_be_continued() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    let path = store.path().to_path_buf();
    drop(store);
    let legacy = legacy_log(&id, 15);
    fs::write(&path, &legacy).unwrap();

    let snapshot = SessionStore::open_read_only(data_dir.path(), &id).expect("16 读取器能读 15");
    assert_eq!(snapshot.events().len(), 1, "15 版日志的事件原样读出来");
    assert_eq!(fs::read(&path).unwrap(), legacy, "只读打开不改文件");

    let mut opened = SessionStore::open(data_dir.path(), &id).expect("写打开也接受 15");
    opened
        .append(&[EventKind::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Completed,
        }])
        .unwrap();
    drop(opened);
    let after = fs::read(&path).unwrap();
    assert!(after.starts_with(&legacy), "续写只追加：原有行一字不改");
    let text = String::from_utf8(after).unwrap();
    let header: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(header["version"], 15, "在 15 版日志上续写时 header 留 15");
}

/// T013.1 测试清单 2：13 及以下、16 及以上照旧拒绝，文件一字不改；更新的提示升级。
#[test]
fn format_versions_outside_14_through_16_are_rejected_without_touching_the_file() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    let path = store.path().to_path_buf();
    drop(store);

    for version in [1, 13] {
        let legacy = legacy_log(&id, version);
        fs::write(&path, &legacy).unwrap();
        let error = match SessionStore::open(data_dir.path(), &id) {
            Ok(_) => panic!("v{version} 日志不能被接受"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                SessionError::UnsupportedVersion {
                    version: got,
                    min: 14,
                    max: 16
                } if got == version
            ),
            "v{version} 要拒绝并写出范围，实际 {error}"
        );
        assert!(
            error.to_string().contains("14–16"),
            "报错要写出能读的范围：{error}"
        );
        assert_eq!(fs::read(&path).unwrap(), legacy, "拒绝旧版本不能改写它");
        let read_error = SessionStore::open_read_only(data_dir.path(), &id).unwrap_err();
        assert!(
            matches!(
                read_error,
                SessionError::UnsupportedVersion {
                    version: got,
                    min: 14,
                    max: 16
                } if got == version
            ),
            "只读路径同样拒绝 v{version}，实际 {read_error}"
        );
    }

    for version in [17, 99] {
        let legacy = legacy_log(&id, version);
        fs::write(&path, &legacy).unwrap();
        let error = match SessionStore::open(data_dir.path(), &id) {
            Ok(_) => panic!("v{version} 日志不能被接受"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                SessionError::UnsupportedFutureVersion {
                    version: got,
                    min: 14,
                    max: 16
                } if got == version
            ),
            "更新版本要拒绝，实际 {error}"
        );
        assert!(
            error.to_string().contains("请升级"),
            "比 16 新的要提示升级：{error}"
        );
        assert_eq!(fs::read(&path).unwrap(), legacy, "拒绝新版本不能改写它");
    }
}

/// T013.1 测试清单 2：tool/result 的 meta 写进日志再读出来一字不差。
#[test]
fn tool_result_meta_roundtrips_through_the_log() {
    let (_data_dir, _cwd, mut store) = create_store();
    let meta = serde_json::json!({
        "diffs": [{"path": "/tmp/a.txt", "oldText": "old", "newText": "new"}],
        "operation": "update",
    });
    store
        .append(&[EventKind::ToolResult {
            turn: 1,
            step: 1,
            message: Message::tool_result("call_1", "Updated file", false),
            error: None,
            meta: Some(meta.clone()),
        }])
        .unwrap();
    // 原始 JSONL 行里就是原样的 meta（按解析后的 JSON 比较，键顺序无关）。
    let text = fs::read_to_string(store.path()).unwrap();
    let line: Value = text
        .lines()
        .nth(1)
        .and_then(|line| serde_json::from_str(line).ok())
        .expect("第二条是 tool/result");
    assert_eq!(line["data"]["meta"], meta, "meta 要原样落盘：{text}");
    // 读回来一字不差。
    let snapshot =
        SessionStore::open_read_only(store.path().parent().unwrap().parent().unwrap(), store.id())
            .unwrap();
    let event = &snapshot.events()[0];
    let EventKind::ToolResult { meta: got, .. } = &event.kind else {
        panic!("第一条应该是 tool/result");
    };
    assert_eq!(got, &Some(meta), "读出来的 meta 要一字不差");
}

/// T013.1（P2）：14 版日志里的 tool/result 没有 meta 字段，15 读取器读出来是 None。
#[test]
fn version_14_tool_result_without_meta_reads_with_none() {
    let (data_dir, _cwd, store) = create_store();
    let id = store.id().clone();
    let path = store.path().to_path_buf();
    drop(store);
    let legacy = format!(
        "{{\"type\":\"session\",\"version\":14,\"id\":\"{id}\",\"createdAt\":1,\"cwd\":\"/tmp\"}}\n\
         {{\"type\":\"tool/result\",\"seq\":1,\"time\":1,\"data\":{{\"turn\":1,\"step\":1,\"message\":{{\"content\":[{{\"text\":\"ok\",\"type\":\"text\"}}],\"id\":\"00000000-0000-4000-8000-000000000001\",\"role\":\"tool\",\"source\":{{\"callId\":\"call_1\",\"kind\":\"tool\"}},\"toolCallId\":\"call_1\"}}}}}}\n"
    );
    fs::write(&path, legacy.as_bytes()).unwrap();
    let snapshot = SessionStore::open_read_only(data_dir.path(), &id)
        .expect("15 读取器能读 14 版没有 meta 的 tool/result");
    let EventKind::ToolResult { meta, .. } = &snapshot.events()[0].kind else {
        panic!("第一条应该是 tool/result");
    };
    assert!(meta.is_none(), "14 版没有 meta 字段 → 读出来 None");
    assert_eq!(
        fs::read(&path).unwrap(),
        legacy.as_bytes(),
        "只读打开不改文件"
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
    // T009.H6：子进程持锁 1.2s，模拟崩溃/持锁窗口；真实宽限期
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
    let home = data.join("test-home");
    std::fs::create_dir_all(&home).expect("隔离 HOME");
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", LOCK_CHILD_TEST, "--nocapture"])
        .env("HOME", &home)
        .env(LOCK_CHILD_DATA, data)
        .env(LOCK_CHILD_ID, id.as_str())
        .env(LOCK_CHILD_MARKER, marker)
        .env(LOCK_CHILD_MODE, mode)
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
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
        // T009.H6：等子进程写 marker 文件；轮询间隔
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("子进程没有在超时内写出 marker");
}

fn wait_until_writable(data: &std::path::Path, id: &crate::SessionId) -> SessionStore {
    for _ in 0..200 {
        if let Ok(store) = SessionStore::open(data, id) {
            return store;
        }
        // T009.H6：等子进程释放写锁；轮询间隔
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

/// 加载记录（tool/registry）进日志后 FORMAT_VERSION 升到 17；读取器仍从 14 起。
#[test]
fn format_version_bumps_to_17_after_tool_registry() {
    assert_eq!(crate::FORMAT_VERSION, 17);
    assert_eq!(crate::MIN_READ_VERSION, 14);
}

/// 注入的 panic 会经默认 panic hook 打到标准错误，门禁日志里就成了「有 panic」。
/// 这里装一个过滤钩子：只吞这一条注入的 panic，其余照旧交给原来的钩子。
/// 不还原：nextest 每个测试一个进程，而在断言失败的 unwind 里改钩子会被标准库拒绝。
fn silence_injected_panic() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let text = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or_default();
        if !text.contains("注入的追加钩子") {
            previous(info);
        }
    }));
}

/// 追加之后的钩子 panic：业务照常（事件已经落盘），但要留一条带原因的警告。
#[test]
fn hook_panic_signal() {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let hook: crate::AppendHook = std::sync::Arc::new(|_events: &[Event]| {
        panic!("注入的追加钩子 panic");
    });
    store.set_append_hook(Some(hook));
    let warnings = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&warnings);
    store.set_warning_sink(Some(std::sync::Arc::new(move |message: &str| {
        sink.lock().expect("警告锁").push(message.to_owned());
    })));
    silence_injected_panic();

    let seqs = store
        .append(&[EventKind::TurnStart { turn: 1 }])
        .expect("钩子 panic 不能挡住已经成功的写入");
    assert_eq!(seqs, vec![1]);
    assert_eq!(store.events().len(), 1, "事件已经落盘");

    let warnings = warnings.lock().expect("警告锁");
    assert_eq!(warnings.len(), 1, "一次 panic 一条警告：{warnings:?}");
    assert!(
        warnings[0].contains("注入的追加钩子 panic"),
        "警告要带原因：{}",
        warnings[0]
    );
}
