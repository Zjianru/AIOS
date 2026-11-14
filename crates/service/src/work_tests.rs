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
            },
            milestone(cwd, "独立核查", vec![0]),
            NewSubtask {
                task: draft(cwd, "再写结果"),
                policy: policy(None),
                after: vec![0],
                milestone: false,
            },
        ],
    }
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
