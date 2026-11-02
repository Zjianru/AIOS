//! 接缝 2：大结果 spill 的存储与保留（测试清单第 8 条；T006.7 照 DSH）。

use std::fs;
use std::time::{Duration, SystemTime};

use tools::spill::{encode_spill_segment, session_dir_name, sweep_spill_root};
use tools::{ToolSessionState, bound_result, estimate_text_tokens};

#[test]
fn spill_store_isolated_per_session() {
    let dir = tempfile::tempdir().unwrap();
    let a = ToolSessionState::new(dir.path(), "session-a");
    let b = ToolSessionState::new(dir.path(), "session-b");

    let ref_a = a.spill().save_text("big", "alpha content").unwrap();
    let ref_b = b.spill().save_text("big", "beta content").unwrap();

    assert_ne!(ref_a.locator, ref_b.locator);
    assert_eq!(
        ref_a.locator.parent().unwrap(),
        dir.path().join("spill").join(session_dir_name("session-a")),
        "会话目录照 DSH spill-local: root/session-<hash>"
    );
    assert_eq!(
        ref_b.locator.parent().unwrap(),
        dir.path().join("spill").join(session_dir_name("session-b"))
    );
    assert_eq!(fs::read_to_string(&ref_a.locator).unwrap(), "alpha content");
    assert_eq!(fs::read_to_string(&ref_b.locator).unwrap(), "beta content");
    assert_eq!(ref_a.bytes, "alpha content".len());
}

#[test]
fn spill_session_dir_and_file_names_match_dsh() {
    let dir = tempfile::tempdir().unwrap();
    let state = ToolSessionState::new(dir.path(), "s1");
    let saved = state.spill().save_text("big.txt", "alpha").unwrap();
    let file_name = saved
        .locator
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let (random, encoded) = file_name.split_once('-').expect("随机前缀 + 转义名");
    assert_eq!(random.len(), 12, "{file_name}");
    assert!(
        random
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()),
        "{file_name}"
    );
    assert_eq!(encoded, "big.txt");

    let escaped = state.spill().save_text("a b/c.txt", "x").unwrap();
    let name = escaped.locator.file_name().unwrap().to_str().unwrap();
    assert_eq!(name.split_once('-').unwrap().1, "a~0020b~002Fc.txt");
    assert_eq!(encode_spill_segment(""), "~");
    assert_eq!(encode_spill_segment("."), "~002E");
    assert_eq!(encode_spill_segment(".."), "~002E~002E");
    assert_eq!(encode_spill_segment("a~b"), "a~007Eb");

    let dir_name = session_dir_name("s1");
    assert!(dir_name.starts_with("session-"));
    let hash = &dir_name["session-".len()..];
    assert_eq!(hash.len(), 12, "{dir_name}");
    assert!(hash.chars().all(|character| character.is_ascii_hexdigit()));
    assert_eq!(session_dir_name("s1"), dir_name);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = fs::metadata(saved.locator.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(&saved.locator).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }
}

fn age_file(path: &std::path::Path, days: u64) {
    let file = fs::File::options().write(true).open(path).unwrap();
    let modified = SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60);
    file.set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

#[test]
fn sweep_removes_expired_spill_files_and_empty_session_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let state = ToolSessionState::new(dir.path(), "s1");
    let fresh = state.spill().save_text("big.txt", "fresh").unwrap();
    let old_path = fresh.locator.parent().unwrap().join("0123456789ab-old.txt");
    fs::write(&old_path, "old").unwrap();
    age_file(&old_path, 31);
    let unrelated = dir.path().join("spill").join("other");
    fs::create_dir_all(&unrelated).unwrap();

    let cutoff = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
    let removed = sweep_spill_root(&dir.path().join("spill"), cutoff);
    assert_eq!(removed, 1, "只删过期文件");
    assert!(!old_path.exists());
    assert!(fresh.locator.exists(), "新文件保留");
    assert!(unrelated.exists(), "非会话目录保留");

    let empty = dir.path().join("spill").join(session_dir_name("dead"));
    fs::create_dir_all(&empty).unwrap();
    sweep_spill_root(&dir.path().join("spill"), cutoff);
    assert!(!empty.exists(), "空会话目录应清掉");
    assert!(fresh.locator.parent().unwrap().exists(), "非空会话目录保留");
}

#[test]
fn bound_result_keeps_head_tail_and_notice() {
    let dir = tempfile::tempdir().unwrap();
    let state = ToolSessionState::new(dir.path(), "s1");
    let store = state.spill().clone();

    let head_marker = "HEAD-MARKER";
    let tail_marker = "TAIL-MARKER";
    let middle = "x".repeat(60_000);
    let content = format!("{head_marker}\n{middle}\n{tail_marker}");
    let preview = bound_result(&store, "big_tool", "call_1", &content, 12_500)
        .expect("超大结果应当落 spill 并换成 preview");

    assert!(preview.starts_with(head_marker), "{preview:.120}");
    assert!(preview.contains(tail_marker));
    assert!(
        !preview.contains(&"x".repeat(60_000)),
        "中间内容不应再进上下文"
    );
    assert!(
        preview.contains("bytes. Full formatted result stored at: "),
        "{preview:.200}"
    );
    assert!(estimate_text_tokens(&preview) <= 12_500);

    let start = preview.find("stored at: ").expect("通知里有 locator") + "stored at: ".len();
    let rest = &preview[start..];
    let end = rest.find(". Use ").expect("locator 后面跟着取回指引");
    let locator = &rest[..end];
    let locator_path = std::path::Path::new(locator);
    assert!(locator_path.is_absolute(), "locator 是绝对路径：{locator}");
    assert!(
        locator_path.starts_with(dir.path().join("spill").join(session_dir_name("s1"))),
        "{locator}"
    );
    assert_eq!(fs::read_to_string(locator).unwrap(), content);

    assert!(bound_result(&store, "small", "call_2", "hello", 12_500).is_none());
    assert!(bound_result(&store, "big_tool", "call_3", &content, 0).is_none());
    assert!(bound_result(&store, "read", "call_4", &content, 12_500).is_none());
}

#[test]
fn random_prefix_is_12_hex_and_varies() {
    let dir = tempfile::tempdir().unwrap();
    let state = ToolSessionState::new(dir.path(), "s1");
    let prefix = |saved: &tools::SpillRef| {
        saved
            .locator
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .split_once('-')
            .unwrap()
            .0
            .to_owned()
    };
    let first = prefix(&state.spill().save_text("big.txt", "a").unwrap());
    let second = prefix(&state.spill().save_text("big.txt", "b").unwrap());
    for random in [&first, &second] {
        assert_eq!(random.len(), 12, "uuid v4 前 6 字节 -> 12 hex：{random}");
        assert!(
            random
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()),
            "{random}"
        );
    }
    assert_ne!(first, second, "uuid v4 entropy：两次落盘前缀应不同");
}

#[test]
fn relative_data_dir_still_yields_absolute_locator() {
    let cwd = std::env::current_dir().unwrap();
    let dir = tempfile::tempdir_in(&cwd).unwrap();
    let relative = dir.path().strip_prefix(&cwd).unwrap();
    let state = ToolSessionState::new(relative, "s-rel");
    let saved = state.spill().save_text("big.txt", "x").unwrap();
    assert!(
        saved.locator.is_absolute(),
        "相对 data-dir 也要给绝对 locator：{:?}",
        saved.locator
    );
    assert_eq!(fs::read_to_string(&saved.locator).unwrap(), "x");
}
