//! T055：工作页 /「现在」共用的一份工作快照（纯派生 + 组装）。
//!
//! 只读事实：任务库 + Kind 根会话里还没点的确认卡 + Kind 待答提问。页面拿同一份，
//! 刷新/重开一样；这里不写任务库、不发通知。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::protocol::WorkResponse;
use tasks::{EffectKind, ReportState, Stage, Store, Task};

const KIND_RESUME: &str = "resume";
const KIND_CONFIRM: &str = "confirm";
const KIND_ANSWER: &str = "answer";
const KIND_JUDGE: &str = "judge";
const KIND_ACCEPT: &str = "accept";

fn kind_priority(kind: &str) -> u8 {
    match kind {
        KIND_RESUME => 0,
        KIND_CONFIRM => 1,
        KIND_ANSWER => 2,
        KIND_JUDGE => 3,
        KIND_ACCEPT => 4,
        _ => 5,
    }
}

fn need_text(kind: &str, title: &str) -> String {
    match kind {
        KIND_RESUME => "确认接着做被打断的这一轮".to_owned(),
        KIND_CONFIRM if title == "Kind 问你" => "回答 Kind 的问题".to_owned(),
        KIND_CONFIRM => "确认这张卡上的做法".to_owned(),
        KIND_ANSWER if title == "Kind 问你" => "回答 Kind 的问题".to_owned(),
        KIND_ANSWER => "回答任务里的问题".to_owned(),
        KIND_JUDGE => "判这份核查结果".to_owned(),
        KIND_ACCEPT => "接受这份结果".to_owned(),
        _ => "看一眼".to_owned(),
    }
}

/// T055：一张任务卡的事实（页面只读这些；不发会话内容和工具细节）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskFact {
    pub id: String,
    pub title: String,
    /// 阶段 key（needs_conditions / pending / running / ready_for_review / requirement_met / closed）。
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// 当前要求版本的写入范围；相对任务 cwd 显示；None = 只读调查任务。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_scope: Option<String>,
    pub version: u32,
    pub attempts: u64,
    pub created_at: i64,
    /// 最近一次 Attempt 还没结束，或 launch 还在排队。
    pub running: bool,
    /// 这是里程碑门任务。
    pub gate: bool,
    /// 被这个门把关的任务（门任务上有）；或者把关这个任务的门（被把关任务上有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_of: Option<String>,
    /// 排队要等的前置任务 id。
    pub after: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<WorkLatest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<i64>,
}

/// 任务最近一份报告（页面判断「等你 / 完成」用；正文只放摘要和问题原文）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkLatest {
    pub report_id: String,
    /// ready / blocked / failed / waiting。
    pub state: String,
    pub at: i64,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

/// 组接受/单项接受时带着「勾选那一刻」看到的那份报告和版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkPick {
    pub task_id: String,
    pub report_id: String,
    pub version: u32,
}

/// T055：「等你」里的一项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WaitItem {
    /// resume / confirm / answer / judge / accept。
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub title: String,
    /// 原阶段 key；不绑任务的项没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// 程序拼的一句「要你做什么」。
    pub need: String,
    /// 模型写的摘要或问题原文。
    pub body: String,
    pub at: i64,
    /// Kind 对话里那一轮的号（「去看/去判/去答」滚过去用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    /// 同一任务排在后面的其他种类。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<WorkPick>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
}

struct FactMeta<'a> {
    task: &'a Task,
    stage: Stage,
    gate_of: Option<String>,
    running: bool,
    latest: Option<&'a tasks::Report>,
}

fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::NeedsConditions => "needs_conditions",
        Stage::Pending => "pending",
        Stage::Running => "running",
        Stage::ReadyForReview => "ready_for_review",
        Stage::RequirementMet => "requirement_met",
        Stage::Closed => "closed",
    }
}

fn report_state_key(state: ReportState) -> &'static str {
    match state {
        ReportState::Ready => "ready",
        ReportState::Blocked => "blocked",
        ReportState::Failed => "failed",
        ReportState::Waiting => "waiting",
    }
}

/// 根会话里那条 relay 所在轮次（task id 和可选报告 id 都要对上）；同一任务取最新一条。
fn relay_turn(events: &[session::Event], task_id: &str, report_id: Option<&str>) -> Option<u64> {
    let mut found = None;
    for event in events {
        let session::EventKind::UserMessage { turn, message, .. } = &event.kind else {
            continue;
        };
        let model::MessageSource::User {
            rpc_id: Some(rpc_id),
        } = &message.source
        else {
            continue;
        };
        let rest = rpc_id
            .strip_prefix("task-report:")
            .or_else(|| rpc_id.strip_prefix("task-stop:"));
        let Some(rest) = rest else {
            continue;
        };
        let Some((relay_task, relay_report)) = rest.split_once(':') else {
            continue;
        };
        if relay_task != task_id {
            continue;
        }
        if report_id.is_some_and(|report_id| relay_report != report_id) {
            continue;
        }
        found = Some(*turn);
    }
    found
}

fn event_time(events: &[session::Event], seq: u64) -> i64 {
    events
        .iter()
        .find(|event| event.seq == seq)
        .map(|event| i64::try_from(event.time).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn event_turn(events: &[session::Event], seq: u64) -> Option<u64> {
    events
        .iter()
        .find(|event| event.seq == seq)
        .and_then(|event| match &event.kind {
            session::EventKind::TurnStart { turn }
            | session::EventKind::TurnEnd { turn, .. }
            | session::EventKind::StepStart { turn, .. }
            | session::EventKind::StepEnd { turn, .. }
            | session::EventKind::SystemMessage { turn, .. }
            | session::EventKind::UserMessage { turn, .. }
            | session::EventKind::QuestionAsked { turn, .. }
            | session::EventKind::QuestionAnswered { turn, .. }
            | session::EventKind::QuestionCancelled { turn, .. }
            | session::EventKind::AssistantMessage { turn, .. }
            | session::EventKind::ToolCall { turn, .. }
            | session::EventKind::ToolResult { turn, .. } => Some(*turn),
            _ => None,
        })
}

fn wait_item(meta: &FactMeta<'_>, kind: &str, events: &[session::Event]) -> WaitItem {
    let latest = meta.latest;
    let title = meta.task.title.clone();
    let body = match (kind, latest) {
        (KIND_RESUME, Some(report)) => report
            .question
            .clone()
            .unwrap_or_else(|| report.summary.clone()),
        (KIND_ANSWER, Some(report)) => report
            .question
            .clone()
            .unwrap_or_else(|| report.summary.clone()),
        (_, Some(report)) => report.summary.clone(),
        (_, None) => "上一轮被打断了，没有新结果".to_owned(),
    };
    let at = match kind {
        KIND_RESUME => meta
            .task
            .latest_attempt()
            .and_then(|attempt| attempt.ended_at)
            .or_else(|| latest.map(|report| report.at))
            .unwrap_or_default(),
        _ => latest.map(|report| report.at).unwrap_or_default(),
    };
    let report_id = latest.map(|report| report.id.as_str());
    let turn = relay_turn(events, meta.task.id.as_str(), report_id);
    let (pick, write, version) = if kind == KIND_ACCEPT {
        let pick = latest.map(|report| WorkPick {
            task_id: meta.task.id.as_str().to_owned(),
            report_id: report.id.as_str().to_owned(),
            version: report.version,
        });
        (
            pick,
            crate::result_cards::write_scope_display(meta.task),
            Some(meta.task.requirement_version()),
        )
    } else {
        (None, None, None)
    };
    WaitItem {
        kind: kind.to_owned(),
        task_id: Some(meta.task.id.as_str().to_owned()),
        title,
        stage: Some(stage_key(meta.stage).to_owned()),
        need: need_text(kind, ""),
        body,
        at,
        turn,
        also: Vec::new(),
        pick,
        write,
        version,
    }
}

fn task_wait_kinds(meta: &FactMeta<'_>, gate_passed: bool) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if meta.task.is_interrupted() {
        kinds.push(KIND_RESUME);
    }
    if meta.stage == Stage::NeedsConditions
        && let Some(report) = meta.latest
    {
        let state = report.state;
        if matches!(state, ReportState::Blocked | ReportState::Failed) || report.question.is_some()
        {
            kinds.push(KIND_ANSWER);
        }
    }
    if meta.task.is_gate()
        && let Some(report) = meta.latest
        && report.state == ReportState::Ready
        && meta.task.verdict_for_gate_report(&report.id).is_none()
    {
        kinds.push(KIND_JUDGE);
    }
    if !meta.task.is_gate()
        && !meta.running
        && gate_passed
        && let Some(report) = meta.latest
        && report.state == ReportState::Ready
        && report.version == meta.task.requirement_version()
        && meta.task.acceptance_for(&report.id).is_none()
    {
        kinds.push(KIND_ACCEPT);
    }
    kinds
}

/// 组装一份工作快照。
pub(crate) fn snapshot(
    store: &Store,
    events: &[session::Event],
    pending_questions: &[agent::Question],
    now: i64,
) -> Result<WorkResponse, tasks::TaskError> {
    let all = store.all_tasks()?;
    let by_id: HashMap<String, Task> = all
        .iter()
        .map(|task| (task.id.as_str().to_owned(), task.clone()))
        .collect();
    let mut facts = Vec::new();
    let mut waits: HashMap<String, Vec<WaitItem>> = HashMap::new();
    let mut stage_keys: HashMap<String, String> = HashMap::new();
    for task in all.iter().filter(|task| !task.is_graph_root()) {
        let stage = store.stage(&task.id)?;
        stage_keys.insert(task.id.as_str().to_owned(), stage_key(stage).to_owned());
        let closed_at = store.closed_at(&task.id)?;
        let gate = task.is_gate();
        let gate_of = if gate {
            task.needs.first().map(|need| need.from.as_str().to_owned())
        } else {
            store
                .guarding_gate(&task.id)?
                .map(|guarding| guarding.id.as_str().to_owned())
        };
        let running = task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none())
            || store.effect_pending(EffectKind::Launch, task.id.as_str())?;
        let accepted_at = task
            .acceptances
            .iter()
            .rev()
            .find(|acceptance| acceptance.version == task.requirement_version())
            .map(|acceptance| acceptance.at);
        let latest = task.latest_report();
        let meta = FactMeta {
            task,
            stage,
            gate_of: gate_of.clone(),
            running,
            latest,
        };
        // 被把关任务要看门有没有对这份报告判过通过。
        let gate_passed = match meta.gate_of.as_deref() {
            Some(gate_id) => by_id
                .get(gate_id)
                .is_some_and(|gate_task| latest.is_some_and(|report| gate_task.passed(&report.id))),
            None => true,
        };
        let kinds = task_wait_kinds(&meta, gate_passed);
        let task_waits = waits.entry(task.id.as_str().to_owned()).or_default();
        for kind in kinds {
            task_waits.push(wait_item(&meta, kind, events));
        }
        facts.push(TaskFact {
            id: task.id.as_str().to_owned(),
            title: task.title.clone(),
            stage: stage_key(stage).to_owned(),
            parent: task
                .parent
                .as_ref()
                .map(|parent| parent.as_str().to_owned()),
            write_scope: crate::result_cards::write_scope_display(task),
            version: task.requirement_version(),
            attempts: task.runs.iter().map(|run| run.attempts.len() as u64).sum(),
            created_at: task.created_at,
            running,
            gate,
            gate_of: meta.gate_of,
            after: task
                .needs
                .iter()
                .map(|need| need.from.as_str().to_owned())
                .collect(),
            latest: latest.map(|report| WorkLatest {
                report_id: report.id.as_str().to_owned(),
                state: report_state_key(report.state).to_owned(),
                at: report.at,
                summary: report.summary.clone(),
                question: report.question.clone(),
            }),
            accepted_at,
            closed_at,
        });
    }

    // 确认卡：还没点、没被替换的 proposal；绑了任务的归任务，没绑的单列。
    for proposal in crate::cards::proposals_of(events) {
        if crate::cards::decision_of(events, &proposal.call_id).is_some()
            || crate::cards::is_replaced(events, &proposal.call_id)
        {
            continue;
        }
        let task_id = proposal
            .args
            .options
            .iter()
            .find_map(|option| option.details.get("task_id").and_then(|id| id.as_str()))
            .map(str::to_owned);
        let at = event_time(events, proposal.seq);
        let turn = event_turn(events, proposal.seq);
        let body = proposal.meta.brief.clone();
        if let Some(task_id) = task_id.as_deref()
            && by_id.contains_key(task_id)
            && !by_id[task_id].is_graph_root()
        {
            let stage = stage_keys.get(task_id).cloned().unwrap_or_default();
            let item = WaitItem {
                kind: KIND_CONFIRM.to_owned(),
                task_id: Some(task_id.to_owned()),
                title: by_id[task_id].title.clone(),
                stage: Some(stage),
                need: need_text(KIND_CONFIRM, &by_id[task_id].title),
                body,
                at,
                turn,
                also: Vec::new(),
                pick: None,
                write: crate::result_cards::write_scope_display(&by_id[task_id]),
                version: None,
            };
            waits.entry(task_id.to_owned()).or_default().push(item);
        } else {
            waits.entry(String::new()).or_default().push(WaitItem {
                kind: KIND_CONFIRM.to_owned(),
                task_id: task_id.clone(),
                title: proposal.meta.title.clone(),
                stage: None,
                need: need_text(KIND_CONFIRM, &proposal.meta.title),
                body,
                at,
                turn,
                also: Vec::new(),
                pick: None,
                write: None,
                version: None,
            });
        }
    }

    // Kind 的待答提问：全部合成一项（不按问题逐条列）。
    if !pending_questions.is_empty() {
        let ids: HashSet<&str> = pending_questions
            .iter()
            .map(|question| question.id.as_str())
            .collect();
        let asked = events.iter().find_map(|event| {
            let session::EventKind::QuestionAsked {
                turn,
                question_id,
                question,
                ..
            } = &event.kind
            else {
                return None;
            };
            ids.contains(question_id.as_str())
                .then(|| (*turn, event.time, question.clone()))
        });
        let body = pending_questions
            .iter()
            .map(|question| question.question.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let at = asked
            .as_ref()
            .map(|(_, time, _)| i64::try_from(*time).unwrap_or(i64::MAX))
            .unwrap_or(now);
        let turn = asked.as_ref().map(|(turn, _, _)| *turn);
        waits.entry(String::new()).or_default().push(WaitItem {
            kind: KIND_ANSWER.to_owned(),
            task_id: None,
            title: "Kind 问你".to_owned(),
            stage: None,
            need: need_text(KIND_ANSWER, "Kind 问你"),
            body,
            at,
            turn,
            also: Vec::new(),
            pick: None,
            write: None,
            version: None,
        });
    }

    let mut wait = Vec::new();
    for (_, mut candidates) in waits {
        if candidates.is_empty() {
            continue;
        }
        candidates.sort_by(|left, right| {
            kind_priority(&left.kind)
                .cmp(&kind_priority(&right.kind))
                .then_with(|| right.at.cmp(&left.at))
        });
        let mut primary = candidates.remove(0);
        let mut also: Vec<String> = Vec::new();
        for candidate in candidates {
            if candidate.kind != primary.kind && !also.contains(&candidate.kind) {
                also.push(candidate.kind);
            }
        }
        primary.also = also;
        wait.push(primary);
    }
    wait.sort_by(|left, right| {
        kind_priority(&left.kind)
            .cmp(&kind_priority(&right.kind))
            .then_with(|| right.at.cmp(&left.at))
            .then_with(|| left.title.cmp(&right.title))
    });
    Ok(WorkResponse {
        generated_at: now,
        tasks: facts,
        wait,
    })
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

impl crate::root_lifecycle::RootHost {
    /// T055：读一份工作快照。根会话不可用时仍返回任务事实（确认卡/提问为空）。
    pub(crate) async fn work_snapshot(&self) -> Result<WorkResponse, crate::error::ServiceError> {
        let Some(runtime) = self.task_runtime() else {
            return Err(crate::error::ServiceError::TaskUnavailable(
                "任务库没开，工作快照读不了".to_owned(),
            ));
        };
        let (events, pending) = {
            let guard = self.inner.lock().await;
            match guard.as_ref() {
                Some(host) => (
                    host.agent().session_events_from(0),
                    host.agent().pending_questions(),
                ),
                None => (Vec::new(), Vec::new()),
            }
        };
        snapshot(runtime.tasks_store(), &events, &pending, now_millis())
            .map_err(|error| crate::error::ServiceError::TaskUnavailable(error.to_string()))
    }
}

#[cfg(test)]
#[path = "work_tests.rs"]
mod work_tests;
