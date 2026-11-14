#![allow(dead_code)]
//! T056.2：任务账本的唯一编译器。
//!
//! 表里只存两样程序事实：约束（跟着要求版本）和 ThinAgent 的叙述条（rejected/next）。
//! 进展、待答从最近一份报告现编，不另存；本模块是唯一把它们编成一段文本的地方。

use std::collections::HashSet;
use std::path::Path;

use session::EventKind;
use tasks::{LedgerNote, LedgerNoteKind, ReportState, Store, Task};

/// 一条约束和它的出处（None = 本任务，用户在这次任务的对话里说的）。
pub(crate) struct LedgerConstraint {
    pub text: String,
    pub from: Option<String>,
}

/// 一条在用的叙述条。
pub(crate) struct LedgerNoteView {
    pub id: String,
    pub kind: LedgerNoteKind,
    pub text: String,
    pub attempt: u64,
}

/// 一条历史叙述条（已经关掉，不再带给 ThinAgent）。
pub(crate) struct LedgerHistoryNote {
    pub id: String,
    pub kind: LedgerNoteKind,
    pub text: String,
    pub attempt: u64,
    pub closed_at: i64,
    pub closed_why: String,
}

/// 最近一份报告里的进展/待答。
pub(crate) struct LedgerProgress {
    pub state: ReportState,
    pub summary: String,
    pub question: Option<String>,
    pub attempt: u64,
    pub at: i64,
}

/// 一个任务现编出来的账本。
#[derive(Default)]
pub(crate) struct LedgerView {
    pub constraints: Vec<LedgerConstraint>,
    pub progress: Option<LedgerProgress>,
    pub next: Vec<LedgerNoteView>,
    pub rejected: Vec<LedgerNoteView>,
    pub history: Vec<LedgerHistoryNote>,
}

impl LedgerView {
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
            && self.progress.is_none()
            && self.next.is_empty()
            && self.rejected.is_empty()
    }

    /// 注入用的一段文本；空账本返回空串（调用方就不发/不加这一节）。
    pub fn section(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut lines = vec!["账本：".to_owned()];
        if !self.constraints.is_empty() {
            lines.push("约束：".to_owned());
            for constraint in &self.constraints {
                match &constraint.from {
                    Some(from) => {
                        lines.push(format!("- 「{}」（来自「{from}」）", constraint.text));
                    }
                    None => lines.push(format!("- 「{}」（你说的）", constraint.text)),
                }
            }
        }
        if let Some(progress) = &self.progress {
            lines.push(format!(
                "进展：{} · {}",
                report_state_word(progress.state),
                progress.summary
            ));
            if let Some(question) = &progress.question {
                lines.push(format!("待答：{question}"));
            }
        }
        if !self.next.is_empty() {
            lines.push("下一步：".to_owned());
            for note in &self.next {
                lines.push(format!("- {}", note.text));
            }
        }
        if !self.rejected.is_empty() {
            lines.push("否掉的做法：".to_owned());
            for note in &self.rejected {
                lines.push(format!("- {}", note.text));
            }
        }
        lines.join("\n")
    }
}

/// 唯一编账本处：祖先约束远→近 + 本任务；进展/待答取最近报告；在用叙述条按类。
pub(crate) fn ledger_for(store: &Store, task: &Task) -> Result<LedgerView, String> {
    let mut constraints = Vec::new();
    let mut seen = HashSet::new();
    let mut ancestors = Vec::new();
    let mut parent = task.parent.clone();
    while let Some(id) = parent {
        let ancestor = store
            .task(&id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("账本的祖先任务不存在：{id}"))?;
        parent = ancestor.parent.clone();
        ancestors.push(ancestor);
    }
    ancestors.reverse();
    for ancestor in ancestors {
        for text in ancestor.current_constraints() {
            push_constraint(
                &mut constraints,
                &mut seen,
                text,
                Some(ancestor.title.clone()),
            );
        }
    }
    for text in task.current_constraints() {
        push_constraint(&mut constraints, &mut seen, text, None);
    }

    let notes = store
        .ledger_notes(&task.id)
        .map_err(|error| error.to_string())?;
    let mut next = Vec::new();
    let mut rejected = Vec::new();
    let mut history = Vec::new();
    for note in notes {
        if let Some(view) = note_view(&note) {
            match (&note.closed_at, note.kind) {
                (None, LedgerNoteKind::Next) => next.push(view),
                (None, LedgerNoteKind::Rejected) => rejected.push(view),
                (Some(closed_at), kind) => history.push(LedgerHistoryNote {
                    id: view.id,
                    kind,
                    text: view.text,
                    attempt: view.attempt,
                    closed_at: *closed_at,
                    closed_why: note
                        .closed_why
                        .clone()
                        .unwrap_or_else(|| "已关闭".to_owned()),
                }),
            }
        }
    }
    history.sort_by_key(|note| std::cmp::Reverse(note.closed_at));
    let progress = task.latest_report().map(|report| LedgerProgress {
        state: report.state,
        summary: report.summary.clone(),
        question: report.question.clone(),
        attempt: report.attempt_id.0,
        at: report.at,
    });
    Ok(LedgerView {
        constraints,
        progress,
        next,
        rejected,
        history,
    })
}

fn push_constraint(
    constraints: &mut Vec<LedgerConstraint>,
    seen: &mut HashSet<String>,
    text: &str,
    from: Option<String>,
) {
    let text = text.trim();
    if text.is_empty() || !seen.insert(text.to_owned()) {
        return;
    }
    constraints.push(LedgerConstraint {
        text: text.to_owned(),
        from,
    });
}

fn note_view(note: &LedgerNote) -> Option<LedgerNoteView> {
    Some(LedgerNoteView {
        id: note.id.clone(),
        kind: note.kind,
        text: note.text.clone(),
        attempt: note.attempt_id.0,
    })
}

/// 报告状态给账本段看的短词。
pub(crate) fn report_state_word(state: ReportState) -> &'static str {
    match state {
        ReportState::Ready => "已交结果",
        ReportState::Blocked => "卡住了",
        ReportState::Failed => "失败",
        ReportState::Waiting => "在等",
    }
}

/// T056.2：约束必须在发起会话的某条用户消息里原样出现（去首尾空白后），否则整次拒。
pub(crate) fn verify_user_words(
    data_dir: &Path,
    conversation: &str,
    constraints: &[String],
) -> Result<(), String> {
    let wanted: Vec<&str> = constraints
        .iter()
        .map(|constraint| constraint.trim())
        .filter(|constraint| !constraint.is_empty())
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let messages = user_message_texts(data_dir, conversation)?;
    for constraint in wanted {
        if !messages.iter().any(|message| message.contains(constraint)) {
            return Err(format!(
                "约束要用用户原话：『{constraint}』不在用户的消息里"
            ));
        }
    }
    Ok(())
}

/// 发起会话里用户真正打过的字；程序注入的 relay/说明消息排除。
fn user_message_texts(data_dir: &Path, conversation: &str) -> Result<Vec<String>, String> {
    let session_id = session::SessionId::parse(conversation)
        .map_err(|error| format!("发起会话 id 不合法：{error}"))?;
    let store = session::SessionStore::open_read_only(data_dir, &session_id)
        .map_err(|error| format!("拿不到发起会话的消息：{error}"))?;
    let mut texts = Vec::new();
    for event in store.events() {
        let EventKind::UserMessage { message, .. } = &event.kind else {
            continue;
        };
        if is_generated_message(&message.source) {
            continue;
        }
        let text: String = message
            .content
            .iter()
            .filter_map(model::ContentBlock::as_text)
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        if !text.trim().is_empty() {
            texts.push(text);
        }
    }
    Ok(texts)
}

fn is_generated_message(source: &model::MessageSource) -> bool {
    let model::MessageSource::User {
        rpc_id: Some(rpc_id),
    } = source
    else {
        return false;
    };
    [
        "task-report:",
        "task-cc:",
        "task-stop:",
        "task-update:",
        "task-requirement:",
        "task-accept-many:",
        "assignment:",
        "upstream:",
        "resume:",
        "ledger-after-compaction:",
    ]
    .iter()
    .any(|prefix| rpc_id.starts_with(prefix))
}
#[cfg(test)]
mod ledger_tests {
    use std::path::PathBuf;

    use tasks::{
        ConversationId, ModelChoice, NewGraph, NewLedgerNote, NewReport, NewSubtask, NewTask,
        ReportOrigin, ReportState, TaskPolicy, VersionSource,
    };

    use super::{ledger_for, verify_user_words};

    const T0: i64 = 1_700_000_000_000;

    fn draft(title: &str) -> NewTask {
        NewTask {
            title: title.to_owned(),
            requirement: "把结果写出来".to_owned(),
            dod: "有出处".to_owned(),
            cwd: PathBuf::from("/tmp/kind-cwd"),
            conversation: ConversationId::new("kind-session-1"),
            model: ModelChoice {
                connection: "deepseek".to_owned(),
                model: "deepseek-chat".to_owned(),
                effort_wanted: "default".to_owned(),
            },
            tools: vec!["read".to_owned()],
        }
    }

    fn open() -> (tempfile::TempDir, tasks::Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = tasks::Store::open(directory.path().join("tasks.sqlite")).unwrap();
        (directory, store)
    }

    #[test]
    fn scope_constraints_far_ancestor_dedups_and_does_not_leak_between_siblings() {
        let (_directory, store) = open();
        let graph = NewGraph {
            root: draft("总任务"),
            subtasks: vec![
                NewSubtask {
                    task: draft("A"),
                    policy: TaskPolicy::default(),
                    after: Vec::new(),
                    milestone: false,
                },
                NewSubtask {
                    task: draft("B"),
                    policy: TaskPolicy::default(),
                    after: Vec::new(),
                    milestone: false,
                },
            ],
        };
        let root = store
            .create_graph_with_constraints(graph, vec!["别加新依赖".to_owned()], T0)
            .unwrap();
        let children = store.children(&root.id).unwrap();
        let a = &children[0];
        let b = &children[1];
        store
            .update_requirement_with_constraints(
                &a.id,
                "A 的新要求",
                None,
                Some(vec!["只改一个文件".to_owned()]),
                T0 + 1,
            )
            .unwrap();
        let a = store.task(&a.id).unwrap().expect("A 还在");
        let a_view = ledger_for(&store, &a).unwrap();
        let texts: Vec<&str> = a_view
            .constraints
            .iter()
            .map(|item| item.text.as_str())
            .collect();
        assert_eq!(texts, ["别加新依赖", "只改一个文件"]);
        assert_eq!(a_view.constraints[0].from.as_deref(), Some("总任务"));
        assert!(a_view.constraints[1].from.is_none());

        let b = store.task(&b.id).unwrap().expect("B 还在");
        let b_view = ledger_for(&store, &b).unwrap();
        let b_texts: Vec<&str> = b_view
            .constraints
            .iter()
            .map(|item| item.text.as_str())
            .collect();
        assert_eq!(b_texts, ["别加新依赖"], "兄弟的约束不串");
    }

    #[test]
    fn renders_progress_question_constraints_and_live_notes_but_not_history() {
        let (_directory, store) = open();
        let task = store
            .create_task_with_constraints(
                draft("带账本"),
                TaskPolicy::default(),
                vec!["别加新依赖".to_owned()],
                T0,
            )
            .unwrap();
        let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
        store
            .add_ledger_notes(
                &task.id,
                attempt,
                &[
                    NewLedgerNote {
                        kind: tasks::LedgerNoteKind::Rejected,
                        topic: "cache".to_owned(),
                        text: "缓存不可行".to_owned(),
                    },
                    NewLedgerNote {
                        kind: tasks::LedgerNoteKind::Next,
                        topic: "step".to_owned(),
                        text: "先跑测试".to_owned(),
                    },
                ],
                T0 + 2,
            )
            .unwrap();
        store
            .record_report(
                &task.id,
                attempt,
                NewReport {
                    state: ReportState::Blocked,
                    origin: ReportOrigin::Agent,
                    summary: "缺一个文件".to_owned(),
                    question: Some("给哪个文件？".to_owned()),
                },
                T0 + 3,
            )
            .unwrap();
        let task = store.task(&task.id).unwrap().expect("任务还在");
        let view = ledger_for(&store, &task).unwrap();
        let text = view.section();
        for expected in [
            "账本：",
            "「别加新依赖」（你说的）",
            "进展：卡住了 · 缺一个文件",
            "待答：给哪个文件？",
            "下一步：",
            "先跑测试",
            "否掉的做法：",
            "缓存不可行",
        ] {
            assert!(text.contains(expected), "缺 {expected}\n{text}");
        }
        assert!(!text.contains("历史"));
        assert!(view.history.is_empty());
        let _ = VersionSource::UserAgreed;
    }

    #[test]
    fn user_words_must_be_verbatim_in_a_real_user_message() {
        let directory = tempfile::tempdir().unwrap();
        let mut session =
            session::SessionStore::create(directory.path(), directory.path()).unwrap();
        let conversation = session.id().as_str().to_owned();
        session
            .append(&[session::EventKind::UserMessage {
                turn: 1,
                step: 0,
                message: model::UserMessage::text("先查文档，别加新依赖").into_message(),
            }])
            .unwrap();
        session
            .append(&[session::EventKind::UserMessage {
                turn: 2,
                step: 0,
                message: model::UserMessage::text_with_request_id(
                    "任务结果：别加新依赖",
                    "task-report:t:r",
                )
                .into_message(),
            }])
            .unwrap();
        drop(session);

        verify_user_words(directory.path(), &conversation, &["别加新依赖".to_owned()])
            .expect("用户原话");
        let error = verify_user_words(directory.path(), &conversation, &["不许加依赖".to_owned()])
            .unwrap_err();
        assert!(error.contains("约束要用用户原话"));
        assert!(error.contains("不许加依赖"));
        // 只出现在程序注入的 relay 里不算用户原话。
        let error = verify_user_words(
            directory.path(),
            &conversation,
            &["任务结果：别加新依赖".to_owned()],
        )
        .unwrap_err();
        assert!(error.contains("不在用户的消息里"));
    }
}
