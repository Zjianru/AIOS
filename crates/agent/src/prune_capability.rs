//! 工具结果修剪（T006.4，照 DSH compaction-tool-result-pruner）。
//!
//! DSH 依据：packages/compaction/compaction-tool-result-pruner/src/index.ts:136-182
//! （pruneSession：逐条当前 surface 的 tool/result，内容超预算换 head+marker+tail，
//! 先追加 compaction/prune shadow-price 事件，再追加带 surfaceOp replace 的 tool/result）、
//! config.ts:7-14（默认 thresholdChars 8192 / headChars 4096 / tailChars 1024、PRUNE_MARKER）。
//!
//! T015.3：同一事务里也认画布旧版（D58/D60）——每个画布的非最新版结果换成一句
//! 「画布「标题」第 n 版：<note>」，调用参数里的 html/edits 换成同一句；最新版不剪。
//! 调用参数与整批结果一起按调用顺序重新落盘，保证派生请求里的 tool_use/tool_result 仍配对。
//! 同批其它结果不因跟旧版同批就被豁免：超过阈值的普通结果照 T006.4 换 head+marker+tail。
//!
//! 触发：挂 HookPoint::Compaction，只在真实压缩事务里由压缩实现派发（T006.3 fix 之后），
//! loop 不再每个请求前剪。照 DSH 只跑压力/溢出两路：`compaction-basic/src/index.ts:292-347`
//! 的 pressure 与 context-overflow 才调 pruneSession；手动 `/compact`（compactNow:407）不调
//! pruner，所以 `PruneCapability` 跳过 trigger=Manual。替换写进日志（原始 tool/result 保留
//! 可回放），派生 fold 只认 surfaceOp replace 区间。

use model::ContentBlock;
use session::{CanvasState, Event, EventKind, ShadowedRange, SurfaceOp};

use crate::capabilities::{Capability, HookEvent, HookFuture, HookOutcome, HookPoint, LoopAccess};
use crate::compaction::estimate_message;
use crate::{AgentError, CompactionTrigger};

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
        let ContentBlock::Text { text, .. } = block else {
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

/// 当前 surface 上被 surfaceOp replace 藏掉的区间（照 `session::derive` 的 hidden 判定）。
fn hidden_ranges(events: &[Event]) -> Vec<(u64, u64)> {
    events
        .iter()
        .filter_map(|event| {
            event
                .surface_op
                .map(|SurfaceOp::Replace { start_seq, end_seq }| (start_seq, end_seq))
        })
        .collect()
}

/// 这个 seq 是否在某个被替换区间里。
fn is_hidden(ranges: &[(u64, u64)], seq: u64) -> bool {
    ranges
        .iter()
        .any(|(start, end)| seq >= *start && seq <= *end)
}

/// 画布旧版剪枝的一组替换：一个 assistant/message 连同它这一步的 tool/result。
///
/// 调用参数换成一句会替换整个 assistant/message（消息 id 不变），这一步的结果也要按
/// 调用顺序一起重新落盘，派生里的 tool_use/tool_result 才不会被顺序打乱。
struct CanvasPruneGroup {
    /// 被替换的 assistant/message 事件 seq。
    call_seq: u64,
    /// 该 assistant/message 事件本身（取 turn/step/文本/effort 用）。
    call: EventKind,
    /// 参数要换成一句的调用（callId → 句子）。
    pruned_calls: Vec<(String, String)>,
    /// 这一步要重新落盘的结果（按调用顺序）。
    results: Vec<CanvasPruneResult>,
}

/// 一组里要重新落盘的一条结果。
struct CanvasPruneResult {
    call_id: String,
    seq: u64,
    kind: EventKind,
    /// Some = 旧版画布：结果换成这句；None = 同批的其它结果，照常走普通剪枝
    /// （没超阈值才原样重落并保留 meta）。
    sentence: Option<String>,
}

/// 旧版结果和调用参数共用的那句（票面原文）：画布「标题」第 n 版：<note>。
fn canvas_sentence(state: &CanvasState) -> String {
    format!("画布「{}」第 {} 版：{}", state.title, state.n, state.note)
}

/// 把调用参数里的 html、edits 换成一句；其余字段（id/title/note）不动。
///
/// 参数不是 JSON 对象就不剪（画布调用一定有这两者之一，真走到这里说明日志已坏，不改写）。
fn prune_canvas_arguments(arguments: &str, sentence: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let object = value.as_object_mut()?;
    let mut changed = false;
    for key in ["html", "edits"] {
        if object.contains_key(key) {
            object.insert(
                key.to_owned(),
                serde_json::Value::String(sentence.to_owned()),
            );
            changed = true;
        }
    }
    changed.then(|| value.to_string())
}

/// 找出画布旧版：可见的、非最新版的结果，定位到对应的 assistant/message，连同整批结果。
///
/// 只认日志原事件（`session::canvas_versions` 跳过 surface 替换），所以剪过的版本不会
/// 再被认成候选；最新版（每个画布日志里最后一个版本事件）永远不剪。
fn canvas_prune_groups(events: &[Event], current_turn: Option<u64>) -> Vec<CanvasPruneGroup> {
    let versions = session::canvas_versions(events);
    if versions.is_empty() {
        return Vec::new();
    }
    // 每个画布最后一个版本事件算最新版（与 canvases() 的折叠规则一致）。
    let mut latest: Vec<(&str, u64)> = Vec::new();
    for version in &versions {
        match latest
            .iter_mut()
            .find(|(id, _)| *id == version.state.id.as_str())
        {
            Some((_, seq)) => *seq = version.seq,
            None => latest.push((version.state.id.as_str(), version.seq)),
        }
    }
    let hidden = hidden_ranges(events);
    let mut old_calls: Vec<(String, String)> = Vec::new();
    for version in &versions {
        let seq = version.seq;
        if latest
            .iter()
            .any(|(id, latest_seq)| *id == version.state.id.as_str() && *latest_seq == seq)
        {
            continue;
        }
        if is_hidden(&hidden, seq) {
            continue;
        }
        let Some(event) = events.iter().find(|event| event.seq == seq) else {
            continue;
        };
        let EventKind::ToolResult { turn, message, .. } = &event.kind else {
            continue;
        };
        if current_turn.is_some_and(|current| *turn >= current) {
            continue;
        }
        let Some(call_id) = message.tool_call_id.as_ref() else {
            continue;
        };
        old_calls.push((call_id.clone(), canvas_sentence(&version.state)));
    }
    if old_calls.is_empty() {
        return Vec::new();
    }
    let mut groups = Vec::new();
    for event in events {
        if is_hidden(&hidden, event.seq) {
            continue;
        }
        let EventKind::AssistantMessage { message, .. } = &event.kind else {
            continue;
        };
        let pruned_calls: Vec<(String, String)> = message
            .content
            .iter()
            .filter_map(|block| {
                let (id, _, _) = block.as_tool_call()?;
                old_calls.iter().find(|(call_id, _)| call_id == id).cloned()
            })
            .collect();
        if pruned_calls.is_empty() {
            continue;
        }
        // 这一步的可见结果：assistant 之后、按调用顺序第一次出现的那个。
        let call_ids: Vec<&str> = message
            .content
            .iter()
            .filter_map(|block| block.as_tool_call().map(|(id, _, _)| id))
            .collect();
        let mut results = Vec::new();
        for call_id in &call_ids {
            let Some(result) = events.iter().find(|candidate| {
                candidate.seq > event.seq
                    && !is_hidden(&hidden, candidate.seq)
                    && matches!(&candidate.kind, EventKind::ToolResult { message, .. }
                        if message.tool_call_id.as_deref() == Some(*call_id))
            }) else {
                continue;
            };
            results.push(CanvasPruneResult {
                call_id: (*call_id).to_owned(),
                seq: result.seq,
                kind: result.kind.clone(),
                sentence: old_calls
                    .iter()
                    .find(|(id, _)| id == *call_id)
                    .map(|(_, sentence)| sentence.clone()),
            });
        }
        // 旧版调用的结果必须都在，否则参数和结果只剪了一半。
        let complete = pruned_calls
            .iter()
            .all(|(call_id, _)| results.iter().any(|result| result.call_id == *call_id));
        if !complete {
            continue;
        }
        groups.push(CanvasPruneGroup {
            call_seq: event.seq,
            call: event.kind.clone(),
            pruned_calls,
            results,
        });
    }
    groups
}

/// 追加一条 shadow-price 事件（照 T006.4：先记被替换节点，再写 surfaceOp 替换）。
async fn append_prune(host: &dyn LoopAccess, seq: u64, token_count: u64) -> Result<(), AgentError> {
    let _ = host
        .append_with_seqs(&[EventKind::CompactionPrune {
            shadowed_range: ShadowedRange {
                start: seq,
                end: seq,
            },
            shadowed_seqs: vec![seq],
            shadowed_token_count: token_count,
        }])
        .await?;
    Ok(())
}

/// 落一组画布旧版替换：先调用参数换一句，再按调用顺序重落这一步的结果。
///
/// 返回这组剪掉的节点数（旧版画布结果 + 命中 T006.4 阈值的同批普通结果）；参数换不动、
/// 这一组什么都没写时返回 0。
async fn apply_canvas_group(
    host: &dyn LoopAccess,
    group: &CanvasPruneGroup,
) -> Result<usize, AgentError> {
    let EventKind::AssistantMessage {
        turn,
        step,
        message,
        interrupted,
        effort,
        effort_wanted,
        ..
    } = &group.call
    else {
        return Ok(0);
    };
    let mut replacement = message.clone();
    let mut changed = false;
    for block in &mut replacement.content {
        let ContentBlock::ToolCall { id, arguments, .. } = block else {
            continue;
        };
        let Some((_, sentence)) = group.pruned_calls.iter().find(|(call_id, _)| call_id == id)
        else {
            continue;
        };
        if let Some(pruned) = prune_canvas_arguments(arguments, sentence) {
            *arguments = pruned;
            changed = true;
        }
    }
    if !changed {
        return Ok(0);
    }
    append_prune(host, group.call_seq, estimate_message(message)).await?;
    host.append_surface(
        EventKind::AssistantMessage {
            turn: *turn,
            step: *step,
            message: replacement,
            // 替换只服务模型视图：usage/stream 是原事件的事实，不在这里重记一次。
            stream: Vec::new(),
            usage: None,
            interrupted: *interrupted,
            effort: effort.clone(),
            effort_wanted: effort_wanted.clone(),
        },
        SurfaceOp::Replace {
            start_seq: group.call_seq,
            end_seq: group.call_seq,
        },
        vec![group.call_seq],
    )
    .await?;
    let mut pruned_count = 0;
    for result in &group.results {
        let EventKind::ToolResult {
            turn,
            step,
            message,
            error,
            meta,
        } = &result.kind
        else {
            // 组里只放 ToolResult（建组时已筛过）；真出现别的就跳过这一条，不再往下走半套。
            continue;
        };
        let token_count = estimate_message(message);
        // 旧版换一句；同批其它结果照常走 T006.4：超过阈值就换 head+marker+tail，
        // 没超才原样重落并保留 meta（页面工作过程照旧）。
        let (replacement, meta, pruned) = match &result.sentence {
            Some(sentence) => {
                let mut replacement = message.clone();
                replacement.content = vec![ContentBlock::text(sentence.clone())];
                (replacement, None, true)
            }
            None => match prune_content(&message.content) {
                Some(content) => {
                    let mut replacement = message.clone();
                    replacement.content = content;
                    (replacement, None, true)
                }
                None => (message.clone(), meta.clone(), false),
            },
        };
        if pruned {
            pruned_count += 1;
        }
        append_prune(host, result.seq, token_count).await?;
        host.append_surface(
            EventKind::ToolResult {
                turn: *turn,
                step: *step,
                message: replacement,
                error: error.clone(),
                meta,
            },
            SurfaceOp::Replace {
                start_seq: result.seq,
                end_seq: result.seq,
            },
            vec![result.seq],
        )
        .await?;
    }
    Ok(pruned_count)
}

/// 收集当前 surface 上可修剪的 tool/result：跳过已被更晚 surfaceOp replace 藏掉的节点；
/// current_turn 为 Some 时只剪更早 turn 的结果（T005.2 过渡行为，T006.4 实现期为 None）。
fn prunable_tool_results(events: &[Event], current_turn: Option<u64>) -> Vec<(u64, EventKind)> {
    let hidden = hidden_ranges(events);
    events
        .iter()
        .filter(|event| !is_hidden(&hidden, event.seq))
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

/// 跑一遍修剪：画布旧版换一句、普通大结果换 head+marker+tail；每条替换都先追加
/// compaction/prune shadow-price 事件，再追加带 surfaceOp replace 的替换事件
/// （照 DSH 的 shadow-price 协议）；返回剪掉的节点数。
pub(crate) async fn prune_session(
    host: &dyn LoopAccess,
    current_turn: Option<u64>,
) -> Result<usize, AgentError> {
    let (canvas_groups, mut candidates) = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        let events = store.events();
        let groups = canvas_prune_groups(events, current_turn);
        // 画布旧版结果按「一句」处理，不走普通剪枝（否则同一 seq 会有两条替换）；
        // 同批 sentence=None 的兄弟结果不算画布结果，候选照收，随后在 group 内剪。
        let canvas_sentence_seqs: Vec<u64> = groups
            .iter()
            .flat_map(|group| {
                group
                    .results
                    .iter()
                    .filter(|result| result.sentence.is_some())
                    .map(|result| result.seq)
            })
            .collect();
        let candidates: Vec<(u64, EventKind)> = prunable_tool_results(events, current_turn)
            .into_iter()
            .filter(|(seq, _)| !canvas_sentence_seqs.contains(seq))
            .collect();
        (groups, candidates)
    };
    let mut pruned_count = 0;
    for group in &canvas_groups {
        pruned_count += apply_canvas_group(host, group).await?;
    }
    // 同批的 sentence=None 兄弟结果已随 group 重落，原 seq 被 surfaceOp 藏掉；候选是在
    // 重落之前收的，跳过这些过期节点，避免给同一个 seq 再落一条替换（派生视图会多一份结果）。
    if !canvas_groups.is_empty() {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        let hidden = hidden_ranges(store.events());
        candidates.retain(|(seq, _)| !is_hidden(&hidden, *seq));
    }
    for (seq, kind) in candidates {
        let EventKind::ToolResult {
            turn,
            step,
            message,
            error,
            ..
        } = kind
        else {
            continue;
        };
        let Some(content) = prune_content(&message.content) else {
            continue;
        };
        append_prune(host, seq, estimate_message(&message)).await?;
        let mut replacement = message.clone();
        replacement.content = content;
        host.append_surface(
            EventKind::ToolResult {
                turn,
                step,
                message: replacement,
                error,
                // 修剪后的结果不再带展示元信息（卡片按普通结果渲染）。
                meta: None,
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
            if let HookEvent::Compaction { host, trigger, .. } = event {
                // 照 DSH：只有压力/溢出两路跑 pruner；手动 /compact（compactNow）跳过。
                if *trigger != CompactionTrigger::Manual {
                    // 修剪失败不打断压缩事务（DSH 也是尽力而为的 model-free pass；
                    // 压缩自己的错误仍按原路径上报）。
                    let _ = prune_session(*host, None).await;
                }
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
