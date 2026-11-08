use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use model::{
    AssistantStreamAccumulator, BlockType, CancelToken, ContentBlock, CredentialContext, ErrorCode,
    FinishReason, Message, ModelError, StreamChunk, TimedStreamChunk, Usage,
};
use session::{EventKind, RetryId, ToolError};
use tools::{ToolCall, ToolContext, execute_batch};

use crate::AgentEvent;
use crate::capabilities::HookEvent;
use crate::error::AgentError;

use super::helpers::*;
use super::retry::*;
use super::*;

/// 一次 step 的请求上下文：派生消息、工具请求、本步开出的工具名、取消令牌。
pub(super) struct StepContext {
    messages: Vec<Message>,
    tool_request: model::ToolRequest,
    /// 本步按条件开出的工具名；没开出的既不在表里，也不能执行。
    enabled: Arc<std::collections::BTreeSet<String>>,
    cancel: CancelToken,
}

pub(super) async fn prepare_step(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    claimed: Vec<Message>,
    abort: &Arc<AtomicBool>,
) -> Result<Option<StepContext>, AgentError> {
    if abort.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let has_system = inner
        .store
        .lock()
        .expect("agent 会话锁被毒化")
        .has_system_message();
    if !has_system {
        let message = Message::system_prompt(system_prompt(&inner.cwd, &inner.tools));
        inner
            .append(&[EventKind::SystemMessage {
                turn,
                step,
                message,
            }])
            .await?;
    }
    let claimed_messages = claimed.clone();
    for message in claimed {
        inner
            .append(&[EventKind::UserMessage {
                turn,
                step,
                message,
            }])
            .await?;
    }

    // T006.1c：PreStep 两阶段——Inject（instructions/skill 注入）在旧位置，再派发 Compact
    // （压缩自动检查）。prune 已挂到 Compaction 挂点（T006.4），不再每请求剪、也不参与
    // PreStep 顺序；真正的修剪只发生在压缩事务里。
    inner
        .capabilities
        .dispatch(&HookEvent::PreStep {
            turn,
            step,
            host: inner.as_ref(),
            messages: &claimed_messages,
            trigger: None,
            phase: crate::capabilities::PreStepPhase::Inject,
        })
        .await?;
    // T005.8a：请求前检查 token 压力；越过阈值就压缩较早历史（失败不打断本轮，事件已记录）。
    // T006.1b：压缩实现已移出 loop，这里只派发固定挂点；压缩能力在 should_trigger/
    // select_split 确认后才会真正压缩，否则 no-op。
    inner
        .capabilities
        .dispatch(&HookEvent::PreStep {
            turn,
            step,
            host: inner.as_ref(),
            messages: &claimed_messages,
            trigger: None,
            phase: crate::capabilities::PreStepPhase::Compact,
        })
        .await?;
    // T014.2：工具的条件每步算一次；本步新开出的工具记进日志（只在新增时记一条，带完整定义）。
    // 条件里的工具凭证也每步查一次：设置页换了 key，下一步就跟着变。
    let loaded_skills = {
        let store = inner.store.lock().expect("agent 会话锁被毒化");
        crate::tool_gate::loaded_skills(store.events())
    };
    let tool_keys = inner.tool_keys.present();
    let enabled = {
        let gate = inner.tool_gate.lock().expect("加载记录锁被毒化");
        gate.enabled(&inner.tools, &loaded_skills, &tool_keys)
    };
    let additions = {
        let gate = inner.tool_gate.lock().expect("加载记录锁被毒化");
        gate.additions(&inner.tools, &enabled)
    };
    if !additions.is_empty() {
        inner
            .append(&[EventKind::ToolRegistry {
                turn,
                step,
                additions: additions.clone(),
            }])
            .await?;
        inner
            .tool_gate
            .lock()
            .expect("加载记录锁被毒化")
            .commit(&additions);
    }
    let tool_schemas = inner.tools.definitions_for(&enabled);
    let enabled = Arc::new(enabled);
    let (messages, tool_history) = {
        let store = inner.store.lock().expect("agent 会话锁被毒化");
        let derived = store.derive_messages_with_seqs();
        let history = crate::tool_gate::tool_history(store.events(), &derived);
        (
            derived
                .into_iter()
                .map(|(_, message)| message)
                .collect::<Vec<_>>(),
            history,
        )
    };
    let tool_request = model::ToolRequest {
        tools: tool_schemas,
        history: tool_history,
    };
    // 取消令牌共用 agent 的 abort 标志 + Notify：等响应头阶段也能被打断。
    let cancel = CancelToken::new(Arc::clone(abort), Arc::clone(&inner.cancel_notify));
    Ok(Some(StepContext {
        messages,
        tool_request,
        enabled,
        cancel,
    }))
}

/// 一个 step：写 system/user 事件，从日志派生请求，流式收模型输出，
/// 有工具调用就派发批执行并写 tool/call、tool/result，否则收尾这一轮。
pub(super) async fn run_step(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    claimed: Vec<Message>,
    abort: &Arc<AtomicBool>,
) -> Result<StepEnd, AgentError> {
    let Some(context) = prepare_step(inner, turn, step, claimed, abort).await? else {
        return Ok(StepEnd::Aborted);
    };
    let StepContext {
        mut messages,
        tool_request,
        enabled,
        cancel,
    } = context;
    // 已完成的块按 index 存；打开中的 text/reasoning 块累计增量，取消时按 DSH
    // interruptedBlocks 的规则拼出可安全收尾的前缀（text/reasoning、去空白非空、
    // tool-call 丢弃）。
    let mut blocks: BTreeMap<usize, ContentBlock> = BTreeMap::new();
    let mut open: BTreeMap<usize, (BlockType, String)> = BTreeMap::new();
    // T015.5：模型流里的 name 只在工具调用的起始帧给；同一 index 的后续增量沿用它。
    let mut tool_names: BTreeMap<usize, String> = BTreeMap::new();
    let mut usage: Option<Usage> = None;
    let mut finish: Option<FinishReason> = None;
    // 同一 step 的重试链 id（首个 llm/retry 生成，之后复用；DSH 投影在 step/start、turn/end 后重置）。
    let mut retry_id: Option<RetryId> = None;
    // 每次模型尝试的精确帧记录（成功写进 assistant/message，失败写进 assistant/attempt；
    // DSH 的 AssistantStreamAttempt，agent-loop/src/assistant-stream.ts:18-43）。
    let mut attempt_stream = AssistantStreamAccumulator::new();
    let mut attempt: u32 = 0;
    let mut overflow_retries = 0_u32;
    'attempts: loop {
        if abort.load(Ordering::SeqCst) {
            return Ok(StepEnd::Aborted);
        }
        inner
            .capabilities
            .dispatch(&HookEvent::Request {
                turn,
                step,
                host: inner.as_ref(),
                messages: &messages,
            })
            .await?;
        inner.model.set_issue_context(CredentialContext {
            run: Some(inner.id().as_str().to_owned()),
            turn: Some(turn),
        });
        let mut stream = match inner.model.stream(&messages, &tool_request, &cancel).await {
            Ok(stream) => stream,
            Err(error) => {
                if abort.load(Ordering::SeqCst) || error.code == ErrorCode::Aborted {
                    // 请求还没开始流出（DSH agent.ts:488-500：started=false 不落 attempt）。
                    return Ok(StepEnd::Aborted);
                }
                // T005.8a：上下文溢出先压缩再重试一次；压缩不了或第二次溢出按正常错误收尾。
                // T006.1b：压缩由 RequestError 挂点里的压缩能力执行；它压缩不了时返回
                // 原始错误，这里 `?` 直接收尾（行为与旧路径一致）。
                if error.code == ErrorCode::ContextWindowExceeded {
                    // DSH maxOverflowRetries（默认 1）：每次溢出前压缩恢复；次数用尽后
                    // 按原错误收尾，不再派发/压缩（防无限循环）。
                    let max_overflow_retries = inner.compaction_policy().max_overflow_retries;
                    if overflow_retries < max_overflow_retries {
                        overflow_retries += 1;
                        inner
                            .capabilities
                            .dispatch(&HookEvent::RequestError {
                                turn,
                                step,
                                host: inner.as_ref(),
                                error: &error,
                            })
                            .await?;
                        attempt_stream = AssistantStreamAccumulator::new();
                        messages = inner
                            .store
                            .lock()
                            .expect("agent 会话锁被毒化")
                            .derive_messages();
                        continue 'attempts;
                    }
                    return Err(AgentError::Model(error));
                }
                // 请求还没流出（DSH started=false），这次不算 attempt：不落 assistant/attempt。
                inner
                    .capabilities
                    .dispatch(&HookEvent::RequestError {
                        turn,
                        step,
                        host: inner.as_ref(),
                        error: &error,
                    })
                    .await?;
                match decide_retry(
                    inner,
                    &cancel,
                    &error,
                    turn,
                    step,
                    &mut attempt,
                    &mut retry_id,
                )
                .await?
                {
                    RetryDecision::Retry => {
                        reset_attempt(
                            &mut blocks,
                            &mut open,
                            &mut usage,
                            &mut finish,
                            &mut attempt_stream,
                        );
                        tool_names.clear();
                        continue 'attempts;
                    }
                    RetryDecision::Cancel => return Ok(StepEnd::Aborted),
                    RetryDecision::Fatal => return Err(AgentError::Model(error)),
                }
            }
        };
        loop {
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(
                    inner,
                    turn,
                    step,
                    &blocks,
                    &open,
                    &usage,
                    &attempt_stream,
                )
                .await;
            }
            let notified = inner.cancel_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(
                    inner,
                    turn,
                    step,
                    &blocks,
                    &open,
                    &usage,
                    &attempt_stream,
                )
                .await;
            }
            let chunk = tokio::select! {
                chunk = stream.next() => chunk,
                () = &mut notified => {
                    return interrupted_step(
                    inner,
                    turn,
                    step,
                    &blocks,
                    &open,
                    &usage,
                    &attempt_stream,
                )
                .await;
                }
            };
            let Some(chunk) = chunk else {
                break;
            };
            if let Ok(chunk) = &chunk {
                attempt_stream.push(TimedStreamChunk {
                    time: now_ms(),
                    chunk: chunk.clone(),
                });
            }
            match chunk {
                Ok(StreamChunk::BlockStart { index, block_type }) => {
                    if matches!(block_type, BlockType::Text | BlockType::Reasoning) {
                        open.insert(index, (block_type, String::new()));
                    }
                }
                Ok(StreamChunk::TextDelta { index, text }) => {
                    if let Some((_, buffer)) = open.get_mut(&index) {
                        buffer.push_str(&text);
                    }
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "流式增量只作展示；订阅端未连接时丢弃"
                    )]
                    let _ = inner.events.send(AgentEvent::AssistantTextDelta { text });
                }
                Ok(StreamChunk::ReasoningDelta { index, text }) => {
                    if let Some((_, buffer)) = open.get_mut(&index) {
                        buffer.push_str(&text);
                    }
                }
                // 工具参数增量：照 DSH tool-call-delta 往订阅者发（T015.5，页面画「正在画 · 标题」）；
                // 同时照 T006.14 留在 stream 记录里。帧本身不落日志。
                Ok(StreamChunk::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                }) => {
                    if let Some(name) = name {
                        tool_names.insert(index, name);
                    }
                    let name = tool_names.get(&index).cloned().unwrap_or_default();
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "流式增量只作展示；订阅端未连接时丢弃"
                    )]
                    let _ = inner.events.send(AgentEvent::ToolCallDelta {
                        call_id: id,
                        name,
                        arguments_delta,
                    });
                }
                Ok(StreamChunk::BlockEnd { index, block }) => {
                    open.remove(&index);
                    blocks.insert(index, block);
                }
                // K10：适配器报的整段用量为空时当作没报（不伪造空 usage）。
                Ok(StreamChunk::Usage { usage: reported }) => {
                    if !reported.is_empty() {
                        usage = Some(reported);
                    }
                }
                Ok(StreamChunk::Finish { reason }) => finish = Some(reason),
                Err(error) => {
                    if abort.load(Ordering::SeqCst) || error.code == ErrorCode::Aborted {
                        return interrupted_step(
                            inner,
                            turn,
                            step,
                            &blocks,
                            &open,
                            &usage,
                            &attempt_stream,
                        )
                        .await;
                    }
                    // 流已经开过：这次尝试没有可见消息，照 DSH agent.ts:567-580 先落
                    // assistant/attempt（帧保留），再决定重试/收尾。
                    append_attempt(inner, turn, step, &attempt_stream).await?;
                    match decide_retry(
                        inner,
                        &cancel,
                        &error,
                        turn,
                        step,
                        &mut attempt,
                        &mut retry_id,
                    )
                    .await?
                    {
                        RetryDecision::Retry => {
                            reset_attempt(
                                &mut blocks,
                                &mut open,
                                &mut usage,
                                &mut finish,
                                &mut attempt_stream,
                            );
                            continue 'attempts;
                        }
                        RetryDecision::Cancel => return Ok(StepEnd::Aborted),
                        RetryDecision::Fatal => return Err(AgentError::Model(error)),
                    }
                }
            }
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(
                    inner,
                    turn,
                    step,
                    &blocks,
                    &open,
                    &usage,
                    &attempt_stream,
                )
                .await;
            }
        }
        break 'attempts;
    }
    // 流已收完但取消在结算前到了：照 DSH 的 for-await 后 throwIfAborted 走中断前缀。
    if abort.load(Ordering::SeqCst) {
        return interrupted_step(inner, turn, step, &blocks, &open, &usage, &attempt_stream).await;
    }
    let Some(finish) = finish else {
        return Err(AgentError::Model(ModelError::new(
            model::ErrorCode::StreamClosed,
            "模型流没有给出结束原因",
        )));
    };
    // max-tokens 时裁掉被截断的 tool-call 块（照 DSH assembler）：日志里不能留没有
    // 配对 tool/result 的 tool_use；text/reasoning 保留。
    let content: Vec<ContentBlock> = blocks
        .values()
        .filter(|block| {
            !(finish == FinishReason::MaxTokens && matches!(block, ContentBlock::ToolCall { .. }))
        })
        .cloned()
        .collect();
    // 成功结算一律写 assistant/message（DSH agent.ts:566-584）：content 裁空也要写，
    // 让 usage 落日志（T004 K9）；派生历史会跳过空消息（session/surface.ts:140）。
    let message = assistant_message(inner, content.clone());
    let (effort_wanted, effort) = inner.model.effort_state();
    inner
        .append(&[EventKind::AssistantMessage {
            effort: Some(effort),
            effort_wanted: Some(effort_wanted),
            turn,
            step,
            message,
            stream: attempt_stream.snapshot(),
            usage: usage.clone(),
            interrupted: None,
        }])
        .await?;
    emit_usage(inner, &usage);
    if finish == FinishReason::MaxTokens {
        // 被截断的输出里可能有不完整的工具调用：这一轮直接以 max-tokens 收尾，不执行工具。
        return Ok(StepEnd::MaxTokens);
    }
    let calls: Vec<ToolCall> = content
        .iter()
        .filter_map(ContentBlock::as_tool_call)
        .map(|(id, name, arguments)| ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        })
        .collect();
    if calls.is_empty() {
        return Ok(StepEnd::Completed);
    }
    // 先把 tool/call 作为持久事实写进日志，再派发（T004.4 的修复要靠它）。
    let mut call_events = Vec::with_capacity(calls.len());
    for call in &calls {
        call_events.push(EventKind::ToolCall {
            turn,
            step,
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
    }
    inner.append(&call_events).await?;
    // T006.1d2：ToolBefore 挂点可返回 ToolResultOverride（ask 拦截）或 AbortStep（取消）。
    // override 的 index 对应调用跳过实际执行，结果按 calls 顺序并回。
    let dispatch = inner
        .capabilities
        .dispatch(&HookEvent::ToolBefore {
            turn,
            step,
            host: inner.as_ref(),
            calls: &calls,
        })
        .await?;
    if dispatch.abort {
        return Ok(StepEnd::Aborted);
    }
    let overrides = dispatch.overrides;
    let mut exec_calls: Vec<ToolCall> = Vec::new();
    let mut exec_indices: Vec<usize> = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        if !overrides.iter().any(|(existing, _)| *existing == index) {
            exec_calls.push(call.clone());
            exec_indices.push(index);
        }
    }
    // 本步的工具表（条件）、会话日志的现读面都交给工具：查询类工具要算这条对话的用量。
    let events_reader: tools::SessionEventsReader = {
        let inner = Arc::clone(inner);
        Arc::new(move || {
            inner
                .store
                .lock()
                .expect("agent 会话锁被毒化")
                .events()
                .to_vec()
        })
    };
    let context = ToolContext::with_session(inner.cwd.clone(), Arc::clone(&inner.tool_session))
        .gated(Arc::clone(&enabled))
        .observing_session(events_reader);
    let mut outcome = execute_batch(
        &inner.tools,
        &exec_calls,
        inner.max_parallel_tool_calls,
        &context,
        abort.as_ref(),
    )
    .await;
    let mut merged: Vec<Option<tools::ToolResult>> = (0..calls.len()).map(|_| None).collect();
    for (position, result) in outcome.results.into_iter().enumerate() {
        if let Some(original) = exec_indices.get(position) {
            merged[*original] = Some(result);
        }
    }
    for (index, result) in overrides {
        if let Some(slot) = merged.get_mut(index) {
            *slot = Some(result);
        }
    }
    outcome.results = merged
        .into_iter()
        .map(|result| result.expect("每个调用都有结果（执行或 override）"))
        .collect();
    // T006.9c（照 DSH registry.start 的同步 append）：工具执行期间登记的后台任务先把
    // job/registered 落日志，再提交本批 tool/result；job/settled 仍由异步通道结算时写
    // （DSH jobs docs 允许晚于 turn/end）。
    let registered = inner.tool_session().tasks().take_registered_pending();
    if !registered.is_empty() {
        inner
            .append(&crate::jobs_capability::registered_events(&registered))
            .await?;
    }
    for result in &outcome.results {
        let Some(call) = calls.iter().find(|call| call.id == result.call_id) else {
            continue;
        };
        // T006.1d：落盘/内容变换由 ToolResult 挂点（spill 能力）决定；Continue 时用原文。
        let dispatch = inner
            .capabilities
            .dispatch(&HookEvent::ToolResult {
                turn,
                step,
                host: inner.as_ref(),
                call,
                result,
            })
            .await?;
        let content = dispatch.content.unwrap_or_else(|| result.content.clone());
        let message = Message::tool_result(result.call_id.clone(), content, result.is_error);
        inner
            .append(&[EventKind::ToolResult {
                turn,
                step,
                message,
                // 工具失败的身份照 DSH：name + code；普通参数错误没有结构化码，不写 error。
                error: result.error_code.as_ref().map(|code| ToolError {
                    name: "HarnessError".to_owned(),
                    code: code.clone(),
                }),
                // T013.1：工具结果的展示元信息原样落盘（不进模型消息）。
                meta: result.meta.clone(),
            }])
            .await?;
    }
    inner
        .capabilities
        .dispatch(&HookEvent::ToolAfter {
            turn,
            step,
            host: inner.as_ref(),
            calls: &calls,
            results: &outcome.results,
            cancelled: outcome.cancelled,
        })
        .await?;

    if abort.load(Ordering::SeqCst) || outcome.cancelled {
        return Ok(StepEnd::Aborted);
    }
    Ok(StepEnd::Continue)
}
