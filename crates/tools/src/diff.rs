//! T013.1：工具结果的 diff 展示（照 DSH fs/tool-fs/src/diff.ts 的 computeHunkDiffs）。
//!
//! 上下文 3 行、一个 hunk 一条；纯插入 oldText 为 null；无尾换行的 "\\" 标记行不落进 diff。

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

/// 每个 hunk 每侧显示的上下文行数（DSH DIFF_CONTEXT）。
pub const DIFF_CONTEXT: usize = 3;

/// 一条文件 diff（落进 tool/result.meta.diffs 的形状；DSH FileDiff）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    #[serde(rename = "oldText")]
    pub old_text: Option<String>,
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// 计算 before/after 之间每个 hunk 的上下文 diff（一个 hunk 一条，按文件顺序）。
pub fn compute_hunk_diffs(path: &str, before: &str, after: &str) -> Vec<FileDiff> {
    let diff = TextDiff::from_lines(before, after);
    let mut diffs = Vec::new();
    for group in diff.grouped_ops(DIFF_CONTEXT) {
        let mut old_lines: Vec<String> = Vec::new();
        let mut new_lines: Vec<String> = Vec::new();
        for op in group {
            for change in diff.iter_changes(&op) {
                // similar 的行带尾换行；diff 块的文本不带。
                let text = change.value().trim_end_matches('\n');
                match change.tag() {
                    ChangeTag::Delete => old_lines.push(text.to_owned()),
                    ChangeTag::Insert => new_lines.push(text.to_owned()),
                    ChangeTag::Equal => {
                        old_lines.push(text.to_owned());
                        new_lines.push(text.to_owned());
                    }
                }
            }
        }
        // 没有旧行时是纯插入（新建）：oldText 为 null，和 DSH 一致。
        diffs.push(FileDiff {
            path: path.to_owned(),
            old_text: (!old_lines.is_empty()).then(|| old_lines.join("\n")),
            new_text: new_lines.join("\n"),
        });
    }
    diffs
}
