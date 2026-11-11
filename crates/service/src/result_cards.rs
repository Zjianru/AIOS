//! 写任务的结果卡：改动清单和固定文字。
//!
//! 清单取任务所有 Attempt 里 write、edit、apply_patch 成功结果的 `meta.diffs[].path`，
//! 去重后相对 cwd 显示；只读调查任务没有清单。

use std::path::Path;

use session::Event;

/// 写类工具的 diffs 里取改动路径：只认成功结果，按出现顺序去重，相对 cwd。
pub(crate) fn changed_paths(_events: &[Event], _cwd: &Path) -> Vec<String> {
    Vec::new()
}

/// 卡上固定一行：改了 N 个文件（写入范围 X）。
pub(crate) fn changed_line(_count: usize, _scope: &str) -> String {
    String::new()
}
