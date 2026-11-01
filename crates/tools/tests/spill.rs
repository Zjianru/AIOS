//! 接缝 2：大结果 spill 的存储与保留（测试清单第 8 条）。

use std::fs;

use tools::{ToolSessionState, bound_result, estimate_text_tokens};

#[test]
fn spill_store_isolated_per_session() {
    let dir = tempfile::tempdir().unwrap();
    let a = ToolSessionState::new(dir.path(), "session-a");
    let b = ToolSessionState::new(dir.path(), "session-b");

    let ref_a = a.spill().save_text("big", "alpha content").unwrap();
    let ref_b = b.spill().save_text("big", "beta content").unwrap();

    assert_ne!(ref_a.locator, ref_b.locator);
    assert!(
        ref_a
            .locator
            .starts_with(dir.path().join("spill").join("session-session-a")),
        "{:?}",
        ref_a.locator
    );
    assert!(
        ref_b
            .locator
            .starts_with(dir.path().join("spill").join("session-session-b")),
        "{:?}",
        ref_b.locator
    );
    assert_eq!(fs::read_to_string(&ref_a.locator).unwrap(), "alpha content");
    assert_eq!(fs::read_to_string(&ref_b.locator).unwrap(), "beta content");
    assert_eq!(ref_a.bytes, "alpha content".len());
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
    assert_eq!(fs::read_to_string(locator).unwrap(), content);

    assert!(bound_result(&store, "small", "call_2", "hello", 12_500).is_none());
    assert!(bound_result(&store, "big_tool", "call_3", &content, 0).is_none());
    assert!(bound_result(&store, "read", "call_4", &content, 12_500).is_none());
}
