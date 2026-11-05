use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use model::{AssistantStreamAccumulator, BlockType, ContentBlock, Message, Usage};
use session::EventKind;
use tools::ToolSet;

use crate::AgentEvent;
use crate::error::AgentError;

use super::*;

/// 一条助手消息：source 是本次产出它的模型路由（DSH AssistantMessage 的 source=model）。
pub(super) fn assistant_message(inner: &Arc<Inner>, content: Vec<ContentBlock>) -> Message {
    Message::assistant(
        content,
        inner.model.provider(),
        inner.model.config().model.clone(),
    )
}

pub(super) fn emit_usage(inner: &Arc<Inner>, usage: &Option<Usage>) {
    if let Some(usage) = usage {
        let _ = inner.events.send(AgentEvent::UsageReported {
            usage: usage.clone(),
        });
    }
}

/// 取消时收尾一次已开始的尝试（DSH agent.ts:499-533）：有可见前缀写 interrupted 的
/// assistant/message；没有可见内容写 assistant/attempt（帧保留，不留空消息）。
pub(super) async fn interrupted_step(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    blocks: &BTreeMap<usize, ContentBlock>,
    open: &BTreeMap<usize, (BlockType, String)>,
    usage: &Option<Usage>,
    stream: &AssistantStreamAccumulator,
) -> Result<StepEnd, AgentError> {
    let content = interrupted_content(blocks, open);
    if content.is_empty() {
        append_attempt(inner, turn, step, stream).await?;
    } else {
        let message = assistant_message(inner, content);
        inner
            .append(&[EventKind::AssistantMessage {
                turn,
                step,
                message,
                stream: stream.snapshot(),
                usage: usage.clone(),
                interrupted: Some(true),
            }])
            .await?;
        emit_usage(inner, usage);
    }
    Ok(StepEnd::Aborted)
}

/// 把一次没有产出可见消息的尝试落成 assistant/attempt（DSH agent.ts:567-580）。
pub(super) async fn append_attempt(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    stream: &AssistantStreamAccumulator,
) -> Result<(), AgentError> {
    inner
        .append(&[EventKind::AssistantAttempt {
            turn,
            step,
            stream: stream.snapshot(),
        }])
        .await
}

/// 当前 Unix 毫秒时间（会话事件/流帧共用；系统时钟倒退时取 0）。
pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// 取消时保留的可安全收尾前缀（照 DSH assembler.interruptedBlocks）：
/// 已关闭/打开中的 text 和 reasoning 块、去空白非空、按流顺序；tool-call 丢弃。
/// interrupted 的 thinking 不写 signature（照 DSH，handback 记）。
pub(super) fn interrupted_content(
    blocks: &BTreeMap<usize, ContentBlock>,
    open: &BTreeMap<usize, (BlockType, String)>,
) -> Vec<ContentBlock> {
    let mut indexes: Vec<usize> = blocks.keys().chain(open.keys()).copied().collect();
    indexes.sort_unstable();
    indexes.dedup();
    let mut content = Vec::new();
    for index in indexes {
        let block = blocks.get(&index).cloned().or_else(|| {
            open.get(&index).map(|(block_type, text)| match block_type {
                BlockType::Text => ContentBlock::Text { text: text.clone() },
                BlockType::Reasoning => ContentBlock::Reasoning {
                    text: text.clone(),
                    signature: None,
                },
                BlockType::ToolCall => ContentBlock::Text { text: text.clone() },
            })
        });
        match block {
            Some(ContentBlock::Text { text }) if !text.trim().is_empty() => {
                content.push(ContentBlock::Text { text });
            }
            Some(ContentBlock::Reasoning { text, .. }) if !text.trim().is_empty() => {
                content.push(ContentBlock::Reasoning {
                    text,
                    signature: None,
                });
            }
            _ => {}
        }
    }
    content
}

/// 系统提示：固定文字 + 会话的工作目录；按可见工具补指导句（照 DSH 的 tool-fs 各工具
/// 系统提示段，插件名换成 AIOS 的规则表述）。
pub(super) fn system_prompt(cwd: &Path, tools: &ToolSet) -> String {
    let mut prompt = format!(
        "You are AIOS, a coding agent. Your working directory is {}.",
        cwd.display()
    );
    if tools.get("read").is_some() {
        prompt.push_str(
            "\n\nUse the read tool — not shell commands like cat — to inspect text files. Use offset and limit to continue reading large files.",
        );
    }
    if tools.get("bash").is_some() {
        prompt.push_str(
            "\n\nCheck the [exit code: N] marker on every bash result; investigate failures before moving on.",
        );
    }
    if tools.get("job_output").is_some() {
        prompt.push_str(
            "\n\nTrack every background job id you start. You are notified in-session when a job finishes — do not busy-poll or sleep on one; keep working on independent steps and do not duplicate a running job's work. Before giving a final answer, collect every still-relevant job with job_output (set wait: true only when you are genuinely blocked on it), and job_kill jobs that stopped mattering.",
        );
    }
    if tools.get("write").is_some() {
        prompt.push_str("\n\nRead an existing file before overwriting it with write");
        if tools.get("edit").is_some() {
            prompt.push_str("; prefer edit for targeted changes");
        }
        prompt.push('.');
    }
    if tools.get("edit").is_some() {
        prompt.push_str(
            "\n\nRead a file before editing it, unless you just created or edited it in this session.",
        );
    }
    if tools.get("apply_patch").is_some() {
        prompt.push_str(
            "\n\nPrefer apply_patch over separate edit calls for changes across files or separated regions. Read a file before updating, deleting, or moving it, unless you just created or edited it in this session. The whole patch is checked first and a rejected patch changes nothing; once applying starts, operations run in order, stop at the first failure, and are not rolled back, so re-read affected files before retrying when needed.",
        );
    }
    prompt
}
