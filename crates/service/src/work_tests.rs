//! T055：工作快照纯派生（五种「等你」、多命中、运行中、确认卡、Kind 提问、关闭时间）。

use super::*;
use std::path::Path;

use model::{ContentBlock, Message, Role, UserMessage};
use session::{Event, EventKind};
use tasks::{
    AttemptEnd, ConversationId, ModelChoice, NewGraph, NewReport, NewSubtask, NewTask,
    ReportOrigin, ReportState, Store, Task, TaskPolicy,
};
use tempfile::TempDir;

const T0: i64 = 1_700_000_000_000;

fn open_store() -> (TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn draft(cwd: &Path, title: &str) -> NewTask {
    NewTask {
        title: title.to_owned(),
        requirement: format!("{title} 的要求"),
        dod: format!("{title} 的 DoD"),
        cwd: cwd.to_path_buf(),
        conversation: ConversationId::new("kind-work"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn policy(write_scope: Option<&str>) -> TaskPolicy {
    TaskPolicy {
        write_scope: write_scope.map(str::to_owned),
        worktree: false,
        bash: false,
    }
}

fn report(state: ReportState, summary: &str, question: Option<&str>) -> NewReport {
    NewReport {
        state,
        origin: ReportOrigin::Agent,
        summary: summary.to_owned(),
        question: question.map(str::to_owned),
    }
}

fn start(store: &Store, task: &Task, now: i64) -> tasks::AttemptId {
    let claim = store.claim_effect(EffectKind::Launch, now).unwrap();
    if let Some(claim) = claim {
        store.complete_effect(&claim).unwrap();
    }
    store.begin_attempt(&task.id, now).unwrap()
}

fn ready_task(store: &Store, cwd: &Path, title: &str, at: i64) -> Task {
    let task = store.create_task(draft(cwd, title), at).unwrap();
    let attempt = start(store, &task, at + 1);
    store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Ready, "查好了", None),
            at + 2,
        )
        .unwrap();
    store
        .end_attempt(&task.id, attempt, at + 3, AttemptEnd::Reported)
        .unwrap();
    store.task(&task.id).unwrap().unwrap()
}

fn blocked_task(store: &Store, cwd: &Path, title: &str, at: i64, question: &str) -> Task {
    let task = store.create_task(draft(cwd, title), at).unwrap();
    let attempt = start(store, &task, at + 1);
    store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Blocked, "等你定", Some(question)),
            at + 2,
        )
        .unwrap();
    store
        .end_attempt(&task.id, attempt, at + 3, AttemptEnd::Reported)
        .unwrap();
    store.task(&task.id).unwrap().unwrap()
}

fn interrupted_task(store: &Store, cwd: &Path, title: &str, at: i64) -> Task {
    let task = store.create_task(draft(cwd, title), at).unwrap();
    let attempt = start(store, &task, at + 1);
    store
        .end_attempt(&task.id, attempt, at + 2, AttemptEnd::Interrupted)
        .unwrap();
    store.task(&task.id).unwrap().unwrap()
}

fn milestone(cwd: &Path, title: &str, after: Vec<usize>) -> NewSubtask {
    NewSubtask {
        task: draft(cwd, title),
        policy: policy(None),
        after,
        milestone: true,
        program_gate: Vec::new(),
    }
}

fn gated_graph(cwd: &Path) -> NewGraph {
    NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            NewSubtask {
                task: draft(cwd, "先查行数"),
                policy: policy(None),
                after: vec![],
                milestone: false,
                program_gate: Vec::new(),
            },
            milestone(cwd, "独立核查", vec![0]),
            NewSubtask {
                task: draft(cwd, "再写结果"),
                policy: policy(None),
                after: vec![0],
                milestone: false,
                program_gate: Vec::new(),
            },
        ],
    }
}

fn program_gated_graph(cwd: &Path) -> NewGraph {
    NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            NewSubtask {
                task: draft(cwd, "先查行数"),
                policy: policy(None),
                after: vec![],
                milestone: false,
                program_gate: Vec::new(),
            },
            NewSubtask {
                task: draft(cwd, "自动核查"),
                policy: policy(None),
                after: vec![0],
                milestone: true,
                program_gate: vec![tasks::GateCommand {
                    command: "true".to_owned(),
                    timeout_ms: None,
                }],
            },
            NewSubtask {
                task: draft(cwd, "再写结果"),
                policy: policy(None),
                after: vec![0],
                milestone: false,
                program_gate: Vec::new(),
            },
        ],
    }
}

/// 一张被门把关的图：A（被把关）、G（门）、B（独立）、C（等 A 和 B）。
fn two_upstreams_gated_graph(cwd: &Path) -> NewGraph {
    NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            NewSubtask {
                task: draft(cwd, "查 A"),
                policy: policy(None),
                after: vec![],
                milestone: false,
                program_gate: Vec::new(),
            },
            milestone(cwd, "核查 A", vec![0]),
            NewSubtask {
                task: draft(cwd, "查 B"),
                policy: policy(None),
                after: vec![],
                milestone: false,
                program_gate: Vec::new(),
            },
            NewSubtask {
                task: draft(cwd, "写 C"),
                policy: policy(None),
                after: vec![0, 2],
                milestone: false,
                program_gate: Vec::new(),
            },
        ],
    }
}

fn edge<'a>(graph: &'a WorkGraphFact, from: &str, to: &str, kind: &str) -> &'a WorkEdgeFact {
    graph
        .edges
        .iter()
        .find(|edge| edge.from == from && edge.to == to && edge.kind == kind)
        .unwrap_or_else(|| {
            let all: Vec<String> = graph
                .edges
                .iter()
                .map(|edge| format!("{}->{}:{}", edge.from, edge.to, edge.kind))
                .collect();
            panic!("缺线 {kind} {from}->{to}；现有：{all:?}")
        })
}

fn gate_attempt_and_report(store: &Store, guarded: &Task, gate: &Task, at: i64) -> (Task, Task) {
    let attempt = start(store, guarded, at);
    store
        .record_report(
            &guarded.id,
            attempt,
            report(ReportState::Ready, "查好了", None),
            at + 1,
        )
        .unwrap();
    let guarded = store.task(&guarded.id).unwrap().unwrap();
    let guarded_report = guarded.latest_report().cloned().unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, at + 2)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    let gate_attempt = store
        .begin_attempt_with(
            &gate.id,
            at + 3,
            tasks::AttemptTrigger::GateRecheck {
                candidate: tasks::GateCandidate {
                    report_id: guarded_report.id.clone(),
                    requirement_version: guarded_report.version,
                },
            },
            vec![tasks::Handoff {
                report_id: guarded_report.id.clone(),
                requirement_version: guarded_report.version,
                summary: guarded_report.summary.clone(),
            }],
        )
        .unwrap();
    store
        .record_report(
            &gate.id,
            gate_attempt,
            report(ReportState::Ready, "核查完了", None),
            at + 4,
        )
        .unwrap();
    let gate = store.task(&gate.id).unwrap().unwrap();
    (guarded, gate)
}

fn relay(seq: u64, time: i64, turn: u64, task_id: &str, report_id: &str) -> Event {
    let message =
        UserMessage::text_with_request_id("relay", format!("task-report:{task_id}:{report_id}"))
            .into_message();
    Event::new(
        seq,
        u64::try_from(time).unwrap(),
        EventKind::UserMessage {
            turn,
            step: 1,
            message,
        },
    )
}

fn proposal(
    seq: u64,
    time: i64,
    turn: u64,
    call_id: &str,
    title: &str,
    brief: &str,
    task_id: Option<&str>,
) -> Vec<Event> {
    let details = match task_id {
        Some(task_id) => serde_json::json!({ "task_id": task_id }),
        None => serde_json::json!({}),
    };
    let args = serde_json::json!({
        "title": title,
        "brief": brief,
        "options": [{
            "id": "confirm",
            "action": "gate_judge",
            "label": "确认",
            "details": details,
        }]
    })
    .to_string();
    let card = serde_json::json!({
        "card": {
            "title": title,
            "brief": brief,
            "options": [{"id": "confirm", "label": "确认", "facts": []}]
        }
    });
    let mut result = Message::new(Role::Tool, vec![ContentBlock::text("card")]);
    result.tool_call_id = Some(call_id.to_owned());
    vec![
        Event::new(
            seq,
            u64::try_from(time).unwrap(),
            EventKind::ToolCall {
                turn,
                step: 1,
                call_id: call_id.to_owned(),
                name: "propose_action".to_owned(),
                arguments: args,
            },
        ),
        Event::new(
            seq + 1,
            u64::try_from(time + 1).unwrap(),
            EventKind::ToolResult {
                turn,
                step: 1,
                message: result,
                error: None,
                meta: Some(card),
            },
        ),
    ]
}

fn question(seq: u64, time: i64, turn: u64, question_id: &str, text: &str) -> Event {
    Event::new(
        seq,
        u64::try_from(time).unwrap(),
        EventKind::QuestionAsked {
            turn,
            step: 1,
            call_id: "call-ask".to_owned(),
            question_id: question_id.to_owned(),
            question: text.to_owned(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        },
    )
}

fn pending(text: &str, id: &str) -> agent::Question {
    agent::Question {
        id: id.to_owned(),
        question: text.to_owned(),
        header: None,
        options: Vec::new(),
        multi_select: false,
    }
}

#[test]
fn five_kinds_and_multi_hit_are_ordered() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let accept = ready_task(&store, cwd, "写份结果", T0);
    let accept_report = accept.latest_report().unwrap().id.clone();
    // 绑了确认卡的任务只显示最前面的那种；另建一个只有 ready 结果的任务看 accept。
    let accept_only = ready_task(&store, cwd, "写份结果 2", T0 + 5);
    let accept_only_report = accept_only.latest_report().unwrap().id.clone();
    let resume = interrupted_task(&store, cwd, "被打断", T0 + 10);
    let answer = blocked_task(&store, cwd, "等你答", T0 + 20, "要按哪条规则？");
    let answer_report = answer.latest_report().unwrap().id.clone();
    // 被打断的任务也有一份 blocked 报告：resume 优先，answer 进 also。
    let also_task = store.create_task(draft(cwd, "又断又卡"), T0 + 30).unwrap();
    let also_attempt = start(&store, &also_task, T0 + 31);
    store
        .record_report(
            &also_task.id,
            also_attempt,
            report(ReportState::Blocked, "卡住了", Some("选哪个")),
            T0 + 32,
        )
        .unwrap();
    store
        .end_attempt(
            &also_task.id,
            also_attempt,
            T0 + 33,
            AttemptEnd::Interrupted,
        )
        .unwrap();

    // 门图：被把关 ready + 门 ready 但还没判 → judge。
    let root = store.create_graph(gated_graph(cwd), T0 + 40).unwrap();
    store.confirm_graph(&root.id, T0 + 41).unwrap();
    let children = store.children(&root.id).unwrap();
    let (guarded, gate) = (children[0].clone(), children[1].clone());
    let g_attempt = start(&store, &guarded, T0 + 42);
    store
        .record_report(
            &guarded.id,
            g_attempt,
            report(ReportState::Ready, "查好了", None),
            T0 + 43,
        )
        .unwrap();
    let g_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let gate_claim = store.claim_effect(EffectKind::Launch, T0 + 44).unwrap();
    store.complete_effect(&gate_claim.unwrap()).unwrap();
    let gate_attempt = store
        .begin_attempt_with(
            &gate.id,
            T0 + 45,
            tasks::AttemptTrigger::GateRecheck {
                candidate: tasks::GateCandidate {
                    report_id: g_report.id.clone(),
                    requirement_version: g_report.version,
                },
            },
            vec![tasks::Handoff {
                report_id: g_report.id.clone(),
                requirement_version: g_report.version,
                summary: g_report.summary.clone(),
            }],
        )
        .unwrap();
    store
        .record_report(
            &gate.id,
            gate_attempt,
            report(ReportState::Ready, "核查完了", None),
            T0 + 46,
        )
        .unwrap();
    let gate_report = store
        .task(&gate.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();

    let mut events = vec![
        relay(100, T0 + 4, 2, accept.id.as_str(), accept_report.as_str()),
        relay(
            104,
            T0 + 8,
            9,
            accept_only.id.as_str(),
            accept_only_report.as_str(),
        ),
        relay(101, T0 + 14, 3, resume.id.as_str(), "none"),
        relay(102, T0 + 24, 4, answer.id.as_str(), answer_report.as_str()),
        relay(103, T0 + 47, 5, gate.id.as_str(), gate_report.id.as_str()),
    ];
    let mut proposed = proposal(
        110,
        T0 + 50,
        6,
        "call-bound",
        "改任务要求",
        "确认后同一个任务接着做。",
        Some(accept.id.as_str()),
    );
    events.append(&mut proposed);
    events.push(question(130, T0 + 70, 8, "q-1", "第一条问题"));
    events.push(question(131, T0 + 71, 8, "q-2", "第二条问题"));

    let response = snapshot(
        &store,
        &events,
        &[pending("第一条问题", "q-1"), pending("第二条问题", "q-2")],
        T0 + 100,
    )
    .unwrap();

    assert_eq!(response.generated_at, T0 + 100);
    assert!(
        response
            .tasks
            .iter()
            .all(|fact| fact.id != root.id.as_str()),
        "总任务不上看板"
    );
    let accept_fact = response
        .tasks
        .iter()
        .find(|fact| fact.id == accept.id.as_str())
        .unwrap();
    assert_eq!(accept_fact.stage, "ready_for_review");
    assert!(accept_fact.latest.is_some());
    let gate_fact = response
        .tasks
        .iter()
        .find(|fact| fact.id == gate.id.as_str())
        .unwrap();
    assert!(gate_fact.gate);
    assert_eq!(gate_fact.gate_of.as_deref(), Some(guarded.id.as_str()));

    let kinds: Vec<&str> = response
        .wait
        .iter()
        .map(|item| item.kind.as_str())
        .collect();
    assert_eq!(kinds.first(), Some(&"resume"));
    assert!(kinds.contains(&"confirm"));
    assert!(kinds.contains(&"answer"));
    assert!(kinds.contains(&"judge"));
    assert!(kinds.contains(&"accept"));
    // 多命中：resume 在前，answer 在 also。
    let multi = response
        .wait
        .iter()
        .find(|item| item.task_id.as_deref() == Some(also_task.id.as_str()))
        .expect("多命中任务");
    assert_eq!(multi.kind, "resume");
    assert!(
        multi.also.contains(&"answer".to_owned()),
        "{:?}",
        multi.also
    );

    let accept_item = response
        .wait
        .iter()
        .find(|item| item.kind == "accept")
        .expect("accept 项");
    assert_eq!(
        accept_item.task_id.as_deref(),
        Some(accept_only.id.as_str())
    );
    assert_eq!(accept_item.turn, Some(9));
    let pick = accept_item.pick.as_ref().expect("pick");
    assert_eq!(pick.report_id, accept_only_report.as_str());
    assert_eq!(pick.version, 1);

    let bound = response
        .wait
        .iter()
        .find(|item| item.kind == "confirm" && item.task_id.as_deref() == Some(accept.id.as_str()))
        .expect("绑任务的确认卡");
    assert_eq!(bound.title, "写份结果");
    assert_eq!(bound.turn, Some(6));
    let kind_answer = response
        .wait
        .iter()
        .find(|item| item.kind == "answer" && item.task_id.is_none())
        .expect("Kind 提问");
    assert_eq!(kind_answer.title, "Kind 问你");
    assert_eq!(kind_answer.turn, Some(8));
    assert!(kind_answer.body.contains("第一条问题"));
    assert!(kind_answer.body.contains("第二条问题"));
    let task_answer = response
        .wait
        .iter()
        .find(|item| item.kind == "answer" && item.task_id.as_deref() == Some(answer.id.as_str()))
        .expect("任务提问");
    assert!(task_answer.body.contains("要按哪条规则？"));

    let judge_item = response
        .wait
        .iter()
        .find(|item| item.kind == "judge")
        .expect("judge 项");
    assert_eq!(judge_item.task_id.as_deref(), Some(gate.id.as_str()));
    assert_eq!(judge_item.turn, Some(5));

    // 「等你」排序：五种顺序，同种新的在上。
    let priorities: Vec<u8> = response
        .wait
        .iter()
        .map(|item| kind_priority(&item.kind))
        .collect();
    let mut sorted = priorities.clone();
    sorted.sort();
    assert_eq!(priorities, sorted);
}

#[test]
fn a_running_task_is_not_waiting_for_accept() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let task = store.create_task(draft(cwd, "在跑"), T0).unwrap();
    let attempt = start(&store, &task, T0 + 1);
    store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Ready, "查好了", None),
            T0 + 2,
        )
        .unwrap();
    let response = snapshot(&store, &[], &[], T0 + 10).unwrap();
    let fact = response
        .tasks
        .iter()
        .find(|fact| fact.id == task.id.as_str())
        .unwrap();
    assert!(fact.running, "Attempt 还没结束");
    assert!(
        !response.wait.iter().any(|item| item.kind == "accept"),
        "在跑的不进结果等你接受"
    );
}

#[test]
fn a_decided_confirm_card_leaves_the_wait_list() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let task = ready_task(&store, cwd, "写份结果", T0);
    let mut events = proposal(
        100,
        T0 + 10,
        2,
        "call-1",
        "改任务要求",
        "确认后接着做。",
        Some(task.id.as_str()),
    );
    let result_before = snapshot(&store, &events, &[], T0 + 20).unwrap();
    assert!(result_before.wait.iter().any(|item| item.kind == "confirm"));

    let mut result = Message::new(Role::Tool, vec![ContentBlock::text("card")]);
    result.tool_call_id = Some("call-1".to_owned());
    events.push(Event::new(
        102,
        u64::try_from(T0 + 11).unwrap(),
        EventKind::CardDecision {
            call_id: "call-1".to_owned(),
            option: Some("confirm".to_owned()),
            action: Some("gate_judge".to_owned()),
            phase: session::CardDecisionPhase::Confirmed,
            data: None,
        },
    ));
    let result_after = snapshot(&store, &events, &[], T0 + 30).unwrap();
    assert!(
        !result_after.wait.iter().any(|item| item.kind == "confirm"),
        "点了就离开"
    );
}

#[test]
fn kind_questions_collapse_into_one_item_until_answered() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let _ = ready_task(&store, cwd, "写份结果", T0);
    let events = vec![
        question(100, T0 + 10, 2, "q-1", "第一条问题"),
        question(101, T0 + 11, 2, "q-2", "第二条问题"),
    ];
    let response = snapshot(
        &store,
        &events,
        &[pending("第一条问题", "q-1"), pending("第二条问题", "q-2")],
        T0 + 20,
    )
    .unwrap();
    let answers: Vec<&WaitItem> = response
        .wait
        .iter()
        .filter(|item| item.kind == "answer" && item.task_id.is_none())
        .collect();
    assert_eq!(answers.len(), 1, "全部合成一项");
    assert_eq!(answers[0].title, "Kind 问你");
    assert_eq!(answers[0].turn, Some(2));

    let cleared = snapshot(&store, &events, &[], T0 + 30).unwrap();
    assert!(
        !cleared
            .wait
            .iter()
            .any(|item| item.kind == "answer" && item.task_id.is_none()),
        "答了/关掉就离开"
    );
}

#[test]
fn an_unbound_confirm_card_is_listed_on_its_own() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let _ = ready_task(&store, cwd, "写份结果", T0);
    let events = proposal(
        120,
        T0 + 60,
        7,
        "call-model",
        "换模型",
        "换成 DeepSeek V4。",
        None,
    );
    let response = snapshot(&store, &events, &[], T0 + 70).unwrap();
    let unbound = response
        .wait
        .iter()
        .find(|item| item.kind == "confirm" && item.task_id.is_none())
        .expect("不绑任务的确认卡");
    assert_eq!(unbound.title, "换模型");
    assert_eq!(unbound.stage, None);
    assert!(unbound.body.contains("换成 DeepSeek V4"));
}

#[test]
fn facts_carry_closed_at_and_acceptance() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let task = ready_task(&store, cwd, "写份结果", T0);
    let report_id = task.latest_report().unwrap().id.clone();
    store.accept(&task.id, &report_id, T0 + 10).unwrap();
    let response = snapshot(&store, &[], &[], T0 + 20).unwrap();
    let fact = response
        .tasks
        .iter()
        .find(|fact| fact.id == task.id.as_str())
        .unwrap();
    assert_eq!(fact.accepted_at, Some(T0 + 10));
    assert_eq!(fact.closed_at, Some(T0 + 10));
    assert_eq!(fact.stage, "closed");
    assert_eq!(fact.version, 1);
    assert_eq!(fact.attempts, 1);
    assert!(
        !response.wait.iter().any(|item| item.kind == "accept"),
        "接受了就不在等你"
    );
}

/// 未确认图的子任务不上板，但绑总任务的确认卡仍在「等你」；确认后才上。
#[test]
fn an_unconfirmed_graph_is_not_on_the_board_but_its_confirm_card_is_waiting() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let graph = NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            NewSubtask {
                task: draft(cwd, "先查行数"),
                policy: policy(None),
                after: Vec::new(),
                milestone: false,
                program_gate: Vec::new(),
            },
            NewSubtask {
                task: draft(cwd, "再写结果"),
                policy: policy(None),
                after: vec![0],
                milestone: false,
                program_gate: Vec::new(),
            },
        ],
    };
    let root = store.create_graph(graph, T0).unwrap();
    let card = proposal(
        100,
        T0 + 1,
        1,
        "call-graph",
        "开跑这张图",
        "确认后先查、再写。",
        Some(root.id.as_str()),
    );
    let before = snapshot(&store, &card, &[], T0 + 2).unwrap();
    assert!(
        before.tasks.is_empty(),
        "未确认图的子任务还没开跑，不上工作页"
    );
    let confirm = before
        .wait
        .iter()
        .find(|item| item.kind == "confirm")
        .expect("确认卡还在等你");
    assert_eq!(confirm.title, "总任务");
    assert_eq!(confirm.task_id.as_deref(), Some(root.id.as_str()));

    store.confirm_graph(&root.id, T0 + 3).unwrap();
    let after = snapshot(&store, &card, &[], T0 + 4).unwrap();
    assert_eq!(after.tasks.len(), 2, "确认后子任务才上板");
    let waiting = after
        .tasks
        .iter()
        .find(|fact| fact.title == "再写结果")
        .unwrap();
    assert!(waiting.after_waiting, "等上游是排队，不是出了问题");
    let first = after
        .tasks
        .iter()
        .find(|fact| fact.title == "先查行数")
        .unwrap();
    assert!(!first.after_waiting);
}

/// 取消图的子任务不上板。
#[test]
fn a_cancelled_graph_is_not_on_the_board() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let root = store
        .create_graph(
            NewGraph {
                root: draft(cwd, "总任务"),
                subtasks: vec![
                    NewSubtask {
                        task: draft(cwd, "子任务一"),
                        policy: policy(None),
                        after: Vec::new(),
                        milestone: false,
                        program_gate: Vec::new(),
                    },
                    NewSubtask {
                        task: draft(cwd, "子任务二"),
                        policy: policy(None),
                        after: vec![0],
                        milestone: false,
                        program_gate: Vec::new(),
                    },
                ],
            },
            T0,
        )
        .unwrap();
    store
        .cancel_graph(&root.id, tasks::CancellationReason::Dismissed, T0 + 1)
        .unwrap();
    let response = snapshot(&store, &[], &[], T0 + 2).unwrap();
    assert!(response.tasks.is_empty(), "取消图的子任务不上板");
}

/// 未绑确认卡和 Kind 提问同时在「等你」，不能互相顶掉。
#[test]
fn an_unbound_confirm_and_the_kind_questions_coexist() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let _ = ready_task(&store, cwd, "写份结果", T0);
    let mut events = proposal(
        100,
        T0 + 10,
        2,
        "call-model",
        "换模型",
        "换成 DeepSeek V4。",
        None,
    );
    events.push(question(110, T0 + 11, 3, "q-1", "第一条问题"));
    events.push(question(111, T0 + 12, 3, "q-2", "第二条问题"));
    let response = snapshot(
        &store,
        &events,
        &[pending("第一条问题", "q-1"), pending("第二条问题", "q-2")],
        T0 + 20,
    )
    .unwrap();
    assert!(
        response
            .wait
            .iter()
            .any(|item| item.kind == "confirm" && item.title == "换模型"),
        "未绑确认卡要在"
    );
    let answers: Vec<&WaitItem> = response
        .wait
        .iter()
        .filter(|item| item.kind == "answer" && item.task_id.is_none())
        .collect();
    assert_eq!(answers.len(), 1, "Kind 提问合一项，不被确认卡顶掉");
    assert_eq!(answers[0].title, "Kind 问你");
}

/// 两张未绑确认卡各占一项（派生层按 call id 分桶；本用例用同 seq 合成两张都未替换的卡）。
#[test]
fn two_unbound_confirm_cards_are_not_collapsed_into_one() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let _ = ready_task(&store, cwd, "写份结果", T0);
    fn card(seq: u64, time: i64, call_id: &str, title: &str) -> Vec<Event> {
        let args = serde_json::json!({
            "title": title,
            "brief": "确认后继续。",
            "options": [{
                "id": "confirm",
                "action": "gate_judge",
                "label": "确认",
                "details": {},
            }]
        })
        .to_string();
        let meta = serde_json::json!({
            "card": {
                "title": title,
                "brief": "确认后继续。",
                "options": [{"id": "confirm", "label": "确认", "facts": []}]
            }
        });
        let mut result = Message::new(Role::Tool, vec![ContentBlock::text("card")]);
        result.tool_call_id = Some(call_id.to_owned());
        vec![
            Event::new(
                seq,
                u64::try_from(time).unwrap(),
                EventKind::ToolCall {
                    turn: 1,
                    step: 1,
                    call_id: call_id.to_owned(),
                    name: "propose_action".to_owned(),
                    arguments: args,
                },
            ),
            Event::new(
                seq,
                u64::try_from(time + 1).unwrap(),
                EventKind::ToolResult {
                    turn: 1,
                    step: 1,
                    message: result,
                    error: None,
                    meta: Some(meta),
                },
            ),
        ]
    }
    let mut events = card(200, T0 + 10, "call-a", "第一张卡");
    events.extend(card(200, T0 + 20, "call-b", "第二张卡"));
    let response = snapshot(&store, &events, &[], T0 + 30).unwrap();
    let titles: Vec<&str> = response
        .wait
        .iter()
        .filter(|item| item.kind == "confirm")
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(titles.len(), 2, "两张未绑卡各占一项：{titles:?}");
    assert!(titles.contains(&"第一张卡"));
    assert!(titles.contains(&"第二张卡"));
}

/// T056.1：手工量 Work 在长会话（任务多、事件多）上的耗时；不进常规门禁。
#[test]
#[ignore = "T056.1 手工测量 Work 长会话耗时"]
fn measure_work_latency_on_a_long_session() {
    use std::time::Instant;
    let (directory, store) = open_store();
    let cwd = directory.path();
    let mut tasks = Vec::new();
    for index in 0..100_i64 {
        tasks.push(ready_task(
            &store,
            cwd,
            &format!("长会话任务 {index}"),
            T0 + index * 10,
        ));
    }
    let mut events = Vec::new();
    let mut seq = 1_u64;
    for (index, task) in tasks.iter().enumerate() {
        let report = task.latest_report().unwrap();
        events.push(relay(
            seq,
            T0 + 1_000 + i64::try_from(index).expect("非负"),
            u64::try_from(index + 1).expect("非负"),
            task.id.as_str(),
            report.id.as_str(),
        ));
        seq += 1;
    }
    while events.len() < 20_000 {
        events.push(Event::new(
            seq,
            u64::try_from(T0 + 2_000_000 + i64::try_from(seq).unwrap()).unwrap(),
            EventKind::AssistantMessage {
                turn: seq / 10 + 1,
                step: 1,
                message: Message::text(Role::Assistant, "长会话里的一条普通回答"),
                stream: Vec::new(),
                usage: None,
                interrupted: None,
                effort: None,
                effort_wanted: None,

                request_at: None,
                context_parts: None,
            },
        ));
        seq += 1;
    }
    let now = T0 + 3_000_000;
    // 预热一次，避免把首次建 map/分配算进。
    let _ = snapshot(&store, &events, &[], now).unwrap();

    let copy_start = Instant::now();
    let cloned = events.clone();
    let copy = copy_start.elapsed();

    let snapshot_start = Instant::now();
    let response = snapshot(&store, &cloned, &[], now).unwrap();
    let snapshot_time = snapshot_start.elapsed();
    let total = copy + snapshot_time;
    println!(
        "T056_WORK_LATENCY tasks={} events={} waits={} copy_ms={:.2} snapshot_ms={:.2} total_ms={:.2}",
        tasks.len(),
        events.len(),
        response.wait.len(),
        copy.as_secs_f64() * 1000.0,
        snapshot_time.as_secs_f64() * 1000.0,
        total.as_secs_f64() * 1000.0
    );
}

#[test]
fn a_confirmed_gated_graph_projects_edges_and_judge_state() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = Store::open(cwd.join("tasks.sqlite")).unwrap();
    let root = store.create_graph(gated_graph(cwd), T0).unwrap();
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let children = store.children(&root.id).unwrap();
    let (guarded, gate, downstream) = (&children[0], &children[1], &children[2]);
    let (guarded, gate) = gate_attempt_and_report(&store, guarded, gate, T0 + 10);
    let gate_report = gate.latest_report().cloned().unwrap();

    let response = snapshot(&store, &[], &[], T0 + 20).unwrap();
    assert_eq!(response.graphs.len(), 1, "只投影已确认的图");
    let graph = &response.graphs[0];
    assert_eq!(graph.id, root.id.as_str());
    assert_eq!(graph.title, "总任务");
    let guards = edge(graph, guarded.id.as_str(), gate.id.as_str(), "guards");
    assert_eq!(guards.state, "handed", "门有过 Attempt 就是 handed");
    assert!(!guards.program);
    assert_eq!(guards.live.as_deref(), Some("judge"), "门有报告没判：judge");
    assert_eq!(graph.edges.len(), 2, "门图 3 站 2 条：guards + release");
    assert!(
        !graph
            .edges
            .iter()
            .any(|edge| edge.kind == "needs" && edge.from == guarded.id.as_str()),
        "被把关上游不再出 needs"
    );
    let release = edge(graph, gate.id.as_str(), downstream.id.as_str(), "release");
    assert_eq!(release.state, "judge", "门还没判");
    assert_eq!(release.live.as_deref(), Some("judge"));
    assert!(!release.program);

    store
        .judge_gate(
            &gate.id,
            &gate_report.id,
            tasks::GateConclusion::NeedsFix,
            None,
            T0 + 30,
        )
        .unwrap();
    let response = snapshot(&store, &[], &[], T0 + 31).unwrap();
    let release = edge(
        &response.graphs[0],
        gate.id.as_str(),
        downstream.id.as_str(),
        "release",
    );
    assert_eq!(release.state, "fix", "判了需修正");
    assert_eq!(release.live.as_deref(), Some("back"));
    assert!(
        release
            .payload
            .as_deref()
            .is_some_and(|payload| payload.contains("打回第 1 份")),
        "{:?}",
        release.payload
    );
}

#[test]
fn two_upstreams_show_one_handed_and_one_waiting() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = Store::open(cwd.join("tasks.sqlite")).unwrap();
    let root = store
        .create_graph(two_upstreams_gated_graph(cwd), T0)
        .unwrap();
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let children = store.children(&root.id).unwrap();
    let (guarded, gate, independent, downstream) =
        (&children[0], &children[1], &children[2], &children[3]);
    let (guarded, gate) = gate_attempt_and_report(&store, guarded, gate, T0 + 10);
    let gate_report = gate.latest_report().cloned().unwrap();
    store
        .judge_gate(
            &gate.id,
            &gate_report.id,
            tasks::GateConclusion::Pass,
            None,
            T0 + 20,
        )
        .unwrap();

    store.begin_attempt(&independent.id, T0 + 15).unwrap();
    let response = snapshot(&store, &[], &[], T0 + 21).unwrap();
    assert_eq!(response.graphs.len(), 1);
    let graph = &response.graphs[0];
    assert!(
        !graph
            .edges
            .iter()
            .any(|edge| edge.kind == "needs" && edge.from == guarded.id.as_str()),
        "被门把关的上游与下游之间不再出 needs"
    );
    assert_eq!(
        edge(
            graph,
            independent.id.as_str(),
            downstream.id.as_str(),
            "needs"
        )
        .state,
        "waiting",
        "B 还没 ready，还是 waiting"
    );
    let needs = edge(
        graph,
        independent.id.as_str(),
        downstream.id.as_str(),
        "needs",
    );
    assert_eq!(
        needs.live.as_deref(),
        Some("make"),
        "上游在跑、下游等：make"
    );
    assert_eq!(
        edge(graph, gate.id.as_str(), downstream.id.as_str(), "release").state,
        "handed",
        "判通过且下游交接已解析"
    );
    // 下游开跑、上游报告带改动文件数：线是 flow，小字写第 N 份·X 文件。
    store
        .set_report_changed_count(&gate.id, &gate_report.id, 2)
        .unwrap();
    store.begin_attempt(&downstream.id, T0 + 25).unwrap();
    let response = snapshot(&store, &[], &[], T0 + 26).unwrap();
    let release = edge(
        &response.graphs[0],
        gate.id.as_str(),
        downstream.id.as_str(),
        "release",
    );
    assert_eq!(release.live.as_deref(), Some("flow"));
    assert!(
        release
            .payload
            .as_deref()
            .is_some_and(|payload| payload.contains("2 文件")),
        "{:?}",
        release.payload
    );
}

#[test]
fn a_program_gate_graph_has_program_edges_and_no_judge() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = Store::open(cwd.join("tasks.sqlite")).unwrap();
    let root = store.create_graph(program_gated_graph(cwd), T0).unwrap();
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let children = store.children(&root.id).unwrap();
    let (guarded, gate, downstream) = (&children[0], &children[1], &children[2]);
    let attempt = start(&store, guarded, T0 + 10);
    store
        .record_report(
            &guarded.id,
            attempt,
            report(ReportState::Ready, "查好了", None),
            T0 + 11,
        )
        .unwrap();
    let guarded = store.task(&guarded.id).unwrap().unwrap();
    let guarded_report = guarded.latest_report().cloned().unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 12)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    store
        .begin_attempt_with(
            &gate.id,
            T0 + 13,
            tasks::AttemptTrigger::GateRecheck {
                candidate: tasks::GateCandidate {
                    report_id: guarded_report.id.clone(),
                    requirement_version: guarded_report.version,
                },
            },
            vec![tasks::Handoff {
                report_id: guarded_report.id.clone(),
                requirement_version: guarded_report.version,
                summary: guarded_report.summary.clone(),
            }],
        )
        .unwrap();

    let response = snapshot(&store, &[], &[], T0 + 20).unwrap();
    let graph = &response.graphs[0];
    assert!(
        !graph.edges.iter().any(|edge| edge.kind == "needs"),
        "程序门同样不画平行的 needs"
    );
    let release = edge(graph, gate.id.as_str(), downstream.id.as_str(), "release");
    assert!(release.program, "程序门的线要标 program");
    assert_eq!(
        release.state, "waiting",
        "程序门没跑完是 waiting，不出现 judge"
    );
    assert!(!graph.edges.iter().any(|edge| edge.state == "judge"));
}

#[test]
fn unconfirmed_and_cancelled_graphs_do_not_project() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = Store::open(cwd.join("tasks.sqlite")).unwrap();
    let unconfirmed = store.create_graph(gated_graph(cwd), T0).unwrap();
    let cancelled = store.create_graph(gated_graph(cwd), T0 + 10).unwrap();
    store
        .cancel_graph(&cancelled.id, tasks::CancellationReason::Dismissed, T0 + 12)
        .unwrap();
    let confirmed = store.create_graph(gated_graph(cwd), T0 + 20).unwrap();
    store.confirm_graph(&confirmed.id, T0 + 21).unwrap();
    let single = store.create_task(draft(cwd, "单派一个"), T0 + 30).unwrap();

    let response = snapshot(&store, &[], &[], T0 + 40).unwrap();
    assert_eq!(response.graphs.len(), 1, "只有已确认、没取消的图进 graphs");
    assert_eq!(response.graphs[0].id, confirmed.id.as_str());
    assert!(
        !response
            .graphs
            .iter()
            .any(|graph| graph.id == unconfirmed.id.as_str() || graph.id == cancelled.id.as_str()),
        "未确认/取消的图不进 graphs"
    );
    assert!(
        response
            .tasks
            .iter()
            .any(|fact| fact.id == single.id.as_str()),
        "单派任务继续上看板"
    );
}

#[test]
fn work_facts_carry_changed_count_and_running_since() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = Store::open(cwd.join("tasks.sqlite")).unwrap();
    let task = ready_task(&store, cwd, "有改动", T0);
    let report_id = task.latest_report().unwrap().id.clone();
    store
        .set_report_changed_count(&task.id, &report_id, 3)
        .unwrap();
    let before = snapshot(&store, &[], &[], T0 + 10).unwrap();
    let fact = before
        .tasks
        .iter()
        .find(|fact| fact.id == task.id.as_str())
        .expect("任务事实");
    assert_eq!(fact.latest.as_ref().unwrap().changed_count, Some(3));
    assert!(fact.running_since.is_none());
    store.begin_attempt(&task.id, T0 + 10).unwrap();
    let after = snapshot(&store, &[], &[], T0 + 20).unwrap();
    let fact = after
        .tasks
        .iter()
        .find(|fact| fact.id == task.id.as_str())
        .expect("任务事实");
    assert!(
        fact.running_since.is_some(),
        "第二个 Attempt 在跑要有 started_at"
    );
    assert!(fact.running);
}

#[test]
fn cancelled_task_leaves_wait_and_keeps_cancelled_at_on_fact() {
    let (_directory, store) = open_store();
    // 有 ready 报告的任务：取消后不能再挂「接受」等你项。
    let task = ready_task(&store, Path::new("/tmp/work-cancel"), "取消任务", T0);
    store
        .cancel(&task.id, tasks::CancellationSource::User, None, T0 + 10)
        .unwrap();
    let response = snapshot(&store, &[], &[], T0 + 20).unwrap();
    let fact = response
        .tasks
        .iter()
        .find(|fact| fact.id == task.id.as_str())
        .expect("任务仍在事实里");
    assert_eq!(fact.stage, "closed");
    assert_eq!(fact.cancelled_at, Some(T0 + 10));
    assert_eq!(fact.closed_at, Some(T0 + 10));
    assert!(
        !response
            .wait
            .iter()
            .any(|item| item.task_id.as_deref() == Some(task.id.as_str())),
        "取消后等你项要离开"
    );
}
