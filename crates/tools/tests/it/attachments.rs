//! T013.2：附件仓库与文字判断（清单 4、7）——照 DSH file-store.ts / admission.ts 与 git 的 8000 字节规则。

use std::fs;
use std::os::unix::fs::PermissionsExt;

use tools::attachments::{AttachmentError, is_text, read, store};

// ── 文字判断（清单 4）────────────────────────────────────────────

#[test]
fn empty_is_text() {
    assert!(is_text(b""), "空文件算文字");
}

#[test]
fn utf8_text_is_text() {
    assert!(is_text("你好，AIOS\n".as_bytes()));
}

#[test]
fn bom_text_is_text() {
    assert!(is_text(b"\xEF\xBB\xBFhello\n"));
}

#[test]
fn nul_in_first_8000_is_not_text() {
    assert!(!is_text(b"abc\0def"));
}

#[test]
fn nul_at_byte_8000_is_not_text() {
    // 第 8000 个字节（下标 7999）是 NUL：前 8000 字节里有它 → 不是文字。
    let mut data = vec![b'a'; 7999];
    data.push(0);
    assert!(!is_text(&data));
}

#[test]
fn nul_at_byte_8001_is_text() {
    // 前 8000 字节全干净，第 8001 个字节（下标 8000）才是 NUL → 仍是文字（照 git）。
    let mut data = vec![b'a'; 8000];
    data.push(0);
    assert!(is_text(&data));
}

// ── 附件仓库（清单 7）────────────────────────────────────────────

fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// file-objects 下的对象文件数。
fn object_count(root: &std::path::Path) -> usize {
    fn walk(dir: &std::path::Path, count: &mut usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, count);
            } else {
                *count += 1;
            }
        }
    }
    let mut count = 0;
    walk(&root.join("attachments/v1/file-objects"), &mut count);
    count
}

#[test]
fn same_content_gets_the_same_id_and_one_object() {
    let dir = root();
    let first = store(dir.path(), "a.txt", b"hello\n").expect("存第一份");
    let second = store(dir.path(), "b.txt", b"hello\n").expect("再存一份同内容");
    assert_eq!(first.id, second.id, "同内容 id 一样");
    assert_eq!(object_count(dir.path()), 1, "内容只存一份");
    assert_eq!(first.name, "a.txt");
    assert!(first.path.ends_with("a.txt"), "命名副本：{:?}", first.path);
}

#[test]
fn named_copy_is_read_only() {
    let dir = root();
    let stored = store(dir.path(), "notes.txt", b"hello\n").expect("存");
    let mode = fs::metadata(&stored.path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o444, "副本只读：{mode:o}");
}

#[test]
fn object_file_is_read_only() {
    let dir = root();
    let stored = store(dir.path(), "a.txt", b"hello\n").expect("存");
    let hex = stored.id.strip_prefix("sha256:").expect("id 前缀");
    let object = dir
        .path()
        .join("attachments/v1/file-objects")
        .join(&hex[..2])
        .join(hex);
    let mode = fs::metadata(&object).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o444, "内容对象只读：{mode:o}");
}

#[test]
fn binary_is_rejected_as_not_text() {
    let dir = root();
    let error = store(dir.path(), "bin.dat", b"abc\0def").expect_err("二进制要拒收");
    assert!(
        matches!(error, AttachmentError::NotText),
        "错误码 not-text：{error}"
    );
    assert_eq!(object_count(dir.path()), 0, "拒收不落盘");
}

#[test]
fn bad_name_is_rejected() {
    let dir = root();
    for name in ["", "a/b", "..", "a\\b"] {
        let error = store(dir.path(), name, b"hello").expect_err("坏名字要拒收");
        assert!(
            matches!(error, AttachmentError::BadName(_)),
            "{name:?} → {error}"
        );
    }
}

#[test]
fn bad_id_is_rejected() {
    let dir = root();
    for id in ["sha256:xx", "md5:0123", "sha256:"] {
        let error = read(dir.path(), id).expect_err("坏 id 要拒收");
        assert!(
            matches!(error, AttachmentError::BadId(_)),
            "{id:?} → {error}"
        );
    }
}

#[test]
fn store_then_read_roundtrip() {
    let dir = root();
    let stored = store(dir.path(), "greeting.txt", "你好\n".as_bytes()).expect("存");
    let loaded = read(dir.path(), &stored.id).expect("读回");
    assert_eq!(loaded.id, stored.id);
    assert_eq!(loaded.name, "greeting.txt");
    assert_eq!(loaded.bytes, stored.bytes);
    assert_eq!(
        fs::read(&loaded.path).unwrap(),
        "你好\n".as_bytes(),
        "内容一致"
    );
}

#[test]
fn different_names_share_one_object_but_keep_both_copies() {
    let dir = root();
    let a = store(dir.path(), "a.txt", b"same\n").expect("a");
    let b = store(dir.path(), "b.txt", b"same\n").expect("b");
    assert_eq!(a.id, b.id);
    assert_eq!(object_count(dir.path()), 1, "内容还是一份");
    assert_ne!(a.path, b.path, "两个命名副本");
    assert_eq!(fs::read(&a.path).unwrap(), fs::read(&b.path).unwrap());
}
