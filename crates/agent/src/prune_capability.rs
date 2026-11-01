//! 工具结果修剪（T006.4，照 DSH compaction-tool-result-pruner）。
//!
//! DSH 依据：packages/compaction/compaction-tool-result-pruner/src/index.ts:136-182
//! （pruneSession：逐条当前 surface 的 tool/result，内容超预算换 head+marker+tail，
//! 先追加 compaction/prune shadow-price 事件，再追加带 surfaceOp replace 的 tool/result）、
//! config.ts:7-14（默认 thresholdChars 8192 / headChars 4096 / tailChars 1024、PRUNE_MARKER）。
//!
//! 触发：挂 HookPoint::Compaction，只在真实压缩事务里由压缩实现派发（T006.3 fix 之后），
//! loop 不再每个请求前剪。替换写进日志（原始 tool/result 保留可回放），派生 fold 只认
//! surfaceOp replace 区间。

use model::ContentBlock;
use session::{Event, EventKind, ShadowedRange, SurfaceOp};

use crate::AgentError;
use crate::capabilities::{Capability, HookEvent, HookFuture, HookOutcome, HookPoint, LoopAccess};

/// 超过这个码点数才修剪（DSH DEFAULTS.thresholdChars）。
pub(crate) const PRUNE_THRESHOLD_CHARS: usize = 8192;
/// 保留的头部长（码点；DSH DEFAULTS.headChars）。
pub(crate) const PRUNE_HEAD_CHARS: usize = 4096;
/// 保留的尾部长度（码点；DSH DEFAULTS.tailChars）。
pub(crate) const PRUNE_TAIL_CHARS: usize = 1024;
/// 中间被删掉的位置插的标记（照 DSH PRUNE_MARKER 原文）。
pub(crate) const PRUNE_MARKER: &str = "\n\n[... tool result middle pruned ...]\n\n";

/// 单条 tool/result 的 content 超过阈值时返回 head+marker+tail 的修剪视图（照 DSH
/// pruneContent）：只对 text 块计码点并按码点切分；非 text 块原位保留；marker 只插一次。
/// 没超阈值返回 None。
pub(crate) fn prune_content(blocks: &[ContentBlock]) -> Option<Vec<ContentBlock>> {
    let total_chars: usize = blocks
        .iter()
        .filter_map(ContentBlock::as_text)
        .map(|text| text.chars().count())
        .sum();
    if total_chars <= PRUNE_THRESHOLD_CHARS {
        return None;
    }
    let removed_start = PRUNE_HEAD_CHARS;
    let removed_end = total_chars - PRUNE_TAIL_CHARS;
    let mut pruned: Vec<ContentBlock> = Vec::with_capacity(blocks.len());
    let mut consumed = 0_usize;
    let mut marker_inserted = false;
    for block in blocks {
        let ContentBlock::Text { text } = block else {
            pruned.push(block.clone());
            continue;
        };
        let points: Vec<char> = text.chars().collect();
        let block_start = consumed;
        let block_end = block_start + points.len();
        let head_end = points.len().min(removed_start.saturating_sub(block_start));
        let tail_start = points.len().min(removed_end.saturating_sub(block_start));
        let intersects = block_start < removed_end && block_end > removed_start;
        let marker = if intersects && !marker_inserted {
            marker_inserted = true;
            PRUNE_MARKER
        } else {
            ""
        };
        let head: String = points[..head_end].iter().collect();
        let tail: String = points[tail_start..].iter().collect();
        let text = format!("{head}{marker}{tail}");
        if !text.is_empty() {
            pruned.push(ContentBlock::text(text));
        }
        consumed = block_end;
    }
    debug_assert!(marker_inserted, "超过阈值时必须插一次 marker");
    let chars_after: usize = pruned
        .iter()
        .filter_map(ContentBlock::as_text)
        .map(|text| text.chars().count())
        .sum();
    debug_assert!(
        chars_after <= PRUNE_THRESHOLD_CHARS && chars_after < total_chars,
        "修剪结果必须更小且在阈值内"
    );
    Some(pruned)
}

/// 收集当前 surface 上可修剪的 tool/result：跳过已被更晚 surfaceOp replace 藏掉的节点；
/// current_turn 为 Some 时只剪更早 turn 的结果（T005.2 过渡行为，T006.4 实现期为 None）。
fn prunable_tool_results(events: &[Event], current_turn: Option<u64>) -> Vec<(u64, EventKind)> {
    let mut shadowed: Vec<(u64, u64)> = Vec::new();
    for event in events {
        if let Some(SurfaceOp::Replace { start_seq, end_seq }) = event.surface_op {
            shadowed.push((start_seq, end_seq));
        }
    }
    let hidden = |seq: u64| {
        shadowed
            .iter()
            .any(|(start, end)| seq >= *start && seq <= *end)
    };
    events
        .iter()
        .filter(|event| !hidden(event.seq))
        .filter_map(|event| {
            let EventKind::ToolResult { turn, message, .. } = &event.kind else {
                return None;
            };
            if current_turn.is_some_and(|current| *turn >= current) {
                return None;
            }
            prune_content(&message.content)?;
            Some((event.seq, event.kind.clone()))
        })
        .collect()
}

/// 跑一遍修剪：逐条可剪结果追加 compaction/prune 事件 + 带 surfaceOp replace 的
/// tool/result 替换事件（照 DSH 的 shadow-price 协议）；返回剪掉的节点数。
pub(crate) async fn prune_session(
    host: &dyn LoopAccess,
    current_turn: Option<u64>,
) -> Result<usize, AgentError> {
    let candidates = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        prunable_tool_results(store.events(), current_turn)
    };
    let mut pruned_count = 0;
    for (seq, kind) in candidates {
        let EventKind::ToolResult {
            turn,
            step,
            message,
            error,
        } = kind
        else {
            continue;
        };
        let Some(content) = prune_content(&message.content) else {
            continue;
        };
        let shadowed_token_count = crate::compaction::estimate_message(&message);
        host.append_with_seqs(&[EventKind::CompactionPrune {
            shadowed_range: ShadowedRange {
                start: seq,
                end: seq,
            },
            shadowed_seqs: vec![seq],
            shadowed_token_count,
        }])
        .await?;
        let mut replacement = message.clone();
        replacement.content = content;
        host.append_surface(
            EventKind::ToolResult {
                turn,
                step,
                message: replacement,
                error,
            },
            SurfaceOp::Replace {
                start_seq: seq,
                end_seq: seq,
            },
            vec![seq],
        )
        .await?;
        pruned_count += 1;
    }
    Ok(pruned_count)
}

/// 修剪能力：挂 HookPoint::Compaction，随真实压缩事务（自动/溢出/手动三路）触发。
pub(crate) struct PruneCapability;

impl Capability for PruneCapability {
    fn name(&self) -> &'static str {
        "prune"
    }
    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::Compaction]
    }
    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            if let HookEvent::Compaction { host, .. } = event {
                // 修剪失败不打断压缩事务（DSH 也是尽力而为的 model-free pass；
                // 压缩自己的错误仍按原路径上报）。
                let _ = prune_session(*host, None).await;
            }
            Ok(HookOutcome::Continue)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_blocks(parts: &[&str]) -> Vec<ContentBlock> {
        parts.iter().map(|part| ContentBlock::text(*part)).collect()
    }

    #[test]
    fn keeps_head_and_tail_and_inserts_marker() {
        let text = format!("{}MIDDLE{}", "a".repeat(5000), "b".repeat(5000));
        let pruned = prune_content(&text_blocks(&[&text])).expect("超过阈值应当修剪");
        let joined: String = pruned.iter().filter_map(ContentBlock::as_text).collect();
        assert!(joined.starts_with(&"a".repeat(4096)));
        assert!(joined.ends_with(&"b".repeat(1024)));
        assert!(joined.contains(PRUNE_MARKER));
        assert!(!joined.contains("MIDDLE"));
    }

    #[test]
    fn small_text_is_untouched() {
        let text = "x".repeat(8192);
        assert!(prune_content(&text_blocks(&[&text])).is_none());
    }

    #[test]
    fn non_text_blocks_keep_order_and_position() {
        let blocks = vec![
            ContentBlock::text("a".repeat(5000)),
            ContentBlock::Reasoning {
                text: "thinking".to_owned(),
                signature: None,
            },
            ContentBlock::text("b".repeat(5000)),
        ];
        let pruned = prune_content(&blocks).expect("超过阈值应当修剪");
        assert_eq!(pruned.len(), 3, "非文本块原位保留");
        assert!(matches!(
            &pruned[1],
            ContentBlock::Reasoning { text, .. } if text == "thinking"
        ));
    }
}
