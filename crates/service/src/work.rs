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

/// 任务等待项的一句话：照原型 waitModel.ts 按事实拼（写/调查、第几版、失败/卡住）。
fn need_text(kind: &str, meta: &FactMeta<'_>) -> String {
    let latest = meta.latest;
    match kind {
        KIND_RESUME => {
            if latest.is_some() {
                "被打断了，待 Kind 核实".to_owned()
            } else {
                "AIOS 重启时它在跑，没交结果；Kind 问你接不接着做".to_owned()
            }
        }
        KIND_ANSWER => match latest.map(|report| report.state) {
            Some(ReportState::Failed) => "失败了，要你定怎么办".to_owned(),
            _ => "卡住了，它问你".to_owned(),
        },
        KIND_JUDGE => match (latest, meta.guarded_title.as_deref()) {
            (Some(report), Some(title)) => format!(
                "核查「{title}」第 {} 次交的结果，你判通过还是需修正",
                report.attempt_id.0
            ),
            _ => "判这份核查结果".to_owned(),
        },
        KIND_ACCEPT => {
            let version = latest.map(|report| report.version).unwrap_or(1);
            let what = if crate::result_cards::write_scope_display(meta.task).is_some() {
                "写任务交了改动"
            } else {
                "调查任务交了结果"
            };
            let version = if version > 1 {
                format!("（第 {version} 版要求）")
            } else {
                String::new()
            };
            format!("{what}{version}，你接受就关闭")
        }
        _ => "看一眼".to_owned(),
    }
}

/// 没绑任务事实的确认卡（比如换模型）。
fn confirm_need(title: &str) -> String {
    if title == "Kind 问你" {
        "回答 Kind 的问题".to_owned()
    } else {
        "确认这张卡上的做法".to_owned()
    }
}

/// 已确认、没取消的任务图：服务算好线，页面只排和画。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkGraphFact {
    /// 总任务 id。
    pub id: String,
    /// 总任务标题。
    pub title: String,
    pub edges: Vec<WorkEdgeFact>,
}

/// 图里的一条线；从上游指向下游（或门指向下游）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkEdgeFact {
    pub from: String,
    pub to: String,
    /// needs（下游等上游）/ guards（被把关任务 → 门）/ release（门 → 下游）。
    pub kind: String,
    /// handed / waiting / judge / fix。
    pub state: String,
    /// 这条线连着的是不是程序门。
    pub program: bool,
    /// T066：flow / make / back / judge / 无。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<String>,
    /// T066：线小字（第 N 份 · X 文件 / 打回第 N 份）；没有就空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
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
    /// T066：正在跑的那个 Attempt 的开始时间；没在跑为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_since: Option<i64>,
    /// 这是里程碑门任务。
    pub gate: bool,
    /// T057：这是程序门（有命令）。
    #[serde(default)]
    pub program: bool,
    /// T057：程序门命令条数。
    #[serde(default, rename = "programCommands")]
    pub program_commands: usize,
    /// 被这个门把关的任务（门任务上有）；或者把关这个任务的门（被把关任务上有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_of: Option<String>,
    /// 排队要等的前置任务 id。
    pub after: Vec<String>,
    /// T055 审计：这是「等前置」的排队（上游还没可放行、也没有 blocked/failed/提问/被打断），
    /// 不是「出了问题」；「现在」的问题组按它排除。
    pub after_waiting: bool,
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
    /// T066：这份报告那个 Attempt 改了几个文件；旧报告为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_count: Option<u32>,
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
    /// 循环约定暂停的「等你」项带约定 id；其他项没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appointment_id: Option<String>,
}

struct FactMeta<'a> {
    task: &'a Task,
    stage: Stage,
    gate_of: Option<String>,
    /// 门任务把关的那个任务的标题（判卡一句话用）。
    guarded_title: Option<String>,
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
        need: need_text(kind, meta),
        body,
        at,
        turn,
        also: Vec::new(),
        pick,
        write,
        version,
        appointment_id: None,
    }
}

/// T055 审计：「等前置」的排队不算出了问题——有没解析的上游、且没有 blocked/failed/提问/
/// 被打断/被停止这些「Kind 在补或卡住」的事实。
fn after_waiting(task: &Task, stage: Stage, latest: Option<&tasks::Report>) -> bool {
    if stage != Stage::NeedsConditions || task.needs.is_empty() {
        return false;
    }
    let upstream_unready = task.needs.iter().any(|need| need.handoff.is_none());
    if !upstream_unready {
        return false;
    }
    if task.latest_attempt().is_some_and(|attempt| {
        matches!(
            attempt.end,
            Some(tasks::AttemptEnd::Interrupted | tasks::AttemptEnd::Stopped)
        )
    }) {
        return false;
    }
    !latest.is_some_and(|report| {
        matches!(report.state, ReportState::Blocked | ReportState::Failed)
            || report.question.is_some()
    })
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
        && !meta.task.is_program_gate()
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

/// 一张已确认图里从某任务到下游的线状态：gate 是该下游对应的门（没有门时 None）。
fn release_state(gate: &Task, guarded: &Task, downstream: &Task) -> &'static str {
    let judge = !gate.is_program_gate()
        && gate
            .latest_report()
            .is_some_and(|report| report.state == ReportState::Ready)
        && gate
            .latest_report()
            .is_some_and(|report| gate.verdict_for_gate_report(&report.id).is_none());
    if judge {
        return "judge";
    }
    let Some(report) = guarded.latest_report() else {
        return "waiting";
    };
    let resolved = downstream
        .needs
        .iter()
        .find(|need| need.from == guarded.id)
        .is_some_and(|need| need.handoff.is_some());
    if gate.needs_fix(&report.id).is_some() {
        "fix"
    } else if gate.passed(&report.id) && resolved {
        "handed"
    } else {
        "waiting"
    }
}

/// 已确认、没取消的图：服务从任务事实算线；页面只排和画。
fn edge_runtime(
    edge: &WorkEdgeFact,
    by_id: &HashMap<String, Task>,
) -> (Option<String>, Option<String>) {
    let from = by_id.get(&edge.from);
    let to = by_id.get(&edge.to);
    let report_payload = |task: &Task| {
        let report = task.latest_report()?;
        let count = report.changed_count?;
        let index = task
            .reports
            .iter()
            .filter(|candidate| candidate.state == tasks::ReportState::Ready)
            .count();
        Some(format!("第 {index} 份 · {count} 文件"))
    };
    if edge.state == "fix" {
        let index = from
            .map(|task| {
                task.reports
                    .iter()
                    .filter(|report| report.state == tasks::ReportState::Ready)
                    .count()
            })
            .unwrap_or(0);
        return (Some("back".to_owned()), Some(format!("打回第 {index} 份")));
    }
    if edge.state == "judge" {
        return (Some("judge".to_owned()), None);
    }
    if edge.kind == "guards"
        && edge.state == "handed"
        && to.is_some_and(|task| {
            task.is_gate() && task.latest_report().is_some() && task.verdicts.is_empty()
        })
    {
        return (Some("judge".to_owned()), None);
    }
    if to.is_some_and(tasks::Task::has_attempt) {
        return (Some("flow".to_owned()), from.and_then(report_payload));
    }
    if from.is_some_and(tasks::Task::has_attempt) {
        return (Some("make".to_owned()), None);
    }
    (None, None)
}

fn project_graphs(all: &[Task], by_id: &HashMap<String, Task>) -> Vec<WorkGraphFact> {
    let mut graphs = Vec::new();
    for root in all.iter().filter(|task| {
        task.is_graph_root()
            && task.arrangement.as_ref().is_some_and(|arrangement| {
                arrangement.confirmed.is_some() && arrangement.cancelled.is_none()
            })
    }) {
        let mut children: Vec<&Task> = all
            .iter()
            .filter(|task| {
                task.parent.as_ref().map(tasks::TaskId::as_str) == Some(root.id.as_str())
            })
            .collect();
        children.sort_by_key(|task| task.created_at);
        // 被门把关的上游只画 guards/release：Gate 子任务的 need 和下游等它的 need 都不再画 needs。
        let mut gate_of: HashMap<&str, &Task> = HashMap::new();
        for gate in &children {
            if gate.is_gate()
                && let Some(need) = gate.needs.first()
            {
                gate_of.insert(need.from.as_str(), *gate);
            }
        }
        let mut edges = Vec::new();
        for child in &children {
            if !child.is_gate() {
                for need in &child.needs {
                    if !by_id.contains_key(need.from.as_str())
                        || gate_of.contains_key(need.from.as_str())
                    {
                        continue;
                    }
                    edges.push(WorkEdgeFact {
                        from: need.from.as_str().to_owned(),
                        to: child.id.as_str().to_owned(),
                        kind: "needs".to_owned(),
                        state: if need.handoff.is_some() {
                            "handed".to_owned()
                        } else {
                            "waiting".to_owned()
                        },
                        program: false,
                        live: None,
                        payload: None,
                    });
                }
            }
            if !child.is_gate() {
                continue;
            }
            let Some(guarded_id) = child.needs.first().map(|need| need.from.clone()) else {
                continue;
            };
            let Some(guarded) = by_id.get(guarded_id.as_str()) else {
                continue;
            };
            edges.push(WorkEdgeFact {
                from: guarded.id.as_str().to_owned(),
                to: child.id.as_str().to_owned(),
                kind: "guards".to_owned(),
                state: if child.latest_attempt().is_some() {
                    "handed".to_owned()
                } else {
                    "waiting".to_owned()
                },
                program: child.is_program_gate(),
                live: None,
                payload: None,
            });
            for downstream in &children {
                if downstream.is_gate()
                    || !downstream.needs.iter().any(|need| need.from == guarded_id)
                {
                    continue;
                }
                edges.push(WorkEdgeFact {
                    from: child.id.as_str().to_owned(),
                    to: downstream.id.as_str().to_owned(),
                    kind: "release".to_owned(),
                    state: release_state(child, guarded, downstream).to_owned(),
                    program: child.is_program_gate(),
                    live: None,
                    payload: None,
                });
            }
        }
        for edge in &mut edges {
            let (live, payload) = edge_runtime(edge, by_id);
            edge.live = live;
            edge.payload = payload;
        }
        edges.sort_by(|left, right| {
            (left.from.as_str(), left.to.as_str(), left.kind.as_str()).cmp(&(
                right.from.as_str(),
                right.to.as_str(),
                right.kind.as_str(),
            ))
        });
        graphs.push(WorkGraphFact {
            id: root.id.as_str().to_owned(),
            title: root.title.clone(),
            edges,
        });
    }
    graphs.sort_by(|left, right| left.id.cmp(&right.id));
    graphs
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
    // T055 审计：工作页/现在只上「已确认图的子任务 + 单派任务」；未确认图和取消图的子任务
    // 还没开跑（或已作废），不上板。总任务也不上。
    let visible: Vec<&Task> = all
        .iter()
        .filter(|task| {
            // T066：约定到点的触发任务只在日历上，不进看板/图/等你/现在。
            if task.origin.as_ref().map(|origin| origin.role)
                == Some(tasks::AppointmentRole::Trigger)
            {
                return false;
            }
            if task.is_graph_root() {
                return false;
            }
            match task.parent.as_ref() {
                None => true,
                Some(parent) => by_id.get(parent.as_str()).is_some_and(|root| {
                    root.arrangement.as_ref().is_some_and(|arrangement| {
                        arrangement.confirmed.is_some() && arrangement.cancelled.is_none()
                    })
                }),
            }
        })
        .collect();
    for task in visible {
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
        let guarded_title = gate_of
            .as_deref()
            .and_then(|gate_id| by_id.get(gate_id))
            .map(|guarded| guarded.title.clone());
        let meta = FactMeta {
            task,
            stage,
            gate_of: gate_of.clone(),
            guarded_title,
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
        let task_waits = waits.entry(format!("task:{}", task.id)).or_default();
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
            running_since: task
                .latest_attempt()
                .filter(|attempt| attempt.ended_at.is_none())
                .map(|attempt| attempt.started_at),
            gate,
            program: task.is_program_gate(),
            program_commands: task.gate_commands().len(),
            gate_of: meta.gate_of,
            after: task
                .needs
                .iter()
                .map(|need| need.from.as_str().to_owned())
                .collect(),
            after_waiting: after_waiting(task, stage, latest),
            latest: latest.map(|report| WorkLatest {
                report_id: report.id.as_str().to_owned(),
                state: report_state_key(report.state).to_owned(),
                at: report.at,
                summary: report.summary.clone(),
                question: report.question.clone(),
                changed_count: report.changed_count,
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
            && let Some(task) = by_id.get(task_id)
        {
            // 绑任务的确认卡归同一个任务桶（绑总任务也显示，但总任务不上板）。
            let stage = stage_keys.get(task_id).cloned();
            let item = WaitItem {
                kind: KIND_CONFIRM.to_owned(),
                task_id: Some(task_id.to_owned()),
                title: task.title.clone(),
                stage,
                need: confirm_need(&task.title),
                body,
                at,
                turn,
                also: Vec::new(),
                pick: None,
                write: crate::result_cards::write_scope_display(task),
                version: None,
                appointment_id: None,
            };
            waits
                .entry(format!("task:{task_id}"))
                .or_default()
                .push(item);
        } else {
            // 没绑任务的确认卡每张一个桶：后端是「一张卡一项」，不能互相顶掉。
            waits
                .entry(format!("confirm:{}", proposal.call_id))
                .or_default()
                .push(WaitItem {
                    kind: KIND_CONFIRM.to_owned(),
                    task_id: None,
                    title: proposal.meta.title.clone(),
                    stage: None,
                    need: confirm_need(&proposal.meta.title),
                    body,
                    at,
                    turn,
                    also: Vec::new(),
                    pick: None,
                    write: None,
                    version: None,
                    appointment_id: None,
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
        waits
            .entry("kind:questions".to_owned())
            .or_default()
            .push(WaitItem {
                kind: KIND_ANSWER.to_owned(),
                task_id: None,
                title: "Kind 问你".to_owned(),
                stage: None,
                need: "回答 Kind 的问题".to_owned(),
                body,
                at,
                turn,
                also: Vec::new(),
                pick: None,
                write: None,
                version: None,
                appointment_id: None,
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
        graphs: project_graphs(&all, &by_id),
    })
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// 把日历等额外的「等你」项并进工作快照，按同一种排序。
pub(crate) fn merge_waits(response: &mut WorkResponse, mut extra: Vec<WaitItem>) {
    if extra.is_empty() {
        return;
    }
    response.wait.append(&mut extra);
    response.wait.sort_by(|left, right| {
        kind_priority(&left.kind)
            .cmp(&kind_priority(&right.kind))
            .then_with(|| right.at.cmp(&left.at))
            .then_with(|| left.title.cmp(&right.title))
    });
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
        let mut response = snapshot(runtime.tasks_store(), &events, &pending, now_millis())
            .map_err(|error| crate::error::ServiceError::TaskUnavailable(error.to_string()))?;
        if let Ok(waits) = runtime.calendar_pause_waits() {
            merge_waits(&mut response, waits);
        }
        Ok(response)
    }

    /// T055.2/T056.1：把勾选的一组结果交给任务库一个事务；逐项回执照 store 的原话。
    /// 每记下一项，就给那份报告的 relay 卡补一条 card/decision（Kind 卡上显示「你接受了」）。
    pub(crate) async fn accept_many(
        &self,
        request: &crate::protocol::TaskAcceptManyRequest,
    ) -> Result<crate::protocol::TaskAcceptManyResponse, crate::error::ServiceError> {
        let Some(runtime) = self.task_runtime() else {
            return Err(crate::error::ServiceError::TaskUnavailable(
                "任务库没开，接受不了结果".to_owned(),
            ));
        };
        let picks: Vec<(tasks::TaskId, tasks::ReportId)> = request
            .picks
            .iter()
            .map(|pick| {
                (
                    tasks::TaskId::from(pick.task_id.as_str()),
                    tasks::ReportId::from(pick.report_id.as_str()),
                )
            })
            .collect();
        let batch = runtime
            .tasks_store()
            .accept_many(&picks, now_millis())
            .map_err(|error| crate::error::ServiceError::TaskUnavailable(error.to_string()))?;
        let accepted_at = batch.accepted_at;
        let mut outcomes = Vec::with_capacity(batch.outcomes.len());
        for (outcome, pick) in batch.outcomes.into_iter().zip(request.picks.iter()) {
            if outcome.refused.is_none() {
                let decision = session::CardDecision {
                    call_id: format!(
                        "task-report:{}:{}",
                        outcome.task_id.as_str(),
                        pick.report_id
                    ),
                    option: None,
                    action: Some("accept_task_result".to_owned()),
                    phase: session::CardDecisionPhase::Confirmed,
                    data: Some(serde_json::json!({
                        "summary": "你接受了",
                        "version": pick.version,
                        "report_id": pick.report_id,
                        "at": accepted_at / 1000,
                    })),
                };
                self.record_decision(&decision).await?;
            }
            outcomes.push(crate::protocol::TaskAcceptManyOutcome {
                task_id: outcome.task_id.as_str().to_owned(),
                title: outcome.title,
                refused: outcome.refused,
            });
        }
        Ok(crate::protocol::TaskAcceptManyResponse {
            accepted_at,
            outcomes,
        })
    }
}

#[cfg(test)]
#[path = "work_tests.rs"]
mod work_tests;
