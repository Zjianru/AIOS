//! 确认卡的提议数据与用户动作：从日志读回提议、判断新卡是否替换旧卡、记下决定。
//!
//! 提议本身是 propose_action 的 tool/call + tool/result（结果 meta 里带展示数据）；
//! 决定是 card/decision 事件。replaced 不落日志，由「后面又有被接受的新卡」推导。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actions::{ActionEnv, CardFact};
use crate::error::ServiceError;
use crate::protocol::{
    CardActRequest, CardActResponse, CardVerb, GateJudgeConclusion, GateJudgeRequest,
    GateJudgeResponse,
};
use crate::root_lifecycle::RootHost;

/// 结果卡「接受」这个动作的名字（进 card/decision 的 action）。
const ACCEPT_RESULT_ACTION: &str = "accept_task_result";
/// 门结果卡判定动作的名字（进 card/decision 的 action）。
const GATE_JUDGE_ACTION: &str = "gate_judge";

/// 模型传给 propose_action 的参数。
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProposeArgs {
    pub title: String,
    pub brief: String,
    pub options: Vec<ProposeOption>,
}

/// 一个选项：id 是本卡内唯一的键，action 是登记表里的名字，details 是动作自己的字段。
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProposeOption {
    pub id: String,
    pub action: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub details: Value,
}

/// 卡片在日志 meta 里的展示数据（模型给的文字经过一次校验/整理）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CardMeta {
    pub title: String,
    pub brief: String,
    pub options: Vec<CardMetaOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CardMetaOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub facts: Vec<CardFact>,
}

/// 一条被接受的提议（重开页面、点按钮都从日志读回它）。
#[derive(Debug, Clone)]
pub(crate) struct Proposal {
    pub call_id: String,
    pub seq: u64,
    pub args: ProposeArgs,
    pub meta: CardMeta,
}

/// 从日志里找一条被接受的提议；call id 不是 propose_action 或没有成功结果就是 None。
pub(crate) fn proposal_of(events: &[session::Event], call_id: &str) -> Option<Proposal> {
    let mut arguments = None;
    for event in events {
        if let session::EventKind::ToolCall {
            call_id: id,
            name,
            arguments: raw,
            ..
        } = &event.kind
            && id == call_id
            && name == "propose_action"
        {
            arguments = Some(raw.clone());
        }
    }
    let args: ProposeArgs = serde_json::from_str(arguments.as_deref()?).ok()?;
    let mut meta = None;
    let mut seq = None;
    for event in events {
        if let session::EventKind::ToolResult {
            message,
            error,
            meta: result_meta,
            ..
        } = &event.kind
        {
            if message.tool_call_id.as_deref() != Some(call_id) || error.is_some() {
                continue;
            }
            let Some(card) = result_meta.as_ref().and_then(|meta| meta.get("card")) else {
                continue;
            };
            meta = serde_json::from_value::<CardMeta>(card.clone()).ok();
            seq = Some(event.seq);
        }
    }
    Some(Proposal {
        call_id: call_id.to_owned(),
        seq: seq?,
        args,
        meta: meta?,
    })
}

/// 这条日志里最后一条决定（同一张卡后写的胜出）。
pub(crate) fn decision_of(
    events: &[session::Event],
    call_id: &str,
) -> Option<session::CardDecision> {
    events
        .iter()
        .rev()
        .filter_map(|event| session::CardDecision::from_event(&event.kind))
        .find(|decision| decision.call_id == call_id)
}

/// 有没有一条更晚被接受的新卡：有的话这条没动过的旧卡就是 replaced。
pub(crate) fn is_replaced(events: &[session::Event], call_id: &str) -> bool {
    let Some(proposal) = proposal_of(events, call_id) else {
        return false;
    };
    events.iter().any(|event| {
        if event.seq <= proposal.seq {
            return false;
        }
        match &event.kind {
            session::EventKind::ToolResult {
                message,
                error,
                meta: Some(meta),
                ..
            } if error.is_none() => {
                meta.get("card").is_some() && message.tool_call_id.as_deref() != Some(call_id)
            }
            _ => false,
        }
    })
}

/// 卡片展示数据（提议时写进工具结果的 meta）。
pub(crate) fn card_meta(title: &str, brief: &str, options: Vec<CardMetaOption>) -> Value {
    let meta = CardMeta {
        title: title.to_owned(),
        brief: brief.to_owned(),
        options,
    };
    serde_json::json!({ "card": meta })
}

/// 一张确认卡的串行锁：正常结束（有决定/没这张卡/被替换）就从表里摘掉自己，
/// 出错时保留——同一张卡的失败重试仍然串行，不放出两个请求同时重跑动作。
struct CardLockSlot<'a> {
    host: &'a RootHost,
    call_id: String,
    lock: Arc<tokio::sync::Mutex<()>>,
    keep: bool,
}

impl Drop for CardLockSlot<'_> {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let mut locks = self.host.card_locks.lock().expect("card locks 锁被毒化");
        if locks
            .get(&self.call_id)
            .is_some_and(|current| Arc::ptr_eq(current, &self.lock))
        {
            locks.remove(&self.call_id);
        }
    }
}

impl RootHost {
    /// 用户点了确认卡上的按钮：核对提议、跑 reconcile、执行、记决定。
    ///
    /// 未知的卡、被新卡替换的旧卡不执行也不记（返回 None）；已经有决定的卡返回那条决定，
    /// 重复点击不重复执行。
    pub async fn card_act(&self, request: CardActRequest) -> Result<CardActResponse, ServiceError> {
        // 同一张卡并发/连点只让一个请求进：第一个写决定，后面的读到决定后原样返回。
        // 不同 call_id 各有各的锁，不互相排队；正常结束后把锁摘掉，表不随 call_id 无界增长。
        let mut slot = self.card_lock(&request.call_id);
        let _guard = Arc::clone(&slot.lock).lock_owned().await;
        let result = self.card_act_serialized(&request).await;
        slot.keep = result.is_err();
        result
    }

    /// 取（或建）这张卡的串行锁；返回值在请求结束时负责清理表里的条目。
    fn card_lock(&self, call_id: &str) -> CardLockSlot<'_> {
        let lock = {
            let mut locks = self.card_locks.lock().expect("card locks 锁被毒化");
            Arc::clone(locks.entry(call_id.to_owned()).or_default())
        };
        CardLockSlot {
            host: self,
            call_id: call_id.to_owned(),
            lock,
            keep: false,
        }
    }

    /// 拿到这张卡的锁之后才执行：读日志、核对、执行、记决定。
    async fn card_act_serialized(
        &self,
        request: &CardActRequest,
    ) -> Result<CardActResponse, ServiceError> {
        let events = self.card_events().await?;
        // 任务结果卡（relay 的 request id）走接受或门判定，不是 propose_action 的确认卡。
        if let Some((task_id, report_id)) = crate::result_cards::parse_relay_id(&request.call_id) {
            if matches!(request.verb, CardVerb::Pass | CardVerb::NeedsFix) {
                return self
                    .judge_gate_card(request, task_id, report_id, &events)
                    .await;
            }
            return self
                .accept_result_card(request, task_id, report_id, &events)
                .await;
        }
        let Some(proposal) = proposal_of(&events, &request.call_id) else {
            return Ok(CardActResponse { decision: None });
        };
        if let Some(decision) = decision_of(&events, &proposal.call_id) {
            return Ok(CardActResponse {
                decision: Some(decision),
            });
        }
        if is_replaced(&events, &proposal.call_id) {
            return Ok(CardActResponse { decision: None });
        }
        let decision = match request.verb {
            CardVerb::Dismiss => session::CardDecision {
                call_id: proposal.call_id.clone(),
                option: None,
                action: None,
                phase: session::CardDecisionPhase::Dismissed,
                data: None,
            },
            CardVerb::Confirm => {
                match self
                    .confirm_card(&proposal, request.option.as_deref(), &events)
                    .await?
                {
                    Some(decision) => decision,
                    None => return Ok(CardActResponse { decision: None }),
                }
            }
            // 门判定只作用于结果卡；确认卡上没有这两个动作。
            CardVerb::Pass | CardVerb::NeedsFix => {
                return Ok(CardActResponse { decision: None });
            }
        };
        self.record_decision(&decision).await?;
        Ok(CardActResponse {
            decision: Some(decision),
        })
    }

    /// 结果卡上的「接受」：只认真的送到 Kind 对话里的那张卡，记下当前版本和报告。
    ///
    /// 重复点击返回同一条决定；旧版本/不是最新的报告回 refused，不写接受记录。
    async fn accept_result_card(
        &self,
        request: &CardActRequest,
        task_id: &str,
        report_id: &str,
        events: &[session::Event],
    ) -> Result<CardActResponse, ServiceError> {
        if let Some(decision) = decision_of(events, &request.call_id) {
            return Ok(CardActResponse {
                decision: Some(decision),
            });
        }
        // 结果卡只有「接受」；没有暂不这个动作。
        if request.verb != CardVerb::Confirm {
            return Ok(CardActResponse { decision: None });
        }
        if crate::result_cards::relay_card(events, &request.call_id, task_id).is_none() {
            return Ok(CardActResponse { decision: None });
        }
        let Some(runtime) = self.task_runtime() else {
            return Err(ServiceError::TaskUnavailable(
                "任务库没开，接受不了结果".to_owned(),
            ));
        };
        let decision = match runtime.accept_result(task_id, report_id) {
            Ok(acceptance) => session::CardDecision {
                call_id: request.call_id.clone(),
                option: None,
                action: Some(ACCEPT_RESULT_ACTION.to_owned()),
                phase: session::CardDecisionPhase::Confirmed,
                data: Some(serde_json::json!({
                    "summary": "你已接受",
                    "version": acceptance.version,
                    "report_id": acceptance.report_id.as_str(),
                })),
            },
            Err(reason) => session::CardDecision {
                call_id: request.call_id.clone(),
                option: None,
                action: Some(ACCEPT_RESULT_ACTION.to_owned()),
                phase: session::CardDecisionPhase::Refused,
                data: Some(serde_json::json!({ "summary": reason })),
            },
        };
        self.record_decision(&decision).await?;
        Ok(CardActResponse {
            decision: Some(decision),
        })
    }

    /// 门结果卡上的「通过」「需修正」：只认真的送到 Kind 对话里的那张卡。
    ///
    /// 判定由任务库在同一个事务里核对候选、去重和记；这里只把结果落成 card/decision 回执。
    async fn judge_gate_card(
        &self,
        request: &CardActRequest,
        task_id: &str,
        report_id: &str,
        events: &[session::Event],
    ) -> Result<CardActResponse, ServiceError> {
        if let Some(decision) = decision_of(events, &request.call_id) {
            return Ok(CardActResponse {
                decision: Some(decision),
            });
        }
        if crate::result_cards::relay_card(events, &request.call_id, task_id).is_none() {
            return Ok(CardActResponse { decision: None });
        }
        let Some(runtime) = self.task_runtime() else {
            return Err(ServiceError::TaskUnavailable(
                "任务库没开，判定不了".to_owned(),
            ));
        };
        let gate_id = tasks::TaskId::from(task_id);
        if !runtime
            .task_is_gate(&gate_id)
            .map_err(ServiceError::TaskUnavailable)?
        {
            return Ok(CardActResponse { decision: None });
        }
        let conclusion = match request.verb {
            CardVerb::Pass => GateJudgeConclusion::Pass,
            CardVerb::NeedsFix => GateJudgeConclusion::NeedsFix,
            CardVerb::Confirm | CardVerb::Dismiss => {
                return Ok(CardActResponse { decision: None });
            }
        };
        let judged = runtime
            .gate_judge(&GateJudgeRequest {
                gate_task: task_id.to_owned(),
                gate_report: report_id.to_owned(),
                conclusion,
                note: request.note.clone(),
            })
            .map_err(ServiceError::TaskUnavailable)?;
        let decision = if judged.changed {
            session::CardDecision {
                call_id: request.call_id.clone(),
                option: None,
                action: Some(GATE_JUDGE_ACTION.to_owned()),
                phase: session::CardDecisionPhase::Refused,
                data: Some(serde_json::json!({ "summary": judged.summary })),
            }
        } else {
            let verdict = judged.verdict.clone();
            session::CardDecision {
                call_id: request.call_id.clone(),
                option: None,
                action: Some(GATE_JUDGE_ACTION.to_owned()),
                phase: session::CardDecisionPhase::Confirmed,
                data: Some(serde_json::json!({
                    "summary": judged.summary,
                    "conclusion": match conclusion {
                        GateJudgeConclusion::Pass => "pass",
                        GateJudgeConclusion::NeedsFix => "needs_fix",
                    },
                    "note": verdict.as_ref().and_then(|verdict| verdict.note.clone()),
                    "candidate_report": verdict.as_ref().map(|verdict| verdict.candidate_report.clone()),
                    "requirement_version": verdict.as_ref().map(|verdict| verdict.requirement_version),
                    "released": judged.released,
                })),
            }
        };
        self.record_decision(&decision).await?;
        Ok(CardActResponse {
            decision: Some(decision),
        })
    }

    /// T053：门判定的内部操作（Call::GateJudge）；结果卡动作也走这里。
    pub async fn gate_judge(
        &self,
        request: GateJudgeRequest,
    ) -> Result<GateJudgeResponse, ServiceError> {
        let Some(runtime) = self.task_runtime() else {
            return Err(ServiceError::TaskUnavailable(
                "任务库没开，判定不了".to_owned(),
            ));
        };
        runtime
            .gate_judge(&request)
            .map_err(ServiceError::TaskUnavailable)
    }

    /// 一次确认：选中项 → 动作 → 核对事实 → 执行。返回要记的决定。
    async fn confirm_card(
        &self,
        proposal: &Proposal,
        option_id: Option<&str>,
        events: &[session::Event],
    ) -> Result<Option<session::CardDecision>, ServiceError> {
        let Some(option) =
            option_id.and_then(|id| proposal.args.options.iter().find(|option| option.id == id))
        else {
            return Ok(None);
        };
        let Some(kind) = self.options.actions.get(&option.action) else {
            return Ok(Some(failed_decision(
                proposal,
                option,
                "这个动作现在不可用",
            )));
        };
        let details = match kind.details(&option.details) {
            Ok(details) => details,
            Err(reason) => {
                return Ok(Some(failed_decision(proposal, option, &reason)));
            }
        };
        let facts = proposal
            .meta
            .options
            .iter()
            .find(|meta| meta.id == option.id)
            .map(|meta| meta.facts.as_slice())
            .unwrap_or(&[]);
        let env = ActionEnv {
            data_dir: &self.options.data_dir,
            identity: self.options.identity.as_deref(),
            events,
        };
        if let Err(reason) = kind.reconcile(&details, facts, &env) {
            return Ok(Some(session::CardDecision {
                call_id: proposal.call_id.clone(),
                option: Some(option.id.clone()),
                action: Some(option.action.clone()),
                phase: session::CardDecisionPhase::Refused,
                data: Some(serde_json::json!({ "summary": reason })),
            }));
        }
        match kind.act(&details, self).await {
            Ok(outcome) => Ok(Some(session::CardDecision {
                call_id: proposal.call_id.clone(),
                option: Some(option.id.clone()),
                action: Some(option.action.clone()),
                phase: session::CardDecisionPhase::Confirmed,
                data: Some(serde_json::json!({
                    "summary": outcome.summary,
                    "result": outcome.data,
                })),
            })),
            Err(reason) => Ok(Some(failed_decision(proposal, option, &reason))),
        }
    }

    async fn card_events(&self) -> Result<Vec<session::Event>, ServiceError> {
        let guard = self.inner.lock().await;
        let Some(host) = guard.as_ref() else {
            return Err(ServiceError::RootUnavailable(
                self.state.lock().expect("root state 锁被毒化").clone(),
            ));
        };
        Ok(host.agent().session_events_from(0))
    }

    async fn record_decision(&self, decision: &session::CardDecision) -> Result<(), ServiceError> {
        let guard = self.inner.lock().await;
        let Some(host) = guard.as_ref() else {
            return Err(ServiceError::RootUnavailable(
                self.state.lock().expect("root state 锁被毒化").clone(),
            ));
        };
        host.agent().record_card_decision(decision).await?;
        Ok(())
    }
}

fn failed_decision(
    proposal: &Proposal,
    option: &ProposeOption,
    reason: &str,
) -> session::CardDecision {
    session::CardDecision {
        call_id: proposal.call_id.clone(),
        option: Some(option.id.clone()),
        action: Some(option.action.clone()),
        phase: session::CardDecisionPhase::Failed,
        data: Some(serde_json::json!({ "summary": reason })),
    }
}
