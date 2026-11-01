//! 旧工具结果修剪（照 DSH compaction-tool-result-pruner 的 head/middle/tail 语义）。
//!
//! 纯文本函数：只决定「这段结果要不要修剪、修剪后长什么样」；追加事件与派生投影
//! 在 driver/session 里。阈值取 DSH 默认 8192 码点、头 4096、尾 1024。

/// 超过这个码点数才修剪。
pub(crate) const PRUNE_THRESHOLD_CHARS: usize = 8192;
/// 保留的头部长（码点）。
pub(crate) const PRUNE_HEAD_CHARS: usize = 4096;
/// 保留的尾部长度（码点）。
pub(crate) const PRUNE_TAIL_CHARS: usize = 1024;
/// 中间被删掉的位置插的标记（照 DSH PRUNE_MARKER 原文）。
pub(crate) const PRUNE_MARKER: &str = "\n\n[... tool result middle pruned ...]\n\n";

/// 超过阈值时返回 head + marker + tail；没超返回 None。
pub(crate) fn prune_text(text: &str) -> Option<String> {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= PRUNE_THRESHOLD_CHARS {
        return None;
    }
    let head: String = characters.iter().take(PRUNE_HEAD_CHARS).collect();
    let tail_start = characters
        .len()
        .saturating_sub(PRUNE_TAIL_CHARS)
        .max(PRUNE_HEAD_CHARS);
    let tail: String = characters[tail_start..].iter().collect();
    Some(format!("{head}{PRUNE_MARKER}{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_head_and_tail() {
        let text = format!("{}MIDDLE{}", "a".repeat(5000), "b".repeat(5000));
        let pruned = prune_text(&text).expect("超过阈值应当修剪");
        assert!(pruned.starts_with(&"a".repeat(4096)));
        assert!(pruned.ends_with(&"b".repeat(1024)));
        assert!(pruned.contains(PRUNE_MARKER));
        assert!(!pruned.contains("MIDDLE"));
    }

    #[test]
    fn small_text_is_untouched() {
        assert!(prune_text(&"x".repeat(8192)).is_none());
    }
}
