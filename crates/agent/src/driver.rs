//! agent 的驱动循环与共享状态。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use model::{
    BlockType, CancelToken, ContentBlock, DeepSeek, ErrorCode, FinishReason, Message, ModelError,
    Role, StreamChunk, Usage, UserMessage,
};
use session::{EventKind, SessionId, SessionStore, TurnEndReason};
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tools::{ToolCall, ToolContext, ToolSessionState, ToolSet, execute_batch};

use crate::error::AgentError;
use crate::inbox::{Delivery, Inbox, InboxTarget};
use crate::prune;
use crate::{AgentConfig, AgentEvent, Deps, TurnOutcome};

const EVENT_CHANNEL_CAPACITY: usize = 256;

/// 驱动阶段。
enum Phase {
    Idle,
    Running { abort: Arc<AtomicBool>, turn: u64 },
}

struct RunState {
    phase: Phase,
    wake_requested: bool,
    last_turn: u64,
}

/// 一个 step 的结束方式。
enum StepEnd {
    /// 这一步执行了工具，turn 还没结束；继续开下一步。
    Continue,
    Completed,
    MaxTokens,
    Aborted,
}

/// agent 的共享状态；驱动任务和 6 个入口方法都通过它交互。
pub(crate) struct Inner {
    id: SessionId,
    log_path: PathBuf,
    cwd: PathBuf,
    model: DeepSeek,
    tools: ToolSet,
    /// 会话级工具状态（观察表/spill/任务注册表）；续接时新建，不继承上一次。
    tool_session: Arc<ToolSessionState>,
    max_parallel_tool_calls: usize,
    /// 单个工具结果内联进上下文的 token 预算；0 表示禁用 spill（AgentConfig 同名）。
    spill_max_inline_tokens: usize,
    store: Mutex<SessionStore>,
    inbox: Mutex<Inbox>,
    state: Mutex<RunState>,
    events: broadcast::Sender<AgentEvent>,
    idle_notify: Notify,
    /// 取消时唤醒正在等模型流的 step（让流读取立刻停下）。
    cancel_notify: Arc<Notify>,
}

impl Inner {
    pub(crate) fn start(
        config: AgentConfig,
        deps: Deps,
        last_turn: u64,
    ) -> Result<Arc<Self>, AgentError> {
        // 协调线加账：非法配置要在建 agent 时返回错误，不能等到驱动任务里 panic。
        config.validate()?;
        let Deps {
            model,
            tools,
            store,
        } = deps;
        let id = store.id().clone();
        let log_path = store.path().to_path_buf();
        let cwd = PathBuf::from(store.cwd());
        // 日志路径固定是 <data-dir>/sessions/<id>.jsonl；spill 根按会话分目录。
        let data_dir = log_path
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| cwd.clone(), Path::to_path_buf);
        let tool_session = Arc::new(ToolSessionState::new(&data_dir, id.as_str()));
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Ok(Arc::new(Self {
            id,
            log_path,
            cwd,
            model,
            tools,
            tool_session,
            max_parallel_tool_calls: config.max_parallel_tool_calls,
            spill_max_inline_tokens: config.spill_max_inline_tokens,
            store: Mutex::new(store),
            inbox: Mutex::new(Inbox::default()),
            state: Mutex::new(RunState {
                phase: Phase::Idle,
                wake_requested: false,
                last_turn,
            }),
            events,
            idle_notify: Notify::new(),
            cancel_notify: Arc::new(Notify::new()),
        }))
    }

    pub(crate) fn id(&self) -> &SessionId {
        &self.id
    }

    pub(crate) fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    pub(crate) fn send(self: &Arc<Self>, message: UserMessage, delivery: Delivery) {
        let target = {
            let state = self.state.lock().expect("agent 状态锁被毒化");
            let aborting = match &state.phase {
                Phase::Running { abort, .. } => abort.load(Ordering::SeqCst),
                Phase::Idle => false,
            };
            // 已经取消的活动收不下会唤醒的输入：改投下一轮（照 DSH 的 send）。
            if aborting && delivery.wakes() {
                InboxTarget::NextTurn
            } else {
                delivery.target()
            }
        };
        self.inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .push(target, message);
        if delivery.wakes() {
            self.wake_driver();
        }
    }

    pub(crate) fn cancel(&self) {
        self.inbox.lock().expect("agent 收件箱锁被毒化").clear();
        let mut state = self.state.lock().expect("agent 状态锁被毒化");
        state.wake_requested = false;
        if let Phase::Running { abort, .. } = &state.phase {
            abort.store(true, Ordering::SeqCst);
            // 让正在等模型流的 step 立刻醒来、走 interrupted 收尾。
            self.cancel_notify.notify_waiters();
        }
    }

    /// 起驱动或在 Running 时置 latch。
    ///
    /// T004.1 审计 known-weakness 3：收尾检查与 settle 快照之间来的 FollowUp/Steer
    /// 过去会被丢掉。Running 时无条件 latch，settle 看到 wake && pending 会重开驱动。
    fn wake_driver(self: &Arc<Self>) {
        let mut state = self.state.lock().expect("agent 状态锁被毒化");
        match &state.phase {
            Phase::Idle => {
                let turn = state.last_turn;
                state.phase = Phase::Running {
                    abort: Arc::new(AtomicBool::new(false)),
                    turn,
                };
                drop(state);
                tokio::spawn(kick(Arc::clone(self)));
            }
            Phase::Running { .. } => {
                state.wake_requested = true;
            }
        }
    }

    pub(crate) async fn idle(&self) {
        loop {
            let notified = self.idle_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if matches!(
                &self.state.lock().expect("agent 状态锁被毒化").phase,
                Phase::Idle
            ) {
                return;
            }
            notified.await;
        }
    }

    fn append(&self, kinds: &[EventKind]) -> Result<(), AgentError> {
        self.store
            .lock()
            .expect("agent 会话锁被毒化")
            .append(kinds)?;
        Ok(())
    }
}

/// 驱动任务：连续跑若干轮，直到收件箱里没有会唤醒的输入。
async fn kick(inner: Arc<Inner>) {
    while let Ok(true) = run_turn(&inner).await {}
    settle_idle(&inner);
}

/// 收敛到 Idle；如果取消之后有人唤过，就立刻再起一个驱动。
fn settle_idle(inner: &Arc<Inner>) {
    let (turn, wake) = {
        let mut state = inner.state.lock().expect("agent 状态锁被毒化");
        let turn = match &state.phase {
            Phase::Running { turn, .. } => *turn,
            Phase::Idle => state.last_turn,
        };
        state.last_turn = turn;
        let wake = std::mem::take(&mut state.wake_requested);
        (turn, wake)
    };
    let has_pending = inner
        .inbox
        .lock()
        .expect("agent 收件箱锁被毒化")
        .has_pending();
    if wake && has_pending {
        let mut state = inner.state.lock().expect("agent 状态锁被毒化");
        state.phase = Phase::Running {
            abort: Arc::new(AtomicBool::new(false)),
            turn,
        };
        drop(state);
        tokio::spawn(kick(Arc::clone(inner)));
        return;
    }
    inner.state.lock().expect("agent 状态锁被毒化").phase = Phase::Idle;
    inner.idle_notify.notify_waiters();
}

async fn run_turn(inner: &Arc<Inner>) -> Result<bool, AgentError> {
    let (current_turn, abort) = {
        let state = inner.state.lock().expect("agent 状态锁被毒化");
        match &state.phase {
            Phase::Running { abort, turn } => (*turn, Arc::clone(abort)),
            Phase::Idle => return Ok(false),
        }
    };
    // step 是这一轮内的局部计数：每轮从 1 开始，不跨 turn 携带（T004 审 K1）。
    let mut step = 0u64;
    let turn = current_turn + 1;
    inner.append(&[EventKind::TurnStart { turn }])?;
    {
        let mut state = inner.state.lock().expect("agent 状态锁被毒化");
        if let Phase::Running { turn: current, .. } = &mut state.phase {
            *current = turn;
        }
    }

    let mut turn_end: Option<TurnEndReason> = None;
    let steps = run_turn_steps(inner, turn, &mut step, &abort, &mut turn_end).await;

    let (reason, outcome) = match &steps {
        Ok(()) => {
            let reason = turn_end.unwrap_or_else(|| TurnEndReason::Error {
                code: "UNKNOWN".to_owned(),
                message: "turn ended without a reason".to_owned(),
            });
            let outcome = TurnOutcome::from(&reason);
            (reason, outcome)
        }
        Err(error) => {
            let reason = TurnEndReason::Error {
                code: error.code(),
                message: error.to_string(),
            };
            let outcome = TurnOutcome::from(&reason);
            (reason, outcome)
        }
    };
    let appended = inner.append(&[EventKind::TurnEnd { turn, reason }]);
    let _ = inner.events.send(AgentEvent::TurnEnded { turn, outcome });
    appended?;
    let mut result = match steps {
        Ok(()) => Ok(inner
            .inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .has_pending()),
        Err(error) => Err(error),
    };
    // T004.3 审计 fix-now 1：取消之后、收尾之前投递的消息会被 latch；若把
    // has_pending 带回 kick，就会用同一个已 abort 的 Phase 空转。按「abort 置位」
    // （不看 reason：cancel 也可能落在正常完成之后）收敛到 settle_idle；settle 会
    // 用新 Phase（新 abort）重开驱动，领取取消后投递的输入。
    if abort.load(Ordering::SeqCst) {
        result = Ok(false);
    }
    // 收尾检查与 settle_idle 的 wake_requested 快照之间只有一个显式让出点
    // （tokio::task::yield_now），除此之外没有别的 await；让排队中的
    // FollowUp/Steer 能在 settle 之前落地（确定性回归靠这个点，不靠 sleep）。
    tokio::task::yield_now().await;
    result
}

/// 一轮里不断开 step：turn 开始时领取一条 follow-up，加上全部 next-step；
/// 之后的 step 只领取 next-step。执行了工具、轮次还没结束时继续开下一步。
async fn run_turn_steps(
    inner: &Arc<Inner>,
    turn: u64,
    step: &mut u64,
    abort: &Arc<AtomicBool>,
    turn_end: &mut Option<TurnEndReason>,
) -> Result<(), AgentError> {
    let mut target = InboxTarget::NextTurn;
    loop {
        if abort.load(Ordering::SeqCst) {
            *turn_end = Some(TurnEndReason::Aborted);
            return Ok(());
        }
        let claimed = inner
            .inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .claim(target);
        if turn_end.is_some() && claimed.is_empty() {
            return Ok(());
        }
        if *step == 0 && claimed.is_empty() {
            *turn_end = Some(TurnEndReason::Completed);
            return Ok(());
        }
        *step += 1;
        let step_number = *step;
        inner.append(&[EventKind::StepStart {
            turn,
            step: step_number,
        }])?;
        let outcome = run_step(inner, turn, step_number, claimed, abort).await;
        let end = match outcome {
            Ok(end) => end,
            Err(error) => {
                if let Err(cleanup_error) = inner.append(&[EventKind::StepEnd {
                    turn,
                    step: step_number,
                }]) {
                    return Err(AgentError::StepCleanup {
                        first: error.to_string(),
                        second: cleanup_error.to_string(),
                    });
                }
                return Err(error);
            }
        };
        inner.append(&[EventKind::StepEnd {
            turn,
            step: step_number,
        }])?;
        match end {
            StepEnd::Continue => {}
            StepEnd::Completed => {
                if !matches!(turn_end.as_ref(), Some(TurnEndReason::MaxTokens)) {
                    *turn_end = Some(TurnEndReason::Completed);
                }
            }
            StepEnd::MaxTokens => {
                if !matches!(turn_end.as_ref(), Some(TurnEndReason::MaxTokens)) {
                    *turn_end = Some(TurnEndReason::MaxTokens);
                }
            }
            StepEnd::Aborted => {
                *turn_end = Some(TurnEndReason::Aborted);
            }
        }
        if turn_end.is_some()
            && inner
                .inbox
                .lock()
                .expect("agent 收件箱锁被毒化")
                .next_step_is_empty()
        {
            return Ok(());
        }
        target = InboxTarget::NextStep;
    }
}

/// 一个 step：写 system/user 事件，从日志派生请求，流式收模型输出，
/// 有工具调用就派发批执行并写 tool/call、tool/result，否则收尾这一轮。
async fn run_step(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    claimed: Vec<Message>,
    abort: &Arc<AtomicBool>,
) -> Result<StepEnd, AgentError> {
    if abort.load(Ordering::SeqCst) {
        return Ok(StepEnd::Aborted);
    }
    let has_system = inner
        .store
        .lock()
        .expect("agent 会话锁被毒化")
        .has_system_message();
    if !has_system {
        let message = Message::text(Role::System, system_prompt(&inner.cwd, &inner.tools));
        inner.append(&[EventKind::SystemMessage {
            turn,
            step,
            message,
        }])?;
    }
    for message in claimed {
        inner.append(&[EventKind::UserMessage {
            turn,
            step,
            message,
        }])?;
    }

    // T005.2：派生请求之前，把「旧」超大工具结果换成 head+marker+tail 的修剪视图
    // （追加 compaction/prune 事件，原始 tool/result 保留在日志里可回放）。
    prune_old_tool_results(inner, turn)?;
    let messages = inner
        .store
        .lock()
        .expect("agent 会话锁被毒化")
        .derive_messages();
    let tool_schemas = inner.tools.definitions();
    // 取消令牌共用 agent 的 abort 标志 + Notify：等响应头阶段也能被打断。
    let cancel = CancelToken::new(Arc::clone(abort), Arc::clone(&inner.cancel_notify));
    // 已完成的块按 index 存；打开中的 text/reasoning 块累计增量，取消时按 DSH
    // interruptedBlocks 的规则拼出可安全收尾的前缀（text/reasoning、去空白非空、
    // tool-call 丢弃）。
    let mut blocks: BTreeMap<usize, ContentBlock> = BTreeMap::new();
    let mut open: BTreeMap<usize, (BlockType, String)> = BTreeMap::new();
    let mut usage: Option<Usage> = None;
    let mut finish: Option<FinishReason> = None;
    let mut content_emitted = false;
    let mut attempt: u32 = 0;
    'attempts: loop {
        if abort.load(Ordering::SeqCst) {
            return Ok(StepEnd::Aborted);
        }
        let mut stream = match inner.model.stream(&messages, &tool_schemas, &cancel).await {
            Ok(stream) => stream,
            Err(error) => {
                if abort.load(Ordering::SeqCst) || error.code == ErrorCode::Aborted {
                    return interrupted_step(inner, turn, step, &blocks, &open, &usage).await;
                }
                match decide_retry(inner, &cancel, &error, &mut attempt, content_emitted).await {
                    RetryDecision::Retry => {
                        reset_attempt(&mut blocks, &mut open, &mut usage, &mut finish);
                        content_emitted = false;
                        continue 'attempts;
                    }
                    RetryDecision::Cancel => {
                        return interrupted_step(inner, turn, step, &blocks, &open, &usage).await;
                    }
                    RetryDecision::Fatal => return Err(AgentError::Model(error)),
                }
            }
        };
        loop {
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(inner, turn, step, &blocks, &open, &usage).await;
            }
            let notified = inner.cancel_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(inner, turn, step, &blocks, &open, &usage).await;
            }
            let chunk = tokio::select! {
                chunk = stream.next() => chunk,
                () = &mut notified => {
                    return interrupted_step(inner, turn, step, &blocks, &open, &usage).await;
                }
            };
            let Some(chunk) = chunk else {
                break;
            };
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
                    content_emitted = true;
                    let _ = inner.events.send(AgentEvent::AssistantTextDelta { text });
                }
                Ok(StreamChunk::ReasoningDelta { index, text }) => {
                    if let Some((_, buffer)) = open.get_mut(&index) {
                        buffer.push_str(&text);
                    }
                    content_emitted = true;
                }
                Ok(StreamChunk::BlockEnd { index, block }) => {
                    open.remove(&index);
                    blocks.insert(index, block);
                }
                Ok(StreamChunk::Usage(reported)) => usage = Some(reported),
                Ok(StreamChunk::Finish(reason)) => finish = Some(reason),
                Err(error) => {
                    if abort.load(Ordering::SeqCst) || error.code == ErrorCode::Aborted {
                        return interrupted_step(inner, turn, step, &blocks, &open, &usage).await;
                    }
                    match decide_retry(inner, &cancel, &error, &mut attempt, content_emitted).await
                    {
                        RetryDecision::Retry => {
                            reset_attempt(&mut blocks, &mut open, &mut usage, &mut finish);
                            content_emitted = false;
                            continue 'attempts;
                        }
                        RetryDecision::Cancel => {
                            return interrupted_step(inner, turn, step, &blocks, &open, &usage)
                                .await;
                        }
                        RetryDecision::Fatal => return Err(AgentError::Model(error)),
                    }
                }
            }
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(inner, turn, step, &blocks, &open, &usage).await;
            }
        }
        break 'attempts;
    }
    let Some(finish) = finish else {
        return Err(AgentError::Model(ModelError::new(
            model::ErrorCode::StreamClosed,
            "模型流没有给出结束原因",
        )));
    };
    // max-tokens 时裁掉被截断的 tool-call 块（照 DSH assembler）：日志里不能留没有
    // 配对 tool/result 的 tool_use；text/reasoning 保留。裁完为空就不写 assistant 消息（D1）。
    let content: Vec<ContentBlock> = blocks
        .values()
        .filter(|block| {
            !(finish == FinishReason::MaxTokens && matches!(block, ContentBlock::ToolCall { .. }))
        })
        .cloned()
        .collect();
    if !content.is_empty() {
        let message = Message::new(Role::Assistant, content.clone());
        inner.append(&[EventKind::AssistantMessage {
            turn,
            step,
            message,
            usage: usage.clone(),
            interrupted: None,
        }])?;
        emit_usage(inner, &usage);
    }
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
    inner.append(&call_events)?;
    let context = ToolContext::with_session(inner.cwd.clone(), Arc::clone(&inner.tool_session));
    let outcome = execute_batch(
        &inner.tools,
        &calls,
        inner.max_parallel_tool_calls,
        &context,
        abort.as_ref(),
    )
    .await;
    for result in &outcome.results {
        // T005.2：超大结果落 spill（按会话目录），进日志/上下文的是 preview + locator；
        // read 有自己的分页上限，不在此列（照 DSH spill-policy）。
        let tool_name = calls
            .iter()
            .find(|call| call.id == result.call_id)
            .map_or("", |call| call.name.as_str());
        let content = tools::bound_result(
            inner.tool_session.spill(),
            tool_name,
            &result.call_id,
            &result.content,
            inner.spill_max_inline_tokens,
        )
        .unwrap_or_else(|| result.content.clone());
        let message = Message::tool_result(result.call_id.clone(), content, result.is_error);
        inner.append(&[EventKind::ToolResult {
            turn,
            step,
            message,
            error_code: result.error_code.clone(),
        }])?;
    }
    if abort.load(Ordering::SeqCst) || outcome.cancelled {
        return Ok(StepEnd::Aborted);
    }
    Ok(StepEnd::Continue)
}

/// T005.2 修剪：只修剪「旧 turn」的 tool/result（当前 turn 刚拿到的结果保留原样），
/// 已经修剪过的 call_id 不再重复追加；一批 append 落日志。
fn prune_old_tool_results(inner: &Arc<Inner>, current_turn: u64) -> Result<(), AgentError> {
    let mut additions: Vec<EventKind> = Vec::new();
    {
        let store = inner.store.lock().expect("agent 会话锁被毒化");
        let events = store.events();
        let already: std::collections::HashSet<&str> = events
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::CompactionPrune { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        for event in events {
            let EventKind::ToolResult { turn, message, .. } = &event.kind else {
                continue;
            };
            if *turn >= current_turn {
                continue;
            }
            let Some(ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            }) = message.content.first()
            else {
                continue;
            };
            if already.contains(tool_use_id.as_str()) {
                continue;
            }
            if let Some(pruned) = prune::prune_text(content) {
                additions.push(EventKind::CompactionPrune {
                    call_id: tool_use_id.clone(),
                    content: pruned,
                });
            }
        }
    }
    if !additions.is_empty() {
        inner.append(&additions)?;
    }
    Ok(())
}

/// 请求层重试：最多 5 次、500ms 起指数退避、上限 10s、±10% 抖动（std 派生，不加依赖）。
const MAX_RETRIES: u32 = 5;
const INITIAL_RETRY_DELAY_MS: u64 = 500;
const MAX_RETRY_DELAY_MS: u64 = 10_000;

enum RetryDecision {
    Retry,
    Cancel,
    Fatal,
}

/// 只认 spec 定的 5 个可重试码。
fn is_retryable(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::EmptyResponse
            | ErrorCode::RateLimit
            | ErrorCode::Server
            | ErrorCode::Timeout
            | ErrorCode::Transport
    )
}

/// 这次失败要不要重试；要的话等完（可取消）再回 Retry。
async fn decide_retry(
    inner: &Arc<Inner>,
    cancel: &CancelToken,
    error: &ModelError,
    attempt: &mut u32,
    content_emitted: bool,
) -> RetryDecision {
    // 已经流出内容 delta 的请求不重试：避免把重复文本再打一遍（协调线裁定；spec 没规定，偏离 DSH）。
    if content_emitted || !is_retryable(error.code) || *attempt >= MAX_RETRIES {
        return RetryDecision::Fatal;
    }
    let Some(delay_ms) = retry_delay_ms(error, *attempt + 1) else {
        return RetryDecision::Fatal;
    };
    *attempt += 1;
    let _ = inner.events.send(AgentEvent::Retrying {
        attempt: *attempt,
        max_attempts: MAX_RETRIES,
        delay_ms,
        code: error.code.as_str(),
    });
    let cancelled = tokio::select! {
        biased;
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(Duration::from_millis(delay_ms)) => false,
    };
    if cancelled {
        RetryDecision::Cancel
    } else {
        RetryDecision::Retry
    }
}

/// 服务端 retry-after 优先；超过上限就不重试（照 DSH normal 模式）；否则本地退避。
fn retry_delay_ms(error: &ModelError, retry: u32) -> Option<u64> {
    if let Some(after) = error.retry_after_ms {
        if after > MAX_RETRY_DELAY_MS {
            return None;
        }
        return Some(after.max(1));
    }
    let exponent = retry.saturating_sub(1).min(10);
    let exponential = INITIAL_RETRY_DELAY_MS
        .saturating_mul(1u64 << exponent)
        .min(MAX_RETRY_DELAY_MS);
    let factor = jitter_factor(retry);
    Some(((exponential as f64 * factor).round() as u64).clamp(1, MAX_RETRY_DELAY_MS))
}

/// std 派生伪随机抖动（SystemTime 纳秒 + 重试序号混合），不引 rand。
fn jitter_factor(retry: u32) -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos() as u64);
    let mixed = nanos ^ u64::from(retry).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let unit = (mixed >> 11) as f64 / (1u64 << 53) as f64;
    1.0 + 0.1 * (2.0 * unit - 1.0)
}

fn reset_attempt(
    blocks: &mut BTreeMap<usize, ContentBlock>,
    open: &mut BTreeMap<usize, (BlockType, String)>,
    usage: &mut Option<Usage>,
    finish: &mut Option<FinishReason>,
) {
    blocks.clear();
    open.clear();
    *usage = None;
    *finish = None;
}

fn emit_usage(inner: &Arc<Inner>, usage: &Option<Usage>) {
    if let Some(usage) = usage {
        let _ = inner.events.send(AgentEvent::UsageReported {
            usage: usage.clone(),
        });
    }
}

async fn interrupted_step(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    blocks: &BTreeMap<usize, ContentBlock>,
    open: &BTreeMap<usize, (BlockType, String)>,
    usage: &Option<Usage>,
) -> Result<StepEnd, AgentError> {
    let content = interrupted_content(blocks, open);
    if !content.is_empty() {
        let message = Message::new(Role::Assistant, content);
        inner.append(&[EventKind::AssistantMessage {
            turn,
            step,
            message,
            usage: usage.clone(),
            interrupted: Some(true),
        }])?;
        emit_usage(inner, usage);
    }
    Ok(StepEnd::Aborted)
}

/// 取消时保留的可安全收尾前缀（照 DSH assembler.interruptedBlocks）：
/// 已关闭/打开中的 text 和 reasoning 块、去空白非空、按流顺序；tool-call 丢弃。
/// interrupted 的 thinking 不写 signature（照 DSH，handback 记）。
fn interrupted_content(
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
fn system_prompt(cwd: &Path, tools: &ToolSet) -> String {
    let mut prompt = format!(
        "You are AIOS, a coding agent. Your working directory is {}.",
        cwd.display()
    );
    if tools.get("read").is_some() {
        prompt.push_str(
            "\n\nUse the read tool — not shell commands like cat — to inspect text files. Use offset and limit to continue reading large files.",
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
