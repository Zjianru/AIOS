//! ask 能力（T006.1d2 / T006.13）：ask_user_question 的拦截、提问、等待、答案 tool/result。
//!
//! 本票照 DSH 改回：答案形状 answers[{id, selected[], custom?}]（K14，
//! tool-ask-user/src/index.ts:111-116）；回答方接缝（Agent::answer /
//! Agent::cancel_question，宿主方法在 T006.16a）；问题/答案落 durable 事件
//! （question/asked 带 callId、question/answered、question/cancelled），待答问题跨重启保留
//! （D45：fold question/asked - answered - cancelled）；重启后的回答以 DSH 的迟到回答消息接续
//! （source=user-question-reply，内容 answer_to_pending_question JSON，
//! user-questions/src/index.ts:162-210）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::Question;
use session::EventKind;
use tokio::sync::Notify;
use tools::ToolResult;

use crate::capabilities::{
    BoxedFuture, Capability, HookEvent, HookFuture, HookOutcome, HookPoint, LoopAccess,
};
use crate::{AgentEvent, QuestionOption};

/// 一条答案（DSH AskUserQuestionAnswerItem）：选中的选项标签 + 可选自由文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnswerItem {
    pub(crate) selected: Vec<String>,
    pub(crate) custom: Option<String>,
}

/// 等待中的一批问题（一次 ask_user_question 调用，批内可能多题）。
struct LiveQuestion {
    turn: u64,
    step: u64,
    call_id: String,
    questions: Vec<Question>,
    answers: BTreeMap<String, AnswerItem>,
    cancelled: BTreeSet<String>,
    /// 整批 settle（全部答完或撤销）时唤醒 hook；只发一次。
    deliver: Option<tokio::sync::oneshot::Sender<()>>,
}

impl LiveQuestion {
    fn settled(&self) -> bool {
        self.answers.len() + self.cancelled.len() == self.questions.len()
    }

    fn is_open(&self, question_id: &str) -> bool {
        self.questions
            .iter()
            .any(|question| question.id == question_id)
            && !self.answers.contains_key(question_id)
            && !self.cancelled.contains(question_id)
    }
}

/// 重启恢复的一批问题（D45：从日志折出，回答整批答完前只在内存累积）。
struct RestoredQuestion {
    call_id: String,
    turn: u64,
    step: u64,
    questions: Vec<Question>,
    answers: BTreeMap<String, AnswerItem>,
}

impl RestoredQuestion {
    fn is_open(&self, question_id: &str) -> bool {
        self.questions
            .iter()
            .any(|question| question.id == question_id)
            && !self.answers.contains_key(question_id)
    }
}

/// 一批问题 settle 后的 durable 事实（hook 据此写事件、写 tool/result）。
struct SettledLive {
    turn: u64,
    step: u64,
    call_id: String,
    /// 已答的问题（按问题顺序）。
    answers: Vec<(String, AnswerItem)>,
    /// 被撤销的问题 id（按问题顺序）。
    cancelled: Vec<String>,
}

/// 恢复的问题整批答完：driver 写 question/answered 并投递迟到回答。
pub(crate) struct RestoredReply {
    pub(crate) turn: u64,
    pub(crate) step: u64,
    pub(crate) call_id: String,
    pub(crate) questions: Vec<Question>,
    /// 全部答案（按问题顺序）。
    pub(crate) answers: Vec<(String, AnswerItem)>,
}

enum RestoredAnswerOutcome {
    Rejected,
    /// 收下了，但整批还没答完（只在内存；整批答完才落日志，照 DSH 的回答是整批提交）。
    Accepted,
    Completed(RestoredReply),
}

/// 回答一个待答问题的结果（driver 据此落日志/投递迟到回答）。
pub(crate) enum AnswerOutcome {
    /// 没有这个待答问题（或已答/已撤销）。
    Rejected,
    /// 已收下：等待中的批由 hook 收尾，恢复的批留在内存等整批。
    Accepted,
    /// 恢复的批整批答完。
    RestoredReply(RestoredReply),
}

/// 恢复的问题被撤销（整批 dismiss）：driver 先补写已收答案，再写撤销。
pub(crate) struct RestoredCancel {
    pub(crate) turn: u64,
    pub(crate) step: u64,
    pub(crate) call_id: String,
    /// 内存里已收下、还没落日志的答案。
    pub(crate) answered: Vec<(String, AnswerItem)>,
    /// 被撤销的问题 id（按问题顺序；批里剩余的全部）。
    pub(crate) cancelled: Vec<String>,
}

/// 撤销一个待答问题的结果。
pub(crate) enum CancelOutcome {
    Rejected,
    /// 等待中的问题：已标记撤销；整批 settle 后 hook 以 ASK_ABORTED 收尾。
    Live,
    /// 恢复的问题：已从待答集合移除，driver 写事件。
    Restored(RestoredCancel),
}

/// ask 能力的 per-Agent 状态：等待中的批 / 重启恢复的批 / 取消唤醒（T005.10 语义）。
pub(crate) struct AskState {
    live: Mutex<Option<LiveQuestion>>,
    restored: Mutex<Vec<RestoredQuestion>>,
    cancel_notify: Notify,
    #[cfg(test)]
    wait_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl AskState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            live: Mutex::new(None),
            restored: Mutex::new(Vec::new()),
            cancel_notify: Notify::new(),
            #[cfg(test)]
            wait_hook: Mutex::new(None),
        })
    }

    fn set_pending(
        &self,
        turn: u64,
        step: u64,
        call_id: String,
        questions: Vec<Question>,
        deliver: tokio::sync::oneshot::Sender<()>,
    ) {
        *self.live.lock().expect("agent 问题锁被毒化") = Some(LiveQuestion {
            turn,
            step,
            call_id,
            questions,
            answers: BTreeMap::new(),
            cancelled: BTreeSet::new(),
            deliver: Some(deliver),
        });
    }

    fn answer_live(
        &self,
        question_id: &str,
        selected: Vec<String>,
        custom: Option<String>,
    ) -> bool {
        let mut live = self.live.lock().expect("agent 问题锁被毒化");
        let Some(live) = live.as_mut() else {
            return false;
        };
        if !live.is_open(question_id) {
            return false;
        }
        live.answers
            .insert(question_id.to_owned(), AnswerItem { selected, custom });
        if live.settled()
            && let Some(deliver) = live.deliver.take()
        {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "等待方可能已取消；唤醒信号丢失不影响状态判定"
            )]
            let _ = deliver.send(());
        }
        true
    }

    fn answer_restored(
        &self,
        question_id: &str,
        selected: Vec<String>,
        custom: Option<String>,
    ) -> RestoredAnswerOutcome {
        let mut restored = self.restored.lock().expect("agent 恢复问题锁被毒化");
        let Some(index) = restored.iter().position(|group| group.is_open(question_id)) else {
            return RestoredAnswerOutcome::Rejected;
        };
        let group = &mut restored[index];
        group
            .answers
            .insert(question_id.to_owned(), AnswerItem { selected, custom });
        if group.answers.len() < group.questions.len() {
            return RestoredAnswerOutcome::Accepted;
        }
        let group = restored.remove(index);
        let answers = group
            .questions
            .iter()
            .map(|question| (question.id.clone(), group.answers[&question.id].clone()))
            .collect();
        RestoredAnswerOutcome::Completed(RestoredReply {
            turn: group.turn,
            step: group.step,
            call_id: group.call_id,
            questions: group.questions,
            answers,
        })
    }

    fn cancel_live(&self, question_id: &str) -> bool {
        let mut live = self.live.lock().expect("agent 问题锁被毒化");
        let Some(live) = live.as_mut() else {
            return false;
        };
        if !live.is_open(question_id) {
            return false;
        }
        live.cancelled.insert(question_id.to_owned());
        if live.settled()
            && let Some(deliver) = live.deliver.take()
        {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "等待方可能已取消；唤醒信号丢失不影响状态判定"
            )]
            let _ = deliver.send(());
        }
        true
    }

    fn cancel_restored(&self, question_id: &str) -> Option<RestoredCancel> {
        let mut restored = self.restored.lock().expect("agent 恢复问题锁被毒化");
        let index = restored
            .iter()
            .position(|group| group.is_open(question_id))?;
        let group = restored.remove(index);
        // 关闭不答是整批 dismiss（照 DSH：关闭问题面板不落回复）；已收下的答案先补成事实。
        let answered: Vec<(String, AnswerItem)> = group
            .questions
            .iter()
            .filter_map(|question| {
                group
                    .answers
                    .get(&question.id)
                    .map(|answer| (question.id.clone(), answer.clone()))
            })
            .collect();
        let cancelled: Vec<String> = group
            .questions
            .iter()
            .filter(|question| !group.answers.contains_key(&question.id))
            .map(|question| question.id.clone())
            .collect();
        Some(RestoredCancel {
            turn: group.turn,
            step: group.step,
            call_id: group.call_id,
            answered,
            cancelled,
        })
    }

    fn answer(
        &self,
        question_id: &str,
        selected: Vec<String>,
        custom: Option<String>,
    ) -> AnswerOutcome {
        if self.answer_live(question_id, selected.clone(), custom.clone()) {
            return AnswerOutcome::Accepted;
        }
        match self.answer_restored(question_id, selected, custom) {
            RestoredAnswerOutcome::Rejected => AnswerOutcome::Rejected,
            RestoredAnswerOutcome::Accepted => AnswerOutcome::Accepted,
            RestoredAnswerOutcome::Completed(reply) => AnswerOutcome::RestoredReply(reply),
        }
    }

    fn cancel(&self, question_id: &str) -> CancelOutcome {
        if self.cancel_live(question_id) {
            return CancelOutcome::Live;
        }
        match self.cancel_restored(question_id) {
            Some(cancel) => CancelOutcome::Restored(cancel),
            None => CancelOutcome::Rejected,
        }
    }

    /// 等待被唤醒后取走整批的 durable 事实（hook 写事件用）。
    fn take_settled(&self) -> Option<SettledLive> {
        let live = self.live.lock().expect("agent 问题锁被毒化").take()?;
        let answers = live
            .questions
            .iter()
            .filter_map(|question| {
                live.answers
                    .get(&question.id)
                    .map(|answer| (question.id.clone(), answer.clone()))
            })
            .collect();
        let cancelled = live
            .questions
            .iter()
            .filter(|question| live.cancelled.contains(&question.id))
            .map(|question| question.id.clone())
            .collect();
        Some(SettledLive {
            turn: live.turn,
            step: live.step,
            call_id: live.call_id,
            answers,
            cancelled,
        })
    }

    /// 当前待答问题（等待中 + 重启恢复；不含已答/已撤销）。
    pub(crate) fn pending_questions(&self) -> Vec<Question> {
        let mut pending = Vec::new();
        if let Some(live) = self.live.lock().expect("agent 问题锁被毒化").as_ref() {
            pending.extend(
                live.questions
                    .iter()
                    .filter(|question| live.is_open(&question.id))
                    .cloned(),
            );
        }
        for group in self.restored.lock().expect("agent 恢复问题锁被毒化").iter() {
            pending.extend(
                group
                    .questions
                    .iter()
                    .filter(|question| group.is_open(&question.id))
                    .cloned(),
            );
        }
        pending
    }

    /// cancel() 时唤醒等待中的 ask 能力（Notify 不存 permit，能力侧先 enable 再查 abort）。
    pub(crate) fn cancel_waiters(&self) {
        self.cancel_notify.notify_waiters();
    }

    /// 等整批 settle；取消整轮时 abort 置位并通过 cancel_notify 唤醒，返回 false。
    async fn wait(
        &self,
        mut receiver: tokio::sync::oneshot::Receiver<()>,
        abort: &Arc<AtomicBool>,
    ) -> bool {
        #[cfg(test)]
        {
            let hook = self
                .wait_hook
                .lock()
                .expect("question wait hook 锁被毒化")
                .clone();
            if let Some(hook) = hook {
                hook();
            }
        }
        // 先注册并 enable cancellation，再检查 abort：否则 cancel 在「检查 abort」与
        // 「注册 notified」之间调用 notify_waiters（Notify 不存 permit），通知会丢。
        let cancelled = self.cancel_notify.notified();
        tokio::pin!(cancelled);
        cancelled.as_mut().enable();
        if abort.load(Ordering::SeqCst) {
            return false;
        }
        loop {
            tokio::select! {
                result = &mut receiver => return result.is_ok(),
                () = &mut cancelled => {
                    if abort.load(Ordering::SeqCst) {
                        return false;
                    }
                    cancelled.set(self.cancel_notify.notified());
                }
            }
        }
    }
}

pub(crate) fn new_state() -> Arc<AskState> {
    AskState::new()
}

/// 从日志折出重启后仍待答的问题（D45）：question/asked 建批、question/answered 与
/// question/cancelled 关闭问题；同一 callId 的剩余问题保留。问题定义优先取同 callId 的
/// tool/call 参数（照 DSH projection 从 tool/call 读问题，选项描述不丢）。
pub(crate) fn rehydrate(state: &AskState, events: &[session::Event]) {
    let mut details: BTreeMap<String, Vec<Question>> = BTreeMap::new();
    for event in events {
        if let EventKind::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } = &event.kind
            && name == "ask_user_question"
            && let Some(questions) = questions_from_arguments(arguments)
        {
            details.insert(call_id.clone(), questions);
        }
    }
    struct Group {
        turn: u64,
        step: u64,
        questions: Vec<Question>,
        answered: BTreeSet<String>,
        cancelled: BTreeSet<String>,
    }
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for event in events {
        match &event.kind {
            EventKind::QuestionAsked {
                turn,
                step,
                call_id,
                question_id,
                question,
                header,
                options,
                multi_select,
            } => {
                let group = groups.entry(call_id.clone()).or_insert_with(|| {
                    order.push(call_id.clone());
                    Group {
                        turn: *turn,
                        step: *step,
                        questions: Vec::new(),
                        answered: BTreeSet::new(),
                        cancelled: BTreeSet::new(),
                    }
                });
                group.questions.push(Question {
                    id: question_id.clone(),
                    question: question.clone(),
                    header: header.clone(),
                    options: options
                        .iter()
                        .map(|label| QuestionOption {
                            label: label.clone(),
                            description: None,
                        })
                        .collect(),
                    multi_select: *multi_select,
                });
            }
            EventKind::QuestionAnswered {
                call_id,
                question_id,
                ..
            } => {
                if let Some(group) = groups.get_mut(call_id) {
                    group.answered.insert(question_id.clone());
                }
            }
            EventKind::QuestionCancelled {
                call_id,
                question_id,
                ..
            } => {
                if let Some(group) = groups.get_mut(call_id) {
                    group.cancelled.insert(question_id.clone());
                }
            }
            _ => {}
        }
    }
    let mut restored = Vec::new();
    for call_id in order {
        let Some(mut group) = groups.remove(&call_id) else {
            continue;
        };
        if let Some(questions) = details.remove(&call_id) {
            group.questions = questions;
        }
        let Group {
            turn,
            step,
            questions,
            answered,
            cancelled,
        } = group;
        let open: Vec<Question> = questions
            .into_iter()
            .filter(|question| {
                !answered.contains(&question.id) && !cancelled.contains(&question.id)
            })
            .collect();
        if open.is_empty() {
            continue;
        }
        restored.push(RestoredQuestion {
            call_id,
            turn,
            step,
            questions: open,
            answers: BTreeMap::new(),
        });
    }
    *state.restored.lock().expect("agent 恢复问题锁被毒化") = restored;
}

pub(crate) struct AskCapability {
    state: Arc<AskState>,
}

impl AskCapability {
    pub(crate) fn new(state: Arc<AskState>) -> Self {
        Self { state }
    }
}

impl Capability for AskCapability {
    fn name(&self) -> &'static str {
        "ask"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::ToolBefore]
    }

    fn answer_with<'a>(
        &'a self,
        host: &'a dyn LoopAccess,
        question_id: &'a str,
        selected: Vec<String>,
        custom: Option<String>,
    ) -> BoxedFuture<'a, bool> {
        Box::pin(async move {
            match self.state.answer(question_id, selected, custom) {
                AnswerOutcome::Rejected => Ok(false),
                AnswerOutcome::Accepted => Ok(true),
                AnswerOutcome::RestoredReply(reply) => {
                    let events: Vec<EventKind> = reply
                        .answers
                        .iter()
                        .map(|(question_id, answer)| EventKind::QuestionAnswered {
                            turn: reply.turn,
                            step: reply.step,
                            call_id: reply.call_id.clone(),
                            question_id: question_id.clone(),
                            selected: answer.selected.clone(),
                            custom: answer.custom.clone(),
                        })
                        .collect();
                    if host.append(&events).await.is_err() {
                        return Ok(false);
                    }
                    Ok(host
                        .enqueue_message(late_reply_message(&reply))
                        .await
                        .unwrap_or(false))
                }
            }
        })
    }

    fn cancel_question_with<'a>(
        &'a self,
        host: &'a dyn LoopAccess,
        question_id: &'a str,
    ) -> BoxedFuture<'a, bool> {
        Box::pin(async move {
            match self.state.cancel(question_id) {
                CancelOutcome::Rejected => Ok(false),
                CancelOutcome::Live => Ok(true),
                CancelOutcome::Restored(cancel) => {
                    let mut events: Vec<EventKind> = cancel
                        .answered
                        .iter()
                        .map(|(question_id, answer)| EventKind::QuestionAnswered {
                            turn: cancel.turn,
                            step: cancel.step,
                            call_id: cancel.call_id.clone(),
                            question_id: question_id.clone(),
                            selected: answer.selected.clone(),
                            custom: answer.custom.clone(),
                        })
                        .collect();
                    events.extend(cancel.cancelled.iter().map(|question_id| {
                        EventKind::QuestionCancelled {
                            turn: cancel.turn,
                            step: cancel.step,
                            call_id: cancel.call_id.clone(),
                            question_id: question_id.clone(),
                        }
                    }));
                    Ok(host.append(&events).await.is_ok())
                }
            }
        })
    }

    fn pending_questions(&self) -> Vec<Question> {
        self.state.pending_questions()
    }

    fn cancel_questions(&self) {
        self.state.cancel_waiters();
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::ToolBefore {
                turn, step, calls, ..
            } = event
            else {
                return Ok(HookOutcome::Continue);
            };
            // T006.13b：批内全部 ask_user_question 按模型顺序依次拦截；每个调用各自
            // 等待回答/撤销（DSH tool-ask-user 每个调用独立走 userQuestions.ask），
            // 结果先收集、最后一次性返回，driver 按 index 回填且批内顺序不变。
            let ask_indices: Vec<usize> = calls
                .iter()
                .enumerate()
                .filter(|(_, call)| call.name == "ask_user_question")
                .map(|(index, _)| index)
                .collect();
            if ask_indices.is_empty() {
                return Ok(HookOutcome::Continue);
            }
            let mut overrides: Vec<(usize, ToolResult)> = Vec::new();
            for index in ask_indices {
                let call = &calls[index];
                let Some(questions) = questions_from_call(call) else {
                    overrides.push((
                        index,
                        ToolResult {
                            call_id: call.id.clone(),
                            content: "Error: invalid ask_user_question arguments (each question needs non-empty id and question)".to_owned(),
                            is_error: true,
                            error_code: Some("ASK_USER_INVALID".to_owned()),
                            meta: None,
                        },
                    ));
                    continue;
                };
                let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
                self.state
                    .set_pending(*turn, *step, call.id.clone(), questions.clone(), sender);
                for question in &questions {
                    event
                        .host()
                        .append(&[EventKind::QuestionAsked {
                            turn: *turn,
                            step: *step,
                            call_id: call.id.clone(),
                            question_id: question.id.clone(),
                            question: question.question.clone(),
                            header: question.header.clone(),
                            options: question
                                .options
                                .iter()
                                .map(|option| option.label.clone())
                                .collect(),
                            multi_select: question.multi_select,
                        }])
                        .await?;
                }
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "页面/服务未在线时问题事件丢弃；追问本身照常进行"
                )]
                let _ = event.host().events().send(AgentEvent::QuestionAsked {
                    questions: questions.clone(),
                });
                let abort = event
                    .host()
                    .current_abort()
                    .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
                let batch_settled = self.state.wait(receiver, &abort).await;
                let Some(settled) = self.state.take_settled() else {
                    // 状态被别处取走（理论上不会）；不作为 aborted 处理，免得吞掉本轮。
                    return Ok(HookOutcome::Continue);
                };
                if !settled.answers.is_empty() {
                    let events: Vec<EventKind> = settled
                        .answers
                        .iter()
                        .map(|(question_id, answer)| EventKind::QuestionAnswered {
                            turn: settled.turn,
                            step: settled.step,
                            call_id: settled.call_id.clone(),
                            question_id: question_id.clone(),
                            selected: answer.selected.clone(),
                            custom: answer.custom.clone(),
                        })
                        .collect();
                    event.host().append(&events).await?;
                }
                if !settled.cancelled.is_empty() {
                    let events: Vec<EventKind> = settled
                        .cancelled
                        .iter()
                        .map(|question_id| EventKind::QuestionCancelled {
                            turn: settled.turn,
                            step: settled.step,
                            call_id: settled.call_id.clone(),
                            question_id: question_id.clone(),
                        })
                        .collect();
                    event.host().append(&events).await?;
                }
                if !batch_settled {
                    // 整轮取消（Agent::cancel）：保留旧行为，按 aborted 收尾；未答的题写撤销。
                    let answered: BTreeSet<&String> = settled
                        .answers
                        .iter()
                        .map(|(question_id, _)| question_id)
                        .collect();
                    let mut undone: Vec<EventKind> = Vec::new();
                    for question in &questions {
                        if !answered.contains(&question.id)
                            && !settled.cancelled.iter().any(|id| id == &question.id)
                        {
                            undone.push(EventKind::QuestionCancelled {
                                turn: settled.turn,
                                step: settled.step,
                                call_id: settled.call_id.clone(),
                                question_id: question.id.clone(),
                            });
                        }
                    }
                    if !undone.is_empty() {
                        event.host().append(&undone).await?;
                    }
                    return Ok(HookOutcome::AbortStep);
                }
                if settled.cancelled.is_empty() {
                    overrides.push((
                        index,
                        ToolResult::success(call.id.clone(), answers_json(&settled.answers)),
                    ));
                } else {
                    // 撤销（提问服务路径）：模型收到 DSH 的 ASK_ABORTED 工具错误，本轮继续。
                    overrides.push((
                        index,
                        ToolResult {
                            call_id: call.id.clone(),
                            content: "ask_user_question was aborted before the user answered"
                                .to_owned(),
                            is_error: true,
                            error_code: Some("ASK_ABORTED".to_owned()),
                            meta: None,
                        },
                    ));
                }
            }
            Ok(HookOutcome::ToolResultOverrides { overrides })
        })
    }
}

/// 从 ask_user_question 的调用参数里取全部问题（字段照 DSH tool-ask-user）。
/// 任一问题缺 id 或 question 文本：整单拒绝（返回 None），不静默丢弃。
fn questions_from_call(call: &tools::ToolCall) -> Option<Vec<Question>> {
    questions_from_arguments(&call.arguments)
}

fn questions_from_arguments(arguments: &str) -> Option<Vec<Question>> {
    let value: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let items = value.get("questions")?.as_array()?;
    if items.is_empty() {
        return None;
    }
    let mut questions = Vec::with_capacity(items.len());
    let mut seen_ids = BTreeSet::new();
    for item in items {
        let id = item.get("id").and_then(|value| value.as_str())?;
        let question = item.get("question").and_then(|value| value.as_str())?;
        if id.is_empty() || question.is_empty() || !seen_ids.insert(id) {
            return None;
        }
        let options = item
            .get("options")
            .and_then(|value| value.as_array())
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| {
                        let label = option.get("label")?.as_str()?;
                        Some(QuestionOption {
                            label: label.to_owned(),
                            description: option
                                .get("description")
                                .and_then(|value| value.as_str())
                                .map(str::to_owned),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        questions.push(Question {
            id: id.to_owned(),
            question: question.to_owned(),
            header: item
                .get("header")
                .and_then(|value| value.as_str())
                .map(str::to_owned),
            options,
            multi_select: item
                .get("multi_select")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        });
    }
    Some(questions)
}

/// 一条答案的 wire 形状（DSH AskUserQuestionAnswerItem）：selected 恒在，custom 可选。
fn answer_json(id: &str, answer: &AnswerItem) -> serde_json::Value {
    let mut value = serde_json::json!({"id": id, "selected": answer.selected});
    if let Some(custom) = &answer.custom {
        value["custom"] = serde_json::json!(custom);
    }
    value
}

/// 工具结果内容：DSH 形态 answers[{id, selected[], custom?}]（按问题顺序）。
fn answers_json(answers: &[(String, AnswerItem)]) -> String {
    let values: Vec<serde_json::Value> = answers
        .iter()
        .map(|(id, answer)| answer_json(id, answer))
        .collect();
    serde_json::json!({"answers": values}).to_string()
}

/// 一条问题的 wire 形状（DSH AskUserQuestionItem 的服务词汇：multiSelect 是 camelCase，
/// options/header 可选；options 为空时省略）。
fn question_json(question: &Question) -> serde_json::Value {
    let mut value = serde_json::json!({"id": question.id, "question": question.question});
    if let Some(header) = &question.header {
        value["header"] = serde_json::json!(header);
    }
    if !question.options.is_empty() {
        let options: Vec<serde_json::Value> = question
            .options
            .iter()
            .map(|option| {
                let mut value = serde_json::json!({"label": option.label});
                if let Some(description) = &option.description {
                    value["description"] = serde_json::json!(description);
                }
                value
            })
            .collect();
        value["options"] = serde_json::json!(options);
    }
    if question.multi_select {
        value["multiSelect"] = serde_json::json!(true);
    }
    value
}

/// 迟到回答消息（DSH user-questions：steer 一条 source=user-question-reply 的 user 消息，
/// 内容是 answer_to_pending_question JSON；user-questions/src/index.ts:191-199）。
pub(crate) fn late_reply_message(reply: &RestoredReply) -> model::Message {
    let content = serde_json::json!({
        "kind": "answer_to_pending_question",
        "tool": "ask_user_question",
        "callId": reply.call_id,
        "questions": reply
            .questions
            .iter()
            .map(question_json)
            .collect::<Vec<_>>(),
        "answers": reply
            .answers
            .iter()
            .map(|(id, answer)| answer_json(id, answer))
            .collect::<Vec<_>>(),
    })
    .to_string();
    model::Message::user_question_reply(content, reply.call_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_before_notify_registration_is_not_lost() {
        let state = AskState::new();
        let abort = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        {
            let mut hook = state.wait_hook.lock().unwrap();
            *hook = Some(Arc::new(move || {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "测试闸门通道的接收端可能先退出；只用于同步步骤"
                )]
                let _ = entered_tx.send(());
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "测试闸门通道的接收端可能先退出；只用于同步步骤"
                )]
                let _ = release_rx.lock().unwrap().recv();
            }));
        }
        let (_sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let wait_state = Arc::clone(&state);
        let abort_task = Arc::clone(&abort);
        let task = tokio::spawn(async move { wait_state.wait(receiver, &abort_task).await });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        abort.store(true, Ordering::SeqCst);
        state.cancel_waiters();
        #[expect(
            clippy::let_underscore_must_use,
            reason = "测试闸门通道的接收端可能先退出；只用于同步步骤"
        )]
        let _ = release_tx.send(());
        let settled = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("cancel 在注册前发生也不能挂住");
        assert!(!settled.unwrap(), "整轮取消要让 wait 返回 false");
    }

    fn call(arguments: &str) -> tools::ToolCall {
        tools::ToolCall {
            id: "call-1".to_owned(),
            name: "ask_user_question".to_owned(),
            arguments: arguments.to_owned(),
        }
    }

    #[test]
    fn questions_from_call_requires_all_questions_valid() {
        assert!(questions_from_call(&call("not json")).is_none());
        assert!(questions_from_call(&call(r#"{}"#)).is_none());
        assert!(questions_from_call(&call(r#"{"questions": []}"#)).is_none());
        assert!(questions_from_call(&call(r#"{"questions": [{"id": "q"}]}"#)).is_none());
        assert!(
            questions_from_call(&call(r#"{"questions": [{"id": "", "question": "P?"}]}"#))
                .is_none()
        );
        assert!(
            questions_from_call(&call(
                r#"{"questions": [{"id": "dup", "question": "P1?"}, {"id": "dup", "question": "P2?"}]}"#
            ))
            .is_none(),
            "重复 question id 必须整单拒绝"
        );
        let questions = questions_from_call(&call(
            r#"{"questions": [{"id": "q1", "question": "P1?", "options": [{"label": "Yes", "description": "go"}]}, {"id": "q2", "question": "P2?"}]}"#,
        ))
        .unwrap();
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[1].id, "q2");
        assert_eq!(questions[0].options[0].description.as_deref(), Some("go"));
    }

    #[test]
    fn rehydrate_folds_asked_minus_answered_and_cancelled() {
        let events = vec![
            session::Event::new(
                1,
                0,
                EventKind::ToolCall {
                    turn: 1,
                    step: 1,
                    call_id: "call_ask".to_owned(),
                    name: "ask_user_question".to_owned(),
                    arguments: r#"{"questions": [{"id": "q1", "question": "P1?", "options": [{"label": "Yes", "description": "go"}]}, {"id": "q2", "question": "P2?"}, {"id": "q3", "question": "P3?"}]}"#.to_owned(),
                },
            ),
            session::Event::new(
                2,
                0,
                EventKind::QuestionAsked {
                    turn: 1,
                    step: 1,
                    call_id: "call_ask".to_owned(),
                    question_id: "q1".to_owned(),
                    question: "P1?".to_owned(),
                    header: None,
                    options: vec!["Yes".to_owned()],
                    multi_select: false,
                },
            ),
            session::Event::new(
                3,
                0,
                EventKind::QuestionAsked {
                    turn: 1,
                    step: 1,
                    call_id: "call_ask".to_owned(),
                    question_id: "q2".to_owned(),
                    question: "P2?".to_owned(),
                    header: None,
                    options: Vec::new(),
                    multi_select: false,
                },
            ),
            session::Event::new(
                4,
                0,
                EventKind::QuestionAsked {
                    turn: 1,
                    step: 1,
                    call_id: "call_ask".to_owned(),
                    question_id: "q3".to_owned(),
                    question: "P3?".to_owned(),
                    header: None,
                    options: Vec::new(),
                    multi_select: false,
                },
            ),
            session::Event::new(
                5,
                0,
                EventKind::QuestionAnswered {
                    turn: 1,
                    step: 1,
                    call_id: "call_ask".to_owned(),
                    question_id: "q1".to_owned(),
                    selected: vec!["Yes".to_owned()],
                    custom: None,
                },
            ),
            session::Event::new(
                6,
                0,
                EventKind::QuestionCancelled {
                    turn: 1,
                    step: 1,
                    call_id: "call_ask".to_owned(),
                    question_id: "q2".to_owned(),
                },
            ),
        ];
        let state = AskState::new();
        rehydrate(&state, &events);
        let pending = state.pending_questions();
        assert_eq!(pending.len(), 1, "只留没答也没撤的那一题：{pending:?}");
        assert_eq!(pending[0].id, "q3");
        assert_eq!(pending[0].question, "P3?");
        let state2 = AskState::new();
        rehydrate(&state2, &[]);
        assert!(state2.pending_questions().is_empty());
    }
}
