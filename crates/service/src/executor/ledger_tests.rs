use std::sync::{Arc, Mutex, Weak};

use model::{DEFAULT_CONTEXT_WINDOW, Effort};
use tasks::{NewReport, ReportOrigin, ReportState, TaskPolicy};

use super::*;
use crate::root::RootSessionOptions;

fn open_runtime() -> (tempfile::TempDir, Arc<TaskRuntime>) {
    let directory = tempfile::tempdir().unwrap();
    let options = RootSessionOptions {
        projects: None,
        data_dir: directory.path().to_path_buf(),
        cwd: directory.path().to_path_buf(),
        model: model::DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::new("high"),
            tool_update: None,
        },
        config: agent::AgentConfig::default(),
        credentials: None,
        identity: None,
        model_factory: None,
        append_hook_factory: None,
        keyring_changed: None,
        actions: Arc::new(crate::actions::ActionRegistry::empty()),
        exa_base_url: None,
        web_listen: None,
        task_runtime: None,
    };
    let runtime =
        TaskRuntime::new(&options, Arc::new(Mutex::new(None)), Weak::new(), None).unwrap();
    (directory, Arc::new(runtime))
}

fn new_task(directory: &std::path::Path, constraints: Vec<String>) -> tasks::Task {
    runtime_store_task(directory, constraints, false)
}

fn runtime_store_task(
    directory: &std::path::Path,
    constraints: Vec<String>,
    bash: bool,
) -> tasks::Task {
    let store = Store::open(directory.join(TASKS_DB_FILE)).unwrap();
    let writing = bash;
    let tools: Vec<String> = if writing {
        THIN_WRITE_TOOLS
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    } else {
        THIN_TOOLS.iter().map(|name| (*name).to_owned()).collect()
    };
    store
        .create_task_with_constraints(
            tasks::NewTask {
                quote: Some("账本注入原话".to_owned()),
                quote_source: None,
                kind_note: Some("Kind 的补充原话".to_owned()),
                project_id: None,
                cwd_source: None,
                title: "账本注入".to_owned(),
                requirement: "把事做完".to_owned(),
                dod: "有结果".to_owned(),
                cwd: directory.to_path_buf(),
                conversation: tasks::ConversationId::new("kind-1"),
                model: tasks::ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools,
            },
            TaskPolicy {
                write_scope: writing.then(|| "out.txt".to_owned()),
                worktree: false,
                bash,
            },
            constraints,
            1_700_000_000_000,
        )
        .unwrap()
}

#[test]
fn continue_package_appends_the_ledger_for_every_cause() {
    let (directory, runtime) = open_runtime();
    let store = Store::open(directory.path().join(TASKS_DB_FILE)).unwrap();
    let task = new_task(directory.path(), vec!["别加新依赖".to_owned()]);
    let attempt = store.begin_attempt(&task.id, 1).unwrap();
    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[tasks::NewLedgerNote {
                kind: tasks::LedgerNoteKind::Next,
                topic: "step".to_owned(),
                text: "先跑测试".to_owned(),
            }],
            2,
        )
        .unwrap();
    let report = store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Blocked,
                origin: ReportOrigin::Agent,
                summary: "缺文件".to_owned(),
                question: None,
            },
            3,
        )
        .unwrap();
    let task = store.task(&task.id).unwrap().unwrap();
    let package = runtime
        .continue_package(&task, &tasks::ContinueCause::AfterReport { report }, None)
        .unwrap();
    assert!(package.contains("账本："), "{package}");
    assert!(package.contains("「别加新依赖」"), "{package}");
    assert!(package.contains("先跑测试"), "{package}");
    assert!(package.contains("缺文件"), "{package}");
}

#[test]
fn prepare_reports_the_ledger_limit_as_a_blocking_missing_item() {
    let (directory, runtime) = open_runtime();
    let long = "要求".repeat(3001);
    let task = new_task(directory.path(), vec![long]);
    let Err(missing) = runtime.prepare(&task) else {
        panic!("超限账本不该通过 prepare");
    };
    let text = missing
        .iter()
        .map(tasks::MissingItem::as_str)
        .collect::<Vec<_>>()
        .join("｜");
    assert!(text.contains("账本超出上限"), "{text}");
    assert!(text.contains("任务太大，请拆任务"), "{text}");
}

fn user_session(directory: &std::path::Path) -> (session::SessionId, ToolContext) {
    let mut log = session::SessionStore::create(directory, directory).unwrap();
    let id = log.id().clone();
    log.append(&[session::EventKind::UserMessage {
        turn: 1,
        step: 0,
        message: model::UserMessage::text_with_request_id(
            "改一下，别加新依赖",
            "7f1c2d3e-4a5b-4c6d-8e9f-0a1b2c3d4e5f",
        )
        .into_message(),
    }])
    .unwrap();
    let state = Arc::new(tools::ToolSessionState::new(directory, id.as_str()));
    let context = ToolContext::with_session(directory.to_path_buf(), state);
    (id, context)
}

#[test]
fn accept_and_propose_verify_constraints_against_the_originating_conversation() {
    let (directory, runtime) = open_runtime();
    let (_session, context) = user_session(directory.path());
    *runtime.selection.lock().expect("selection") = Some(session::ModelSelection {
        connection: "deepseek".to_owned(),
        model: "deepseek-flash".to_owned(),
        effort_wanted: "high".to_owned(),
    });
    let task = runtime
        .accept(
            &context,
            "call-ok",
            TaskAcceptArgs {
                quote: "改一下".to_owned(),
                kind_note: None,
                title: "带约束".to_owned(),
                requirement: "把事做完".to_owned(),
                dod: "有结果".to_owned(),
                cwd: None,
                project: None,
                write_scope: None,
                worktree: false,
                constraints: vec!["别加新依赖".to_owned()],
            },
            false,
        )
        .expect("原话约束应通过")
        .0;
    assert_eq!(task.current_constraints(), ["别加新依赖"]);

    let error = runtime
        .accept(
            &context,
            "call-bad",
            TaskAcceptArgs {
                quote: "改一下".to_owned(),
                kind_note: None,
                title: "伪造".to_owned(),
                requirement: "把事做完".to_owned(),
                dod: "有结果".to_owned(),
                cwd: None,
                project: None,
                write_scope: None,
                worktree: false,
                constraints: vec!["用户没说过".to_owned()],
            },
            false,
        )
        .expect_err("非原话必须整次拒");
    assert!(
        error.message.contains("约束要用用户原话"),
        "{}",
        error.message
    );

    let graph = TaskGraphArgs {
        quote: "改一下".to_owned(),
        kind_note: None,
        title: "图".to_owned(),
        requirement: "两步".to_owned(),
        dod: "都有结果".to_owned(),
        cwd: None,
        project: None,
        subtasks: vec![
            GraphSubtaskArgs {
                title: "一".to_owned(),
                requirement: "第一步".to_owned(),
                dod: "有结果".to_owned(),
                write_scope: None,
                worktree: false,
                after: Vec::new(),
                milestone: false,
                program_gate: Vec::new(),
            },
            GraphSubtaskArgs {
                title: "二".to_owned(),
                requirement: "第二步".to_owned(),
                dod: "有结果".to_owned(),
                write_scope: None,
                worktree: false,
                after: Vec::new(),
                milestone: false,
                program_gate: Vec::new(),
            },
        ],
        constraints: vec!["别加新依赖".to_owned()],
    };
    let (root, _) = runtime
        .propose_graph(&context, "graph-ok", graph)
        .expect("图约束原话应通过");
    assert_eq!(root.current_constraints(), ["别加新依赖"]);
}

fn tool_context(directory: &std::path::Path) -> ToolContext {
    let state = Arc::new(tools::ToolSessionState::new(directory, "session-1"));
    ToolContext::with_session(directory.to_path_buf(), state)
}

async fn run_note(tool: &TaskLedgerNoteTool, args: serde_json::Value) -> ToolResult {
    tool.execute(
        ToolCall {
            id: "call-note".to_owned(),
            name: "task_ledger_note".to_owned(),
            arguments: args.to_string(),
        },
        &tool_context(std::path::Path::new("/tmp")),
    )
    .await
}

#[tokio::test]
async fn ledger_note_tool_rejects_bad_kind_long_text_and_ended_attempt() {
    let (directory, _runtime) = open_runtime();
    let runtime = TaskRuntime::new(
        &crate::root::RootSessionOptions {
            projects: None,
            data_dir: directory.path().to_path_buf(),
            cwd: directory.path().to_path_buf(),
            model: model::DeepSeekConfig {
                provider: "deepseek".to_owned(),
                base_url: "http://127.0.0.1:1".to_owned(),
                model: "deepseek-flash".to_owned(),
                max_tokens: 2048,
                context_window: DEFAULT_CONTEXT_WINDOW,
                effort: Effort::new("high"),
                tool_update: None,
            },
            config: agent::AgentConfig::default(),
            credentials: None,
            identity: None,
            model_factory: None,
            append_hook_factory: None,
            keyring_changed: None,
            actions: Arc::new(crate::actions::ActionRegistry::empty()),
            exa_base_url: None,
            web_listen: None,
            task_runtime: None,
        },
        Arc::new(Mutex::new(None)),
        Weak::new(),
        None,
    )
    .unwrap();
    let runtime = Arc::new(runtime);
    let store = Store::open(directory.path().join(TASKS_DB_FILE)).unwrap();
    let task = new_task(directory.path(), Vec::new());
    let attempt = store.begin_attempt(&task.id, 1).unwrap();
    let tool = TaskLedgerNoteTool::new(Arc::clone(&runtime), task.id.clone(), attempt);

    let bad_kind = run_note(
        &tool,
        serde_json::json!({"entries":[{"kind":"progress","topic":"x","text":"不算"}]}),
    )
    .await;
    assert!(bad_kind.is_error);
    assert!(
        bad_kind.content.contains("rejected 或 next"),
        "{}",
        bad_kind.content
    );

    let too_long = run_note(
        &tool,
        serde_json::json!({"entries":[{"kind":"next","topic":"x","text":"长".repeat(401)}]}),
    )
    .await;
    assert!(too_long.is_error);
    assert!(
        too_long.content.contains("上限 400"),
        "{}",
        too_long.content
    );

    let ok = run_note(
        &tool,
        serde_json::json!({"entries":[
            {"kind":"rejected","topic":"cache","text":"缓存不可行"},
            {"kind":"next","topic":"step","text":"先跑测试"}
        ]}),
    )
    .await;
    assert!(!ok.is_error, "{}", ok.content);
    assert!(ok.content.contains("否掉的做法 1 条"), "{}", ok.content);
    assert!(ok.content.contains("下一步 1 条"), "{}", ok.content);

    let history = TaskLedgerHistoryTool::new(Arc::clone(&runtime), task.id.clone())
        .execute(
            ToolCall {
                id: "call-history".to_owned(),
                name: "task_ledger_history".to_owned(),
                arguments: "{}".to_owned(),
            },
            &tool_context(std::path::Path::new("/tmp")),
        )
        .await;
    assert!(!history.is_error);
    assert!(
        history.content.contains("缓存不可行"),
        "{}",
        history.content
    );
    assert!(history.content.contains("先跑测试"), "{}", history.content);

    store
        .end_attempt(&task.id, attempt, 2, tasks::AttemptEnd::Stopped)
        .unwrap();
    let ended = run_note(
        &tool,
        serde_json::json!({"entries":[{"kind":"next","topic":"y","text":"晚了"}]}),
    )
    .await;
    assert!(ended.is_error);
    assert!(ended.content.contains("已经结束"), "{}", ended.content);
}

#[test]
fn compaction_fixed_block_carries_current_assignment_and_ledger_even_when_empty() {
    let (directory, runtime) = open_runtime();
    let original = new_task(directory.path(), vec!["别加新依赖".to_owned()]);
    let store = Store::open(directory.path().join(TASKS_DB_FILE)).unwrap();
    let attempt = store.begin_attempt(&original.id, 1).unwrap();
    let task = store.task(&original.id).unwrap().unwrap();
    let block = runtime
        .compaction_task_block(&task.id, attempt)
        .expect("当前 Attempt 有任务书固定块");
    assert_eq!(block.kind, "task");
    assert!(
        block.text.contains("把事做完"),
        "带 requirement：{}",
        block.text
    );
    assert!(block.text.contains("有结果"), "带 dod：{}", block.text);
    assert!(
        block.text.contains("user_quotes") && block.text.contains("账本注入原话"),
        "压缩重铺带按版本原话：{}",
        block.text
    );
    assert!(block.text.contains("ledger"), "空账本也发：{}", block.text);
    assert!(
        block.text.contains("不是新任务"),
        "注明程序重带不是新任务：{}",
        block.text
    );

    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[tasks::NewLedgerNote {
                kind: tasks::LedgerNoteKind::Next,
                topic: "step".to_owned(),
                text: "先跑测试".to_owned(),
            }],
            2,
        )
        .unwrap();
    let task = store.task(&task.id).unwrap().unwrap();
    let block = runtime.compaction_task_block(&task.id, attempt).unwrap();
    assert!(block.text.contains("先跑测试"), "带账本：{}", block.text);
    // 旧 Attempt 不再提供固定块。
    assert!(
        runtime
            .compaction_task_block(&task.id, tasks::AttemptId(attempt.0 + 1000))
            .is_none()
    );
}

#[tokio::test]
async fn task_status_reports_the_user_quote_by_version() {
    let (directory, runtime) = open_runtime();
    let task = runtime_store_task(directory.path(), Vec::new(), false);
    let text = runtime
        .status_text(task.id.as_str())
        .await
        .expect("任务状态");
    assert!(text.contains("用户原话（第 1 版）：账本注入原话"), "{text}");
}

#[test]
fn assignment_puts_user_quotes_first_then_kind_material_then_constraints() {
    let (directory, runtime) = open_runtime();
    let task = runtime_store_task(directory.path(), vec!["账本注入原话".to_owned()], false);
    let assignment = runtime.assignment_text(&task, false);
    let quote_at = assignment.find("user_quotes").expect("原话段");
    let understanding_at = assignment.find("requirement").expect("理解段");
    let dod_at = assignment.find("\"dod\"").expect("DoD 段");
    let note_at = assignment.find("kind_note").expect("补充段");
    let id_at = assignment.find("task_id").expect("任务 id");
    assert!(quote_at < understanding_at, "{assignment}");
    assert!(understanding_at < dod_at, "{assignment}");
    assert!(dod_at < note_at, "{assignment}");
    assert!(note_at < id_at, "{assignment}");
    let package: serde_json::Value = serde_json::from_str(&assignment).expect("assignment JSON");
    assert!(
        package.get("constraints").is_none(),
        "constraints 只留账本：{assignment}"
    );
    assert!(
        assignment.contains("账本注入原话"),
        "约束在账本里：{assignment}"
    );
    assert!(assignment.contains("用户原话"), "{assignment}");
    assert!(
        assignment.contains("Kind 的理解，不是用户原话"),
        "{assignment}"
    );
    assert!(
        assignment.contains("Kind 的补充，不算用户要求"),
        "{assignment}"
    );
}

#[test]
fn old_task_without_quote_keeps_the_plain_assignment() {
    let (directory, runtime) = open_runtime();
    let mut task = runtime_store_task(directory.path(), Vec::new(), false);
    for version in &mut task.versions {
        version.quote = None;
        version.kind_note = None;
    }
    let assignment = runtime.assignment_text(&task, false);
    assert!(assignment.starts_with("{\"task_id\":"), "{assignment}");
    assert!(!assignment.contains("user_quotes"), "{assignment}");
    assert!(!assignment.contains("Kind 的理解"), "{assignment}");
    assert!(!assignment.contains("Kind 的补充"), "{assignment}");
}
#[test]
fn find_user_words_returns_the_latest_matching_user_message() {
    let directory = tempfile::tempdir().unwrap();
    let mut log = session::SessionStore::create(directory.path(), directory.path()).unwrap();
    let id = log.id().clone();
    let first = log
        .append(&[session::EventKind::UserMessage {
            turn: 1,
            step: 0,
            message: model::UserMessage::text_with_request_id(
                "第一句 target",
                "11111111-1111-4111-8111-111111111111",
            )
            .into_message(),
        }])
        .unwrap()[0];
    let second = log
        .append(&[session::EventKind::UserMessage {
            turn: 2,
            step: 0,
            message: model::UserMessage::text_with_request_id(
                "第二句 target",
                "22222222-2222-4222-8222-222222222222",
            )
            .into_message(),
        }])
        .unwrap()[0];
    let (text, seq) = crate::ledger::find_user_words(directory.path(), id.as_str(), "target")
        .unwrap()
        .expect("找到原话");
    assert_eq!(text, "第二句 target");
    assert_eq!(seq, second);
    assert!(second > first);
}

#[test]
fn requirement_change_note_lists_every_version_quote_first() {
    let (directory, runtime) = open_runtime();
    let mut task = runtime_store_task(directory.path(), Vec::new(), false);
    let mut second = task.versions[0].clone();
    second.at += 1;
    second.requirement = Some("第二版要求".to_owned());
    second.dod = Some("第二版 DoD".to_owned());
    second.quote = Some("第二次原话".to_owned());
    task.versions.push(second);
    task.requirement = "第二版要求".to_owned();
    task.dod = "第二版 DoD".to_owned();
    let note = runtime.requirement_change_note(&task, "第二版要求", 2);
    assert!(
        note.starts_with(
            "用户原话：账本注入原话
第 2 版：第二次原话"
        ),
        "{note}"
    );
    assert!(
        note.contains("Kind 的理解") || note.contains("第二版要求"),
        "{note}"
    );
}

#[test]
fn continue_package_lists_every_version_quote_first() {
    let (directory, runtime) = open_runtime();
    let mut task = runtime_store_task(directory.path(), Vec::new(), false);
    let mut second = task.versions[0].clone();
    second.at += 1;
    second.requirement = Some("第二版要求".to_owned());
    second.quote = Some("第二次原话".to_owned());
    second.quote_source = Some(tasks::QuoteSource::UserMessage {
        conversation: "kind-1".to_owned(),
        seq: 2,
    });
    second.kind_note = Some("Kind 的补充".to_owned());
    task.versions.push(second);
    assert_eq!(
        runtime.version_quote_lines(&task),
        vec![
            "用户原话：账本注入原话".to_owned(),
            "第 2 版：第二次原话".to_owned()
        ]
    );
    let package = runtime
        .continue_package(
            &task,
            &tasks::ContinueCause::AfterReport {
                report: tasks::ReportId::from("report-1"),
            },
            None,
        )
        .unwrap();
    assert!(
        package.starts_with("用户原话：账本注入原话\n第 2 版：第二次原话"),
        "{package}"
    );
}

#[test]
fn old_task_continue_package_keeps_plain_requirement_label() {
    let (directory, runtime) = open_runtime();
    let mut task = runtime_store_task(directory.path(), Vec::new(), false);
    for version in &mut task.versions {
        version.quote = None;
        version.kind_note = None;
    }
    let package = runtime
        .continue_package(
            &task,
            &tasks::ContinueCause::UpstreamChanged {
                upstream_report: tasks::ReportId::from("upstream-old"),
            },
            None,
        )
        .unwrap();
    assert!(package.contains("原要求不变："), "{package}");
    assert!(!package.contains("Kind 的理解"), "{package}");
    let package = runtime
        .continue_package(
            &task,
            &tasks::ContinueCause::CycleDue {
                appointment_id: "appointment-old".to_owned(),
                run_id: 1,
            },
            None,
        )
        .unwrap();
    assert!(package.contains("原要求不变："), "{package}");
    assert!(!package.contains("Kind 的理解"), "{package}");
}

#[test]
fn latest_kind_note_scans_back_for_the_most_recent_note() {
    let (directory, runtime) = open_runtime();
    let mut task = runtime_store_task(directory.path(), Vec::new(), false);
    task.versions[0].kind_note = Some("一版的补充".to_owned());
    let mut second = task.versions[0].clone();
    second.at += 1;
    second.quote = Some("第二次原话".to_owned());
    second.kind_note = None;
    task.versions.push(second);
    assert_eq!(runtime.latest_kind_note(&task), Some("一版的补充"));
    let assignment = runtime.assignment_text(&task, false);
    assert!(assignment.contains("一版的补充"), "{assignment}");
    assert_eq!(assignment.matches("一版的补充").count(), 1, "{assignment}");
    let package = runtime
        .continue_package(
            &task,
            &tasks::ContinueCause::UpstreamChanged {
                upstream_report: tasks::ReportId::from("upstream-1"),
            },
            None,
        )
        .unwrap();
    assert!(
        package.contains("Kind 的补充（不算用户要求）：一版的补充"),
        "{package}"
    );
    let note = runtime.requirement_change_note(&task, "第二版要求", 2);
    assert!(
        note.contains("Kind 的补充（不算用户要求）：一版的补充"),
        "{note}"
    );
}

#[test]
fn task_tool_required_keys_are_declared_in_properties() {
    let (_, runtime) = open_runtime();
    let tools: Vec<Box<dyn Tool>> = vec![
        Box::new(TaskAcceptTool::new(runtime.clone())),
        Box::new(TaskGraphProposeTool::new(runtime)),
    ];
    for tool in tools {
        let schema = tool.parameters();
        let properties = schema["properties"].as_object().expect("properties 对象");
        for required in schema["required"].as_array().expect("required 数组") {
            let key = required.as_str().expect("required 项是字符串");
            assert!(
                properties.contains_key(key),
                "{} required 的 {key} 没在 properties 里：{schema}",
                tool.name()
            );
        }
    }
}
