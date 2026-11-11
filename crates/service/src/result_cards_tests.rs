//! 结果卡改动清单：只认成功写类工具结果里的 diffs，去重、相对 cwd。

use std::path::Path;

use serde_json::json;
use session::{Event, EventKind, ToolError};

use crate::result_cards::{changed_line, changed_paths};

fn call(seq: u64, call_id: &str, name: &str) -> Event {
    Event::new(
        seq,
        0,
        EventKind::ToolCall {
            turn: 1,
            step: 1,
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            arguments: "{}".to_owned(),
        },
    )
}

fn result(seq: u64, call_id: &str, diffs: &[(&str, &str)], failed: bool) -> Event {
    let diffs: Vec<serde_json::Value> = diffs
        .iter()
        .map(|(path, text)| json!({"path": path, "oldText": null, "newText": text}))
        .collect();
    let message = model::Message {
        id: model::MessageId::generate(),
        role: model::Role::Tool,
        content: Vec::new(),
        source: model::MessageSource::Tool {
            call_id: call_id.to_owned(),
        },
        tool_call_id: Some(call_id.to_owned()),
        is_error: failed.then_some(true),
    };
    Event::new(
        seq,
        0,
        EventKind::ToolResult {
            turn: 1,
            step: 1,
            message,
            error: failed.then(|| ToolError {
                name: "HarnessError".to_owned(),
                code: "WRITE_OUT_OF_SCOPE".to_owned(),
            }),
            meta: Some(json!({"diffs": diffs})),
        },
    )
}

/// 成功写类工具的 diffs 才算；同一个文件多次写只留一条；cwd 外的保留绝对路径。
#[test]
fn changed_paths_takes_successful_write_diffs_once_relative_to_cwd() {
    let cwd = Path::new("/work");
    let events = vec![
        call(1, "c1", "write"),
        result(2, "c1", &[("/work/a.py", "one")], false),
        call(3, "c2", "apply_patch"),
        result(
            4,
            "c2",
            &[
                ("/work/b.py", "b"),
                ("/work/a.py", "two"),
                ("/elsewhere/c.py", "c"),
            ],
            false,
        ),
        call(5, "c3", "edit"),
        result(6, "c3", &[("/work/d.py", "d")], true),
        call(7, "c4", "read"),
        result(8, "c4", &[("/work/e.py", "e")], false),
    ];

    assert_eq!(
        changed_paths(&events, cwd),
        vec!["a.py", "b.py", "/elsewhere/c.py"],
        "只认成功写的 diffs，去重后相对 cwd"
    );
}

#[test]
fn changed_line_is_the_fixed_text() {
    assert_eq!(changed_line(2, "."), "改了 2 个文件（写入范围 .）");
    assert_eq!(
        changed_line(0, "rank/dedupe.py"),
        "改了 0 个文件（写入范围 rank/dedupe.py）"
    );
}
