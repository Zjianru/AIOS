use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use session::{AbortCause, EventKind, InboxTarget, LlmFailure, TurnEndReason};

use crate::capabilities::HookEvent;
use crate::error::AgentError;
use crate::{AgentEvent, TurnOutcome};

use super::step::run_step;
use super::*;

pub(super) async fn kick(inner: Arc<Inner>) {
    loop {
        let has_input = inner
            .inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .has_pending();
        if !has_input {
            break;
        }
        if !matches!(run_turn(&inner).await, Ok(true)) {
            break;
        }
    }
    let status_turn = inner.state.lock().expect("agent 状态锁被毒化").last_turn;
    if let Err(error) = inner
        .capabilities
        .dispatch(&HookEvent::Status {
            turn: status_turn,
            host: inner.as_ref(),
        })
        .await
    {
        let _ = error;
    }
    settle_idle(&inner);
}

/// 收敛到 Idle；如果取消之后有人唤过，就立刻再起一个驱动。
pub(super) fn settle_idle(inner: &Arc<Inner>) {
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
        restart_kick(inner, turn);
        return;
    }
    // 仅测试可见的同步缝：让边界单测能精确停在「预检完成、最终临界区之前」。
    #[cfg(test)]
    {
        let hook = inner
            .settle_hook
            .lock()
            .expect("settle hook 锁被毒化")
            .clone();
        if let Some(hook) = hook {
            hook();
        }
    }
    // 收尾临界区：在同一把 state 锁里重查一次 wake 再转 Idle。
    let mut state = inner.state.lock().expect("agent 状态锁被毒化");
    let wake_now = std::mem::take(&mut state.wake_requested);
    if wake_now {
        drop(state);
        restart_kick(inner, turn);
        return;
    }
    state.phase = Phase::Idle;
    drop(state);
    inner.idle_notify.notify_waiters();
}

pub(super) fn restart_kick(inner: &Arc<Inner>, turn: u64) {
    let mut state = inner.state.lock().expect("agent 状态锁被毒化");
    state.phase = Phase::Running {
        abort: Arc::new(AtomicBool::new(false)),
        turn,
    };
    drop(state);
    tokio::spawn(kick(Arc::clone(inner)));
}

pub(super) async fn run_turn(inner: &Arc<Inner>) -> Result<bool, AgentError> {
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
    inner.append(&[EventKind::TurnStart { turn }]).await?;
    inner.model.begin_turn();
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
                error: LlmFailure {
                    code: "UNKNOWN".to_owned(),
                    message: "turn ended without a reason".to_owned(),
                },
            });
            let outcome = TurnOutcome::from(&reason);
            (reason, outcome)
        }
        Err(error) => {
            let reason = TurnEndReason::Error {
                error: LlmFailure {
                    code: error.code(),
                    message: error.to_string(),
                },
            };
            let outcome = TurnOutcome::from(&reason);
            (reason, outcome)
        }
    };
    inner
        .capabilities
        .dispatch(&HookEvent::TurnStopping {
            turn,
            host: inner.as_ref(),
        })
        .await?;
    let appended = inner.append(&[EventKind::TurnEnd { turn, reason }]).await;
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
pub(super) async fn run_turn_steps(
    inner: &Arc<Inner>,
    turn: u64,
    step: &mut u64,
    abort: &Arc<AtomicBool>,
    turn_end: &mut Option<TurnEndReason>,
) -> Result<(), AgentError> {
    let mut target = InboxTarget::NextTurn;
    loop {
        if abort.load(Ordering::SeqCst) {
            *turn_end = Some(TurnEndReason::Aborted {
                reason: AbortCause::User,
            });
            return Ok(());
        }
        let claimed = inner.claim_input(target, turn).await?;
        if turn_end.is_some() && claimed.is_empty() {
            return Ok(());
        }
        if *step == 0 && claimed.is_empty() {
            *turn_end = Some(TurnEndReason::Completed);
            return Ok(());
        }
        *step += 1;
        let step_number = *step;
        inner
            .append(&[EventKind::StepStart {
                turn,
                step: step_number,
            }])
            .await?;
        let outcome = run_step(inner, turn, step_number, claimed, abort).await;
        let end = match outcome {
            Ok(end) => end,
            Err(error) => {
                if let Err(cleanup_error) = inner
                    .append(&[EventKind::StepEnd {
                        turn,
                        step: step_number,
                    }])
                    .await
                {
                    return Err(AgentError::StepCleanup {
                        first: error.to_string(),
                        second: cleanup_error.to_string(),
                    });
                }
                return Err(error);
            }
        };
        inner
            .append(&[EventKind::StepEnd {
                turn,
                step: step_number,
            }])
            .await?;
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
                *turn_end = Some(TurnEndReason::Aborted {
                    reason: AbortCause::User,
                });
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
