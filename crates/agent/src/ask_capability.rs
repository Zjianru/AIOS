//! ask 能力（T006.1d2）：ask_user_question 的拦截、提问、等待、答案 tool/result。
//!
//! 本票只搬结构：answers {id, answer} 形状/多问题语义照 T005.12；只拦批内第一条 ask 调用；
//! 日志只写 question/asked、question/cancelled；answers tool/result 由 driver 在 ToolResult 阶段统一写。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::Question;
use session::EventKind;
use tokio::sync::Notify;
use tools::ToolResult;

use crate::capabilities::{Capability, HookEvent, HookFuture, HookOutcome, HookPoint};
use crate::{AgentEvent, QuestionOption};

struct PendingQuestion {
    questions: Vec<Question>,
    answers: BTreeMap<String, String>,
    deliver: Option<tokio::sync::oneshot::Sender<Vec<(String, String)>>>,
}

/// ask 能力的 per-Agent 状态：待答问题 / 已收答案 / 取消唤醒（T005.10 语义）。
pub(crate) struct AskState {
    pending: Mutex<Option<PendingQuestion>>,
    cancel_notify: Notify,
    #[cfg(test)]
    wait_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl AskState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(None),
            cancel_notify: Notify::new(),
            #[cfg(test)]
            wait_hook: Mutex::new(None),
        })
    }

    /// 回答一个问题；question_id 必须匹配当前等待的问题且未答过。
    pub(crate) fn answer(&self, question_id: &str, text: &str) -> bool {
        let mut pending = self.pending.lock().expect("agent 问题锁被毒化");
        let Some(current) = pending.as_mut() else {
            return false;
        };
        if !current.questions.iter().any(|q| q.id == question_id) {
            return false;
        }
        if current.answers.contains_key(question_id) {
            return false;
        }
        current
            .answers
            .insert(question_id.to_owned(), text.to_owned());
        if current.answers.len() < current.questions.len() {
            return true;
        }
        let Some(sender) = current.deliver.take() else {
            return false;
        };
        let ordered: Vec<(String, String)> = current
            .questions
            .iter()
            .map(|question| (question.id.clone(), current.answers[&question.id].clone()))
            .collect();
        sender.send(ordered).is_ok()
    }

    /// cancel() 时唤醒等待中的 ask 能力（Notify 不存 permit，能力侧先 enable 再查 abort）。
    pub(crate) fn cancel_waiters(&self) {
        self.cancel_notify.notify_waiters();
    }

    fn set_pending(
        &self,
        questions: Vec<Question>,
        deliver: tokio::sync::oneshot::Sender<Vec<(String, String)>>,
    ) {
        *self.pending.lock().expect("agent 问题锁被毒化") = Some(PendingQuestion {
            questions,
            answers: BTreeMap::new(),
            deliver: Some(deliver),
        });
    }

    fn take_answers(&self) -> BTreeMap<String, String> {
        self.pending
            .lock()
            .expect("agent 问题锁被毒化")
            .take()
            .map(|pending| pending.answers)
            .unwrap_or_default()
    }

    /// 等答案；取消时 abort 置位并通过 cancel_notify 唤醒，返回 None。
    async fn wait(
        &self,
        mut receiver: tokio::sync::oneshot::Receiver<Vec<(String, String)>>,
        abort: &Arc<AtomicBool>,
    ) -> Option<Vec<(String, String)>> {
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
            return None;
        }
        loop {
            tokio::select! {
                result = &mut receiver => return result.ok(),
                () = &mut cancelled => {
                    if abort.load(Ordering::SeqCst) {
                        return None;
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

    fn answer_question(
        &self,
        question_id: &str,
        selected: Vec<String>,
        custom: Option<String>,
    ) -> bool {
        // 骨架：先把 DSH 形状折算成旧的单文本答案，行为不变；实现提交改成 selected/custom。
        let text = custom.unwrap_or_else(|| selected.join(","));
        self.state.answer(question_id, &text)
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
            // 本票只拦批内第一条 ask 调用（与旧实现一致）。
            let Some(index) = calls
                .iter()
                .position(|call| call.name == "ask_user_question")
            else {
                return Ok(HookOutcome::Continue);
            };
            let call = &calls[index];
            let Some(questions) = questions_from_call(call) else {
                return Ok(HookOutcome::ToolResultOverride {
                    index,
                    result: ToolResult {
                        call_id: call.id.clone(),
                        content: "Error: invalid ask_user_question arguments (each question needs non-empty id and question)".to_owned(),
                        is_error: true,
                        error_code: Some("ASK_USER_INVALID".to_owned()),
                    },
                });
            };
            let (sender, receiver) = tokio::sync::oneshot::channel::<Vec<(String, String)>>();
            self.state.set_pending(questions.clone(), sender);
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
            let _ = event.host().events().send(AgentEvent::QuestionAsked {
                questions: questions.clone(),
            });
            let abort = event
                .host()
                .current_abort()
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
            let answered = self.state.wait(receiver, &abort).await;
            let partial = self.state.take_answers();
            match answered {
                Some(answers) => Ok(HookOutcome::ToolResultOverride {
                    index,
                    result: ToolResult::success(call.id.clone(), answers_json(&answers)),
                }),
                None => {
                    for question in &questions {
                        if !partial.contains_key(&question.id) {
                            event
                                .host()
                                .append(&[EventKind::QuestionCancelled {
                                    turn: *turn,
                                    step: *step,
                                    question_id: question.id.clone(),
                                }])
                                .await?;
                        }
                    }
                    Ok(HookOutcome::AbortStep)
                }
            }
        })
    }
}

/// 从 ask_user_question 的调用参数里取全部问题（字段照 DSH tool-ask-user）。
/// 任一问题缺 id 或 question 文本：整单拒绝（返回 None），不静默丢弃。
fn questions_from_call(call: &tools::ToolCall) -> Option<Vec<Question>> {
    let value: serde_json::Value = serde_json::from_str(&call.arguments).ok()?;
    let items = value.get("questions")?.as_array()?;
    if items.is_empty() {
        return None;
    }
    let mut questions = Vec::with_capacity(items.len());
    let mut seen_ids = std::collections::BTreeSet::new();
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

/// 工具结果内容：DSH 形态 answers[{id, answer}]（按问题顺序）。
fn answers_json(answers: &[(String, String)]) -> String {
    let answers: Vec<serde_json::Value> = answers
        .iter()
        .map(|(id, answer)| serde_json::json!({"id": id, "answer": answer}))
        .collect();
    serde_json::json!({"answers": answers}).to_string()
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
                let _ = entered_tx.send(());
                let _ = release_rx.lock().unwrap().recv();
            }));
        }
        let (_sender, receiver) = tokio::sync::oneshot::channel::<Vec<(String, String)>>();
        let wait_state = Arc::clone(&state);
        let abort_task = Arc::clone(&abort);
        let task = tokio::spawn(async move { wait_state.wait(receiver, &abort_task).await });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        abort.store(true, Ordering::SeqCst);
        state.cancel_waiters();
        let _ = release_tx.send(());
        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("cancel 在注册前发生也不能挂住");
        assert_eq!(answer.unwrap(), None);
    }

    #[test]
    fn questions_from_call_requires_all_questions_valid() {
        let call = |arguments: &str| tools::ToolCall {
            id: "call-1".to_owned(),
            name: "ask_user_question".to_owned(),
            arguments: arguments.to_owned(),
        };
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
            r#"{"questions": [{"id": "q1", "question": "P1?"}, {"id": "q2", "question": "P2?"}]}"#,
        ))
        .unwrap();
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[1].id, "q2");
        assert_eq!(questions[1].question, "P2?");
    }
}
