//! T013.1：工具结果的 diff 展示（照 DSH fs/tool-fs/src/diff.ts 的 computeHunkDiffs）。
//!
//! 上下文 3 行、一个 hunk 一条；纯插入 oldText 为 null；无尾换行的 "\\" 标记行不落进 diff。

use serde::{Deserialize, Serialize};

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

/// 计算 before/after 之间每个 hunk 的上下文 diff。
pub fn compute_hunk_diffs(path: &str, before: &str, after: &str) -> Vec<FileDiff> {
    // T013.1 红阶段：实现前空壳。
    let _ = (path, before, after);
    Vec::new()
}
