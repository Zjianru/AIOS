//! T024.5a：task-read 只读打开任务当前 Attempt 的会话（头部 + 一页事件）。

use std::path::Path;

use model::{ContentBlock, Message, MessageId, MessageSource, Role, UserMessage};
use service::{Call, Response, RootHost, TASKS_DB_FILE, THIN_TOOLS, TaskReadRequest};
use session::{EventKind, SessionId, SessionStore, TurnEndReason};
use tasks::{
    ConversationId, ModelChoice, NewReport, NewTask, ReportOrigin, ReportState, Store, TaskId,
};

async fn open_host(data: &Path) -> RootHost {
    crate::common::open(crate::common::options(data)).await
}

fn draft(conversation: &ConversationId, cwd: &Path, title: &str) -> NewTask {
    NewTask {
        title: title.to_owned(),
        requirement: "读归档，说明判重规则".to_owned(),
        dod: "给出规则和一个会漏判的例子".to_owned(),
        cwd: cwd.to_path_buf(),
        conversation: conversation.clone(),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: THIN_TOOLS.map(str::to_owned).to_vec(),
    }
}

/// 建一个任务，把当前 Attempt 指到给定的会话。
fn seed_task(data: &Path, cwd: &Path, conversation: &str, title: &str) -> TaskId {
    let store = Store::open(data.join(TASKS_DB_FILE)).expect("任务库");
    let task = store
        .create_task(draft(&ConversationId::new(conversation), cwd, title), 1_000)
        .expect("建任务");
    let attempt = store.begin_attempt(&task.id, 1_001).expect("开 Attempt");
    store
        .attach_session(
            &task.id,
            attempt,
            ConversationId::new(conversation.to_owned()),
        )
        .expect("写回会话");
    task.id
}

fn assignment(text: &str) -> EventKind {
    let message = UserMessage::text_with_request_id(text, "assignment:task-seed").into_message();
    EventKind::UserMessage {
        turn: 1,
        step: 1,
        message,
    }
}

fn answer(turn: u64, step: u64, text: &str) -> EventKind {
    EventKind::AssistantMessage {
        turn,
        step,
        message: Message::text(Role::Assistant, text),
        stream: Vec::new(),
        usage: None,
        interrupted: None,
        effort: None,
        effort_wanted: None,
    }
}

/// 一条带工具调用和最后回答的会话；返回会话 id。
fn seed_session(data: &Path, cwd: &Path, turns: u64) -> SessionId {
    let mut store = SessionStore::create(data, cwd).expect("建会话");
    let id = store.id().clone();
    let mut kinds = Vec::new();
    for turn in 1..=turns {
        kinds.push(EventKind::TurnStart { turn });
        if turn == 1 {
            kinds.push(assignment(
                "{\"task_id\":\"task-seed\",\"title\":\"查判重\"}",
            ));
            kinds.push(EventKind::ToolCall {
                turn,
                step: 1,
                call_id: "call-read-1".to_owned(),
                name: "read".to_owned(),
                arguments: "{\"file_path\":\"hello.txt\"}".to_owned(),
            });
            let mut result = Message::with_source(
                MessageId::generate(),
                Role::Tool,
                vec![ContentBlock::text("hello from a task")],
                MessageSource::Tool {
                    call_id: "call-read-1".to_owned(),
                },
            );
            result.tool_call_id = Some("call-read-1".to_owned());
            kinds.push(EventKind::ToolResult {
                turn,
                step: 1,
                message: result,
                error: None,
                meta: None,
            });
            kinds.push(answer(turn, 2, "已经查完，判重用的是原始链接。"));
        } else {
            for step in 1..=25 {
                kinds.push(answer(turn, step, &format!("第 {turn} 轮第 {step} 步")));
            }
        }
        kinds.push(EventKind::TurnEnd {
            turn,
            reason: TurnEndReason::Completed,
        });
    }
    store.append(&kinds).expect("写会话事件");
    id
}

async fn read(host: &RootHost, task_id: &TaskId, before_seq: Option<u64>) -> Response {
    host.dispatch(Call::TaskRead(TaskReadRequest {
        task_id: task_id.as_str().to_owned(),
        before_seq,
    }))
    .await
    .expect("task-read 要成功")
}

#[tokio::test]
async fn returns_the_header_and_the_tail_page() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    // 任务泵停下：不要再开真的 ThinAgent，TaskRead 只读我们写好的会话。
    host.task_runtime().expect("任务执行者").set_paused(true);

    let conversation = seed_session(data.path(), cwd.path(), 1);
    let task_id = seed_task(
        data.path(),
        cwd.path(),
        conversation.as_str(),
        "查一下新闻归档里重复链接的判重规则",
    );
    let store = Store::open(data.path().join(TASKS_DB_FILE)).expect("任务库");
    let attempt = store
        .task(&task_id)
        .expect("读任务")
        .expect("任务存在")
        .latest_attempt()
        .expect("Attempt")
        .id;
    store
        .record_report(
            &task_id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "判重用的是原始链接。".to_owned(),
                question: None,
            },
            2_000,
        )
        .expect("写报告");

    let Response::TaskRead(read) = read(&host, &task_id, None).await else {
        panic!("task-read 响应");
    };

    assert_eq!(read.task.id, task_id.as_str());
    assert_eq!(read.task.title, "查一下新闻归档里重复链接的判重规则");
    assert_eq!(read.task.requirement, "读归档，说明判重规则");
    assert_eq!(read.task.dod, "给出规则和一个会漏判的例子");
    assert_eq!(read.task.cwd, cwd.path().to_string_lossy());
    assert_eq!(read.task.model, "deepseek-flash");
    assert_eq!(read.task.tools, THIN_TOOLS.map(str::to_owned).to_vec());
    assert_eq!(read.task.attempts, 1);
    assert_eq!(
        read.task.reported_at,
        Some(2_000),
        "头部带最近一份报告的写入时间（页面据此收起还没 turn/end 的最后一轮）"
    );

    // 一页事件按日志顺序：assignment（页面负责不画成气泡）、工具调用、最后回答都在。
    let types: Vec<&str> = read
        .events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert_eq!(
        types,
        vec![
            "turn/start",
            "user/message",
            "tool/call",
            "tool/result",
            "assistant/message",
            "turn/end",
        ],
        "事件顺序照日志"
    );
    let assignment = &read.events[1].data["message"];
    assert_eq!(assignment["source"]["rpcId"], "assignment:task-seed");
    assert_eq!(read.events[2].data["name"], "read");
    assert!(
        read.events[4].data["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("判重用的是原始链接")
    );
    assert_eq!(read.last_seq, read.events.last().unwrap().seq);
}

#[tokio::test]
async fn pages_backwards_and_last_seq_stays_at_the_log_end() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime().expect("任务执行者").set_paused(true);

    // 三轮共 76 条消息；一页 50 条，所以第一页只到日志尾部，往回翻才能看到第一轮。
    let conversation = seed_session(data.path(), cwd.path(), 3);
    let task_id = seed_task(data.path(), cwd.path(), conversation.as_str(), "分页");
    let all = SessionStore::open_read_only(data.path(), &conversation)
        .expect("只读打开")
        .events()
        .to_vec();
    let log_end = all.last().expect("日志末端").seq;

    let Response::TaskRead(first) = read(&host, &task_id, None).await else {
        panic!("task-read 响应");
    };
    assert_eq!(first.last_seq, log_end);
    assert!(
        !first.events.iter().any(|event| event.seq == all[0].seq),
        "第一页不该包含日志最早的事件"
    );
    let first_seq = first.events.first().expect("第一页非空").seq;
    assert!(first_seq > all[0].seq);

    let Response::TaskRead(second) = read(&host, &task_id, Some(first_seq)).await else {
        panic!("task-read 响应");
    };
    assert_eq!(second.last_seq, log_end, "往上翻页不改日志末端");
    assert!(!second.events.is_empty(), "更早的一页非空");
    assert!(
        second.events.iter().all(|event| event.seq < first_seq),
        "beforeSeq 不含本条，两页不重叠"
    );
    assert_eq!(
        second.events.last().expect("更早的一页").seq,
        first_seq - 1,
        "第二页紧接第一页"
    );
    assert!(
        second
            .events
            .iter()
            .any(|event| event.event_type == "user/message"
                && event.data["message"]["source"]["rpcId"] == "assignment:task-seed"),
        "往回翻能看到第一轮的 assignment"
    );
    assert!(
        second
            .events
            .iter()
            .any(|event| event.event_type == "tool/call" && event.data["name"] == "read"),
        "往回翻能看到工具调用"
    );

    let second_first = second.events.first().expect("第二页").seq;
    let Response::TaskRead(empty) = read(&host, &task_id, Some(second_first)).await else {
        panic!("task-read 响应");
    };
    assert!(empty.events.is_empty(), "日志起点以前没有事件");
    assert_eq!(empty.last_seq, log_end);
}

#[tokio::test]
async fn missing_task_is_a_readable_error() {
    let data = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime().expect("任务执行者").set_paused(true);

    let error = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: "no-such-task".to_owned(),
            before_seq: None,
        }))
        .await
        .expect_err("不存在的任务要报错");
    assert_eq!(error.code().as_str(), "task/unavailable");
    assert!(
        error.to_string().contains("找不到这个任务"),
        "错误要能读：{error}"
    );
}

#[tokio::test]
async fn unopened_task_is_a_readable_error() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime().expect("任务执行者").set_paused(true);

    let store = Store::open(data.path().join(TASKS_DB_FILE)).expect("任务库");
    let task = store
        .create_task(
            draft(&ConversationId::new("kind-test"), cwd.path(), "还没开跑"),
            1_000,
        )
        .expect("建任务");
    let error = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect_err("没有 Attempt 的任务要报错");
    assert_eq!(error.code().as_str(), "task/unavailable");
    assert!(
        error.to_string().contains("还没开跑"),
        "错误要能读：{error}"
    );
}

#[tokio::test]
async fn missing_session_is_a_readable_error() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime().expect("任务执行者").set_paused(true);

    // 合法的会话 id，但文件不存在。
    let missing = "11111111-1111-4111-8111-111111111111";
    let task_id = seed_task(data.path(), cwd.path(), missing, "会话没了");
    let error = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task_id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect_err("会话日志缺失要报错");
    assert_eq!(error.code().as_str(), "task/unavailable");
    assert!(
        error.to_string().contains("任务对话打不开"),
        "错误要能读：{error}"
    );
    assert!(
        error.to_string().contains("会话日志不存在"),
        "原因要带出来：{error}"
    );
}

#[tokio::test]
async fn corrupt_session_is_a_readable_error() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime().expect("任务执行者").set_paused(true);

    let conversation = "22222222-2222-4222-8222-222222222222";
    let sessions = data.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join(format!("{conversation}.jsonl")), "{ broken\n").unwrap();
    let task_id = seed_task(data.path(), cwd.path(), conversation, "会话坏了");
    let error = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task_id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect_err("坏会话要报错");
    assert_eq!(error.code().as_str(), "task/unavailable");
    assert!(
        error.to_string().contains("任务对话打不开"),
        "错误要能读：{error}"
    );
}
