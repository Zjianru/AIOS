use std::sync::{Arc, Mutex, Weak};

use model::{DEFAULT_CONTEXT_WINDOW, Effort};
use tasks::{NewReport, ReportOrigin, ReportState, TaskPolicy};

use super::*;
use crate::root::RootSessionOptions;

fn open_runtime() -> (tempfile::TempDir, Arc<TaskRuntime>) {
    let directory = tempfile::tempdir().unwrap();
    let options = RootSessionOptions {
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
                title: "带约束".to_owned(),
                requirement: "把事做完".to_owned(),
                dod: "有结果".to_owned(),
                cwd: None,
                project: None,
                write_scope: None,
                worktree: false,
                constraints: vec!["别加新依赖".to_owned()],
            },
        )
        .expect("原话约束应通过")
        .0;
    assert_eq!(task.current_constraints(), ["别加新依赖"]);

    let error = runtime
        .accept(
            &context,
            "call-bad",
            TaskAcceptArgs {
                title: "伪造".to_owned(),
                requirement: "把事做完".to_owned(),
                dod: "有结果".to_owned(),
                cwd: None,
                project: None,
                write_scope: None,
                worktree: false,
                constraints: vec!["用户没说过".to_owned()],
            },
        )
        .expect_err("非原话必须整次拒");
    assert!(
        error.message.contains("约束要用用户原话"),
        "{}",
        error.message
    );

    let graph = TaskGraphArgs {
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
