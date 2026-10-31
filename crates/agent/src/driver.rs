//! agent 的驱动循环与共享状态。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use model::{
    ContentBlock, DeepSeek, FinishReason, Message, ModelError, Role, StreamChunk, Usage,
    UserMessage,
};
use session::{EventKind, SessionId, SessionStore, TurnEndReason};
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tools::{ToolCall, ToolContext, ToolSet, execute_batch};

use crate::error::AgentError;
use crate::inbox::{Delivery, Inbox, InboxTarget};
use crate::{AgentConfig, AgentEvent, Deps, TurnOutcome};

const EVENT_CHANNEL_CAPACITY: usize = 256;

/// 驱动阶段。
enum Phase {
    Idle,
    Running {
        abort: Arc<AtomicBool>,
        turn: u64,
        step: u64,
    },
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
    max_parallel_tool_calls: usize,
    store: Mutex<SessionStore>,
    inbox: Mutex<Inbox>,
    state: Mutex<RunState>,
    events: broadcast::Sender<AgentEvent>,
    idle_notify: Notify,
}

impl Inner {
    pub(crate) fn start(
        config: AgentConfig,
        deps: Deps,
        last_turn: u64,
    ) -> Result<Arc<Self>, AgentError> {
        let Deps {
            model,
            tools,
            store,
        } = deps;
        let id = store.id().clone();
        let log_path = store.path().to_path_buf();
        let cwd = PathBuf::from(store.cwd());
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Ok(Arc::new(Self {
            id,
            log_path,
            cwd,
            model,
            tools,
            max_parallel_tool_calls: config.max_parallel_tool_calls,
            store: Mutex::new(store),
            inbox: Mutex::new(Inbox::default()),
            state: Mutex::new(RunState {
                phase: Phase::Idle,
                wake_requested: false,
                last_turn,
            }),
            events,
            idle_notify: Notify::new(),
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
        let (aborting, target) = {
            let state = self.state.lock().expect("agent 状态锁被毒化");
            let aborting = match &state.phase {
                Phase::Running { abort, .. } => abort.load(Ordering::SeqCst),
                Phase::Idle => false,
            };
            // 已经取消的活动收不下会唤醒的输入：改投下一轮（照 DSH 的 send）。
            let target = if aborting && delivery.wakes() {
                InboxTarget::NextTurn
            } else {
                delivery.target()
            };
            (aborting, target)
        };
        self.inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .push(target, message);
        if delivery.wakes() {
            self.wake_driver(aborting);
        }
    }

    pub(crate) fn cancel(&self) {
        self.inbox.lock().expect("agent 收件箱锁被毒化").clear();
        let mut state = self.state.lock().expect("agent 状态锁被毒化");
        state.wake_requested = false;
        if let Phase::Running { abort, .. } = &state.phase {
            abort.store(true, Ordering::SeqCst);
        }
    }

    fn wake_driver(self: &Arc<Self>, wake_after_abort: bool) {
        let mut state = self.state.lock().expect("agent 状态锁被毒化");
        match &state.phase {
            Phase::Idle => {
                let turn = state.last_turn;
                state.phase = Phase::Running {
                    abort: Arc::new(AtomicBool::new(false)),
                    turn,
                    step: 0,
                };
                drop(state);
                tokio::spawn(kick(Arc::clone(self)));
            }
            Phase::Running { abort, .. } => {
                if wake_after_abort || abort.load(Ordering::SeqCst) {
                    state.wake_requested = true;
                }
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
            step: 0,
        };
        drop(state);
        tokio::spawn(kick(Arc::clone(inner)));
        return;
    }
    inner.state.lock().expect("agent 状态锁被毒化").phase = Phase::Idle;
    inner.idle_notify.notify_waiters();
}

async fn run_turn(inner: &Arc<Inner>) -> Result<bool, AgentError> {
    let (current_turn, mut step, abort) = {
        let state = inner.state.lock().expect("agent 状态锁被毒化");
        match &state.phase {
            Phase::Running { abort, turn, step } => (*turn, *step, Arc::clone(abort)),
            Phase::Idle => return Ok(false),
        }
    };
    let turn = current_turn + 1;
    inner.append(&[EventKind::TurnStart { turn }])?;
    {
        let mut state = inner.state.lock().expect("agent 状态锁被毒化");
        if let Phase::Running { turn: current, .. } = &mut state.phase {
            *current = turn;
        }
    }

    let mut turn_end: Option<TurnEndReason> = None;
    let steps = run_turn_steps(inner, turn, &mut step, abort.as_ref(), &mut turn_end).await;
    {
        let mut state = inner.state.lock().expect("agent 状态锁被毒化");
        if let Phase::Running { step: current, .. } = &mut state.phase {
            *current = step;
        }
    }

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
    match steps {
        Ok(()) => Ok(inner
            .inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .has_pending()),
        Err(error) => Err(error),
    }
}

/// 一轮里不断开 step：turn 开始时领取一条 follow-up，加上全部 next-step；
/// 之后的 step 只领取 next-step。执行了工具、轮次还没结束时继续开下一步。
async fn run_turn_steps(
    inner: &Arc<Inner>,
    turn: u64,
    step: &mut u64,
    abort: &AtomicBool,
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
    abort: &AtomicBool,
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
        let has_read = inner.tools.get("read").is_some();
        let message = Message::text(Role::System, system_prompt(&inner.cwd, has_read));
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

    let messages = inner
        .store
        .lock()
        .expect("agent 会话锁被毒化")
        .derive_messages();
    let tool_schemas = inner.tools.definitions();
    let mut stream = inner.model.stream(&messages, &tool_schemas).await?;
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut usage: Option<Usage> = None;
    let mut finish: Option<FinishReason> = None;
    loop {
        if abort.load(Ordering::SeqCst) {
            return Ok(StepEnd::Aborted);
        }
        let Some(chunk) = stream.next().await else {
            break;
        };
        match chunk? {
            StreamChunk::BlockStart { .. } => {}
            StreamChunk::TextDelta { text, .. } => {
                let _ = inner.events.send(AgentEvent::AssistantTextDelta { text });
            }
            StreamChunk::BlockEnd { block, .. } => blocks.push(block),
            StreamChunk::Usage(reported) => usage = Some(reported),
            StreamChunk::Finish(reason) => finish = Some(reason),
        }
    }
    let Some(finish) = finish else {
        return Err(AgentError::Model(ModelError::new(
            model::ErrorCode::StreamClosed,
            "模型流没有给出结束原因",
        )));
    };
    if !blocks.is_empty() {
        let message = Message::new(Role::Assistant, blocks.clone());
        inner.append(&[EventKind::AssistantMessage {
            turn,
            step,
            message,
            usage,
            interrupted: None,
        }])?;
    }
    if finish == FinishReason::MaxTokens {
        // 被截断的输出里可能有不完整的工具调用：这一轮直接以 max-tokens 收尾，不执行工具。
        return Ok(StepEnd::MaxTokens);
    }
    let calls: Vec<ToolCall> = blocks
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
    let context = ToolContext::new(inner.cwd.clone());
    let results = execute_batch(
        &inner.tools,
        &calls,
        inner.max_parallel_tool_calls,
        &context,
    )
    .await;
    for result in &results {
        let message = Message::tool_result(
            result.call_id.clone(),
            result.content.clone(),
            result.is_error,
        );
        inner.append(&[EventKind::ToolResult {
            turn,
            step,
            message,
            error_code: result.error_code.clone(),
        }])?;
    }
    Ok(StepEnd::Continue)
}

/// 系统提示：固定文字 + 会话的工作目录；read 工具可见时补 DSH 原文的 read 指导句。
fn system_prompt(cwd: &Path, has_read: bool) -> String {
    let mut prompt = format!(
        "You are AIOS, a coding agent. Your working directory is {}.",
        cwd.display()
    );
    if has_read {
        prompt.push_str(
            "\n\nUse the read tool — not shell commands like cat — to inspect text files. Use offset and limit to continue reading large files.",
        );
    }
    prompt
}
