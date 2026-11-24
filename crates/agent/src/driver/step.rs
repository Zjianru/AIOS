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

/// 请求结束（成功/失败/重试/取消）时清掉内存里的在飞 requestAt。
struct InFlightRequest(Arc<Inner>);

impl Drop for InFlightRequest {
    fn drop(&mut self) {
        self.0.clear_in_flight_request_at();
    }
}

/// 一次 step 的请求上下文：派生消息、工具请求、本步开出的工具名、取消令牌。
pub(super) struct StepContext {
    messages: Vec<Message>,
    tool_request: model::ToolRequest,
    context_parts: session::ContextParts,
    /// 本步按条件开出的工具名；没开出的既不在表里，也不能执行。
    enabled: Arc<std::collections::BTreeSet<String>>,
    cancel: CancelToken,
}

/// Kind 的读/搜路径参数：只收绝对路径或以 ~/ 开头的路径；glob/grep 的 path 必填。
fn absolute_tool_schemas(schemas: &mut [model::ToolSchema]) {
    for schema in schemas {
        match schema.name.as_str() {
            "read" => {
                if let Some(properties) = schema.parameters.get_mut("properties") {
                    properties["file_path"]["description"] =
                        serde_json::json!("Absolute path or one starting with ~/. Required.");
                }
            }
            "glob" | "grep" => {
                if let Some(properties) = schema.parameters.get_mut("properties") {
                    properties["path"]["description"] = serde_json::json!(
                        "Absolute directory to search, or one starting with ~/. Required."
                    );
                }
                if let Some(required) = schema
                    .parameters
                    .get_mut("required")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    if !required.iter().any(|value| value == "path") {
                        required.push(serde_json::json!("path"));
                    }
                }
            }
            _ => {}
        }
    }
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
        let message = Message::system_prompt(system_prompt(
            &inner.cwd,
            &inner.tools,
            inner.policy_prompt.as_deref(),
            inner.identity_prompt.as_deref(),
            inner.workspace_context,
            inner.tool_session.absolute_paths_only(),
        ));
        inner
            .append(&[EventKind::SystemMessage {
                turn,
                step,
                message,
            }])
            .await?;
    }
    let claimed_messages = claimed.clone();
    // 第一轮顺序：system → 固定块 → 用户话 → 触发记忆 → 程序消息。
    // 固定块在 claim 到的用户话之前注入；触发记忆/显式 skill/确认卡通知仍走 Inject，
    // 那时用户话已经落进日志。prune 只挂在 Compaction 挂点，不参与这里。
    inner
        .capabilities
        .dispatch(&HookEvent::PreStep {
            turn,
            step,
            host: inner.as_ref(),
            messages: &claimed_messages,
            trigger: None,
            phase: crate::capabilities::PreStepPhase::FixedBlocks,
        })
        .await?;
    for message in claimed {
        inner
            .append(&[EventKind::UserMessage {
                turn,
                step,
                message,
            }])
            .await?;
    }

    // 用户话之后的触发材料，再派发 Compact（压缩自动检查）。
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
    // 请求前检查 token 压力；越过阈值就压缩较早历史（失败不打断本轮，事件已记录）。
    // 压缩实现已移出 loop，这里只派发固定挂点；压缩能力在 should_trigger/
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
    // 工具的条件每步算一次；本步新开出的工具记进日志（只在新增时记一条，带完整定义）。
    // 条件里的工具凭证也每步查一次：设置页换了 key，下一步就跟着变。
    let loaded_skills = {
        let store = inner.store.lock().expect("agent 会话锁被毒化");
        crate::tool_gate::loaded_skills(store.events(), None)
    };
    let tool_keys = inner.tool_keys.present();
    // T046：路由没声明 addition-only（MiniMax 等）时，工具表从第一轮起就是该会话角色 ToolSet 的
    // 全量成员——skill 加载只加载内容、不再改变工具表，前缀（含工具表）全程稳定。技能条件不再筛选
    // 工具，但角色/安全边界在构造期 retain_named 已收窄，这里不会跨边界；执行门禁用同一份全量表，
    // 模型看到就能调，不出现「看到却 TOOL_NOT_FOUND」。
    // 声明了 addition-only 的路由保持原样（条件 gate + tool/registry + tool_addition 投影）。
    let supports_tool_updates = inner.model.config().tool_update.is_some();
    let (enabled, additions) = if supports_tool_updates {
        let enabled = {
            let gate = inner.tool_gate.lock().expect("加载记录锁被毒化");
            gate.enabled(&inner.tools, &loaded_skills, &tool_keys)
        };
        let additions = {
            let gate = inner.tool_gate.lock().expect("加载记录锁被毒化");
            gate.additions(&inner.tools, &enabled)
        };
        (enabled, additions)
    } else {
        (
            crate::tool_gate::full_tool_names(&inner.tools, &tool_keys),
            Vec::new(),
        )
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
    let mut tool_schemas = inner.tools.definitions_for(&enabled);
    if inner.tool_session.absolute_paths_only() {
        absolute_tool_schemas(&mut tool_schemas);
    }
    let enabled = Arc::new(enabled);
    let (messages, mut tool_history) = {
        let store = inner.store.lock().expect("agent 会话锁被毒化");
        let derived = store.derive_messages_with_seqs();
        let history = crate::tool_gate::tool_history(store.events(), &derived);
        (
            crate::tool_pairing::sanitize(
                derived
                    .into_iter()
                    .map(|(_, message)| message)
                    .collect::<Vec<_>>(),
            ),
            history,
        )
    };
    if inner.tool_session.absolute_paths_only()
        && let Some(history) = tool_history.as_mut()
    {
        // addition-only 路由的声明从历史里取；历史也要带同一份绝对路径说明。
        absolute_tool_schemas(&mut history.baseline);
        for addition in &mut history.updates {
            absolute_tool_schemas(&mut addition.additions);
        }
    }
    let tool_request = model::ToolRequest {
        tools: tool_schemas,
        history: tool_history,
    };
    let context_parts = estimate_context_parts(&messages, &tool_request);
    // 取消令牌共用 agent 的 abort 标志 + Notify：等响应头阶段也能被打断。
    let cancel = CancelToken::new(Arc::clone(abort), Arc::clone(&inner.cancel_notify));
    Ok(Some(StepContext {
        messages,
        tool_request,
        context_parts,
        enabled,
        cancel,
    }))
}

fn is_task_book_rpc(id: &str) -> bool {
    [
        "assignment:",
        "continue:",
        "resume:",
        "status:",
        "task-update:",
        "task-requirement:",
        "task-cc:",
    ]
    .iter()
    .any(|prefix| id.starts_with(prefix))
}

fn estimate_context_parts(
    messages: &[Message],
    tool_request: &model::ToolRequest,
) -> session::ContextParts {
    let mut parts = session::ContextParts::default();
    for message in messages {
        let cost = crate::compaction::estimate_message(message);
        if message.role == model::Role::Assistant {
            parts.model_replies += cost;
        } else if message.role == model::Role::Tool {
            parts.tool_results += cost;
        } else if matches!(
            &message.source,
            model::MessageSource::User { rpc_id: Some(id) } if is_task_book_rpc(id)
        ) {
            parts.task_book_messages += cost;
        } else {
            parts.fixed_block += cost;
        }
    }
    parts.tool_definitions = serde_json::to_string(&tool_request.tools)
        .map(|text| tools::estimate_text_tokens(&text) as u64)
        .unwrap_or(0);
    parts
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
        context_parts,
        enabled,
        cancel,
    } = context;
    // 已完成的块按 index 存；打开中的 text/reasoning 块累计增量，取消时按 DSH
    // interruptedBlocks 的规则拼出可安全收尾的前缀（text/reasoning、去空白非空、
    // tool-call 丢弃）。
    let mut blocks: BTreeMap<usize, ContentBlock> = BTreeMap::new();
    let mut open: BTreeMap<usize, (BlockType, String)> = BTreeMap::new();
    // 模型流里的 name 只在工具调用的起始帧给；同一 index 的后续增量沿用它。
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
    let mut facts = RequestFacts {
        request_at: 0,
        context_parts: context_parts.clone(),
    };
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
            })
            .await?;
        inner.model.set_issue_context(CredentialContext {
            run: Some(inner.id().as_str().to_owned()),
            turn: Some(turn),
        });
        facts.request_at = now_ms();
        inner.set_in_flight_request_at(facts.request_at);
        let _in_flight = InFlightRequest(Arc::clone(inner));
        let mut stream = match inner.model.stream(&messages, &tool_request, &cancel).await {
            Ok(stream) => stream,
            Err(error) => {
                if abort.load(Ordering::SeqCst) || error.code == ErrorCode::Aborted {
                    // 请求还没开始流出（DSH agent.ts:488-500：started=false 不落 attempt）。
                    return Ok(StepEnd::Aborted);
                }
                // 上下文溢出先压缩再重试一次；压缩不了或第二次溢出按正常错误收尾。
                // 压缩由 RequestError 挂点里的压缩能力执行；它压缩不了时返回
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
                    &facts,
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
                    &facts,
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
                    &facts,
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
                // 工具参数增量：照 DSH tool-call-delta 往订阅者发（页面画「正在画 · 标题」）；
                // 同时照 留在 stream 记录里。帧本身不落日志。
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
                            &facts,
                        )
                        .await;
                    }
                    // 流已经开过：这次尝试没有可见消息，照 DSH agent.ts:567-580 先落
                    // assistant/attempt（帧保留），再决定重试/收尾。
                    append_attempt(inner, turn, step, &attempt_stream, &facts).await?;
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
                    &facts,
                )
                .await;
            }
        }
        break 'attempts;
    }
    // 流已收完但取消在结算前到了：照 DSH 的 for-await 后 throwIfAborted 走中断前缀。
    if abort.load(Ordering::SeqCst) {
        return interrupted_step(
            inner,
            turn,
            step,
            &blocks,
            &open,
            &usage,
            &attempt_stream,
            &facts,
        )
        .await;
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
    // 让 usage 落日志（K9）；派生历史会跳过空消息（session/surface.ts:140）。
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
            request_at: Some(facts.request_at),
            context_parts: Some(facts.context_parts.clone()),
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
    // 先把 tool/call 作为持久事实写进日志，再派发（修复要靠它）。
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
    // ToolBefore 挂点可返回 ToolResultOverride（ask 拦截）或 AbortStep（取消）。
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
    let mut overrides = dispatch.overrides;
    // no_wait 在统一入口判：tool/call 已经落盘，命中的调用不当成工具执行，直接给一条
    // 错误 tool/result（照 DSH 的 tools.guard + NO_WAIT）。超时、后台和短等待照常执行。
    if inner.no_wait {
        for (index, call) in calls.iter().enumerate() {
            if overrides.iter().any(|(existing, _)| *existing == index) {
                continue;
            }
            if let Some(reason) = crate::tool_policy::no_wait_reason(&call.name, &call.arguments) {
                overrides.push((
                    index,
                    tools::ToolResult::error(call.id.clone(), tools::ToolFailure::plain(reason)),
                ));
            }
        }
    }
    // 收尾停标记也在统一入口判：tool/call 已经落盘，除 task_report 以外的调用不执行，
    // 直接给一条固定拒绝语的错误 tool/result（照 D81：插入后除了上报别的都不能做）。
    if inner.stop_after_report_requested() {
        for (index, call) in calls.iter().enumerate() {
            if call.name == "task_report"
                || overrides.iter().any(|(existing, _)| *existing == index)
            {
                continue;
            }
            overrides.push((
                index,
                tools::ToolResult::error(
                    call.id.clone(),
                    tools::ToolFailure::plain(crate::tool_policy::STOP_AFTER_REPORT),
                ),
            ));
        }
    }
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
        .observing_session(events_reader)
        .with_write_scope(inner.write_scope.clone())
        .with_turn(turn);
    let mut outcome = execute_batch(
        &inner.tools,
        &exec_calls,
        inner.max_parallel_tool_calls,
        &context,
        &cancel,
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
    // （照 DSH registry.start 的同步 append）：工具执行期间登记的后台任务先把
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
        // 落盘/内容变换由 ToolResult 挂点（spill 能力）决定；Continue 时用原文。
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
                // 工具结果的展示元信息原样落盘（不进模型消息）。
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
    // T078.1 StopAtTools：这一步全执行完，里面有成功的 kind_say 就直接结束本轮，
    // 不再调模型；kind_say 失败（如只有一条）照常 Continue 让模型改。
    if calls
        .iter()
        .zip(outcome.results.iter())
        .any(|(call, result)| {
            call.name == "kind_say" && !result.is_error && result.error_code.is_none()
        })
    {
        return Ok(StepEnd::Completed);
    }
    Ok(StepEnd::Continue)
}

#[cfg(test)]
mod context_parts_tests {
    use super::*;
    use model::{ContentBlock, Message, MessageSource, Role, ToolSchema};
    use serde_json::json;

    fn message(role: Role, source: MessageSource, text: &str) -> Message {
        Message {
            id: model::MessageId::generate(),
            role,
            content: vec![ContentBlock::text(text)],
            source,
            tool_call_id: None,
            is_error: None,
        }
    }

    /// 已知 usage：按请求里的文本/工具定义字符数独立折算（不是复用被测估算器）。
    fn known_usage(messages: &[Message], tools: &[ToolSchema]) -> model::Usage {
        let mut chars = 0_usize;
        for message in messages {
            for block in &message.content {
                if let Some(text) = block.as_text() {
                    chars += text.chars().count();
                }
            }
        }
        for schema in tools {
            chars += schema.name.chars().count();
            chars += schema.description.chars().count();
            chars += schema.parameters.to_string().chars().count();
        }
        model::Usage {
            input_tokens: Some(
                (chars.div_ceil(4) + messages.len() * 4 + tools.len() * 4 + 4) as u64,
            ),
            ..model::Usage::default()
        }
    }

    fn assert_close(parts: &session::ContextParts, usage: u64) {
        let sum = parts.fixed_block
            + parts.tool_definitions
            + parts.task_book_messages
            + parts.model_replies
            + parts.tool_results;
        let diff = sum.abs_diff(usage);
        assert!(
            diff as f64 / usage as f64 <= 0.30,
            "五块估算和 {sum} 与 usage {usage} 偏差超过 30%：{parts:?}"
        );
    }

    fn tool(name: &str, description: &str, parameters: serde_json::Value) -> ToolSchema {
        ToolSchema {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters,
            defer_loading: false,
        }
    }

    #[test]
    fn large_tool_definitions_stay_within_thirty_percent() {
        let mut tools = Vec::new();
        for index in 0..12 {
            tools.push(tool(
                &format!("tool-{index}"),
                &"Read and inspect a resource with enough description text to count. ".repeat(4),
                json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute path to inspect"},
                        "limit": {"type": "number", "description": "Maximum lines"}
                    },
                    "required": ["path"]
                }),
            ));
        }
        let messages = vec![
            message(
                Role::System,
                MessageSource::SystemPrompt,
                &"You are AIOS. ".repeat(200),
            ),
            Message::user_text_with_request_id(
                "把安装步骤补上，并解释为什么这么改。".repeat(20),
                "assignment:task-1",
            ),
            Message::assistant(
                vec![ContentBlock::text("先看 README 和现有命令。".repeat(20))],
                "deepseek",
                "deepseek-flash",
            ),
            Message::tool_result(
                "call-1",
                "README 里没有安装步骤，package.json 有 scripts。".repeat(20),
                false,
            ),
        ];
        let usage = known_usage(&messages, &tools).input_tokens.unwrap_or(0);
        let request = model::ToolRequest {
            tools,
            history: None,
        };
        assert_close(&estimate_context_parts(&messages, &request), usage);
    }

    #[test]
    fn compaction_checkpoint_stays_within_thirty_percent() {
        let tools = vec![
            tool("read", "Read a file", json!({"type": "object"})),
            tool("write", "Write a file", json!({"type": "object"})),
            tool("bash", "Run a command", json!({"type": "object"})),
        ];
        let mut checkpoint = message(
            Role::User,
            MessageSource::default(),
            "## 在办的事\n- 修安装步骤\n",
        );
        checkpoint.source = MessageSource::CompactCheckpoint {
            compaction_id: "c-1".to_owned(),
            source_command_id: None,
        };
        let messages = vec![
            message(
                Role::System,
                MessageSource::SystemPrompt,
                &"You are AIOS. ".repeat(100),
            ),
            message(
                Role::User,
                MessageSource::AgentInstructions {
                    form: "instructions".to_owned(),
                    baseline: true,
                    baseline_identity: Some("baseline".to_owned()),
                    changes: Vec::new(),
                },
                &"项目规则：README 要有安装步骤。".repeat(10),
            ),
            message(
                Role::User,
                MessageSource::MemoryContext {
                    identity: "memory-v1".to_owned(),
                },
                &"记忆：用户喜欢短结论。".repeat(10),
            ),
            checkpoint,
            Message::assistant(
                vec![ContentBlock::text("继续改 README。".repeat(10))],
                "deepseek",
                "deepseek-flash",
            ),
            Message::user_text("现在把安装步骤补上。".repeat(10)),
        ];
        let usage = known_usage(&messages, &tools).input_tokens.unwrap_or(0);
        let request = model::ToolRequest {
            tools,
            history: None,
        };
        assert_close(&estimate_context_parts(&messages, &request), usage);
    }
}
