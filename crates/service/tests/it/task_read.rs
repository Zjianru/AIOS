//! T024.5a：task-read 只读打开任务当前 Attempt 的会话（头部 + 一页事件）。

use std::path::Path;

use model::{ContentBlock, Message, MessageId, MessageSource, Role, UserMessage};
use service::{Call, Response, RootHost, TASKS_DB_FILE, THIN_TOOLS, TaskReadRequest};
use session::{EventKind, SessionId, SessionStore, TurnEndReason};
use tasks::{
    AttemptId, ConversationId, ModelChoice, NewReport, NewTask, ReportOrigin, ReportState, Store,
    TaskId, TaskPolicy,
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
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

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

/// 一条只有 turn/start + 一条回答 + turn/end 的会话；用来把日志撑过一页的预算。
fn seed_plain_session(data: &Path, cwd: &Path, turns: u64) -> SessionId {
    let mut store = SessionStore::create(data, cwd).expect("建会话");
    let id = store.id().clone();
    let mut kinds = Vec::new();
    for turn in 1..=turns {
        kinds.push(EventKind::TurnStart { turn });
        kinds.push(answer(turn, 1, &format!("第 {turn} 轮回答")));
        kinds.push(EventKind::TurnEnd {
            turn,
            reason: TurnEndReason::Completed,
        });
    }
    store.append(&kinds).expect("写会话事件");
    id
}

#[tokio::test]
async fn the_tail_page_starts_at_a_turn_boundary_instead_of_cutting_at_fifty_messages() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    // 三轮共 52 条消息：50 条消息的下限落在第 2 轮开头，页首必须是一轮的 turn/start，
    // 不能像 maxMessages=50 那样在第 50 条消息处从一轮中间切开。
    let conversation = seed_session(data.path(), cwd.path(), 3);
    let task_id = seed_task(data.path(), cwd.path(), conversation.as_str(), "一整页");
    let all = SessionStore::open_read_only(data.path(), &conversation)
        .expect("只读打开")
        .events()
        .to_vec();

    let Response::TaskRead(page) = read(&host, &task_id, None).await else {
        panic!("task-read 响应");
    };
    assert_eq!(page.last_seq, all.last().unwrap().seq);
    assert_eq!(
        page.events.first().expect("第一页非空").event_type,
        "turn/start",
        "页首落在轮起点，不从一轮中间切"
    );
    assert_eq!(
        page.events
            .iter()
            .filter(|event| event.event_type == "assistant/message")
            .count(),
        50,
        "按 50 条消息、2 轮的下限对齐"
    );
    let first_seq = page.events.first().unwrap().seq;
    assert!(first_seq > all[0].seq, "更早的第一轮在这一页之外");

    let Response::TaskRead(second) = read(&host, &task_id, Some(first_seq)).await else {
        panic!("task-read 响应");
    };
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
}

#[tokio::test]
async fn pages_backwards_at_turn_boundaries_and_last_seq_stays_at_the_log_end() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    // 600 轮、每轮一条消息：一页回到「50 条消息 / 2 轮」的对齐点，页首必须是一轮的 turn/start。
    let conversation = seed_plain_session(data.path(), cwd.path(), 600);
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
    let first_event = first.events.first().expect("第一页非空");
    assert_eq!(
        first_event.event_type, "turn/start",
        "页首落在轮起点，不从一轮中间切"
    );
    assert!(first_event.seq > all[0].seq, "600 轮的日志不该一页给全");
    assert_eq!(
        first
            .events
            .iter()
            .filter(|event| event.event_type == "assistant/message")
            .count(),
        50,
        "一页按 50 条消息、2 轮的下限对齐"
    );
    let first_seq = first_event.seq;

    let Response::TaskRead(second) = read(&host, &task_id, Some(first_seq)).await else {
        panic!("task-read 响应");
    };
    assert_eq!(second.last_seq, log_end, "往上翻页不改日志末端");
    assert!(!second.events.is_empty(), "更早的一页非空");
    assert_eq!(
        second.events.first().expect("第二页").event_type,
        "turn/start",
        "每一页都落在轮起点"
    );
    assert!(
        second.events.iter().all(|event| event.seq < first_seq),
        "beforeSeq 不含本条，两页不重叠"
    );
    assert_eq!(
        second.events.last().expect("更早的一页").seq,
        first_seq - 1,
        "第二页紧接第一页"
    );

    let second_first = second.events.first().expect("第二页").seq;
    let Response::TaskRead(third) = read(&host, &task_id, Some(second_first)).await else {
        panic!("task-read 响应");
    };
    assert!(!third.events.is_empty(), "600 轮的日志还有更早的一页");
    assert_eq!(
        third.events.first().expect("第三页").event_type,
        "turn/start",
        "每一页都落在轮起点"
    );
    assert!(
        third.events.iter().all(|event| event.seq < second_first),
        "beforeSeq 不含本条，两页不重叠"
    );
    assert_eq!(third.last_seq, log_end);
}

#[tokio::test]
async fn waiting_report_does_not_close_the_turn_for_the_page() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let conversation = seed_session(data.path(), cwd.path(), 1);
    let task_id = seed_task(data.path(), cwd.path(), conversation.as_str(), "等报告");
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
                state: ReportState::Waiting,
                origin: ReportOrigin::Executor,
                summary: "我还在查，别急。".to_owned(),
                question: None,
            },
            2_000,
        )
        .expect("写等待报告");

    let Response::TaskRead(read) = read(&host, &task_id, None).await else {
        panic!("task-read 响应");
    };
    assert_eq!(
        read.task.reported_at, None,
        "waiting 不是终态：之后的 task_status 会起新的一轮，页面不能拿它收掉本轮"
    );
}

#[tokio::test]
async fn missing_task_is_a_readable_error() {
    let data = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

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
async fn unopened_task_reads_as_a_waiting_header_with_no_events() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let store = Store::open(data.path().join(TASKS_DB_FILE)).expect("任务库");
    let task = store
        .create_task(
            draft(&ConversationId::new("kind-test"), cwd.path(), "还没开跑"),
            1_000,
        )
        .expect("建任务");
    let Response::TaskRead(read) = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("没开跑的任务也能读")
    else {
        panic!("task-read 响应");
    };
    assert_eq!(read.task.id, task.id.as_str());
    assert_eq!(read.task.title, "还没开跑");
    assert_eq!(read.task.attempts, 0, "attempts: 0");
    assert!(!read.task.running && !read.task.stopped && !read.task.interrupted);
    assert!(read.task.report.is_none(), "没报告");
    assert!(!read.task.review && !read.task.accepted);
    assert!(read.events.is_empty(), "事件为空");
    assert_eq!(read.last_seq, 0, "lastSeq 为 0");
    let stats = read.stats.expect("未开跑也带 stats");
    assert_eq!(stats.turns, 0);
    assert_eq!(stats.steps, 0);
}

#[tokio::test]
async fn missing_session_is_a_readable_error() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

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
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

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

#[tokio::test]
async fn task_stream_replays_after_last_seq_and_is_not_live_when_the_task_is_not_running() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let conversation = seed_session(data.path(), cwd.path(), 2);
    let task_id = seed_task(data.path(), cwd.path(), conversation.as_str(), "流");
    let all = SessionStore::open_read_only(data.path(), &conversation)
        .expect("只读打开")
        .events()
        .to_vec();
    let after = all[all.len() - 3].seq;

    let stream = host
        .task_stream(task_id.as_str(), after)
        .await
        .expect("任务流要能打开");
    assert_eq!(stream.session_id, conversation.as_str());
    assert!(stream.live.is_none(), "任务没在跑：只补齐，不出 live 帧");
    assert!(
        stream.follow.events.iter().all(|event| event.seq > after),
        "只补 lastSeq 之后的事件"
    );
    assert_eq!(stream.follow.last_seq, all.last().unwrap().seq);
    assert_eq!(
        stream.follow.events.last().unwrap().seq,
        all.last().unwrap().seq,
        "补齐到日志末端"
    );
}

#[tokio::test]
async fn task_stream_for_a_missing_task_is_a_readable_error() {
    let data = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let error = match host.task_stream("no-such-task", 0).await {
        Ok(_) => panic!("不存在的任务要报错"),
        Err(error) => error,
    };
    assert_eq!(error.code().as_str(), "task/unavailable");
    assert!(
        error.to_string().contains("找不到这个任务"),
        "错误要能读：{error}"
    );
}

#[tokio::test]
async fn task_stream_for_an_unopened_task_is_empty_without_live() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let store = Store::open(data.path().join(TASKS_DB_FILE)).expect("任务库");
    let task = store
        .create_task(
            draft(&ConversationId::new("kind-test"), cwd.path(), "还没开跑"),
            1_000,
        )
        .expect("建任务");
    let stream = host
        .task_stream(task.id.as_str(), 0)
        .await
        .expect("没开跑的任务也能打开任务流");
    assert!(stream.follow.events.is_empty(), "只回空补齐");
    assert_eq!(stream.follow.last_seq, 0);
    assert!(stream.live.is_none(), "不出 live 帧");
}

/// T051：stats 由服务端 task-read 给（和 Kind status 同一个 session_stats），页面只显示。
/// 这里验证：目录价（模型行没填价时）也算出花费、缓存读写 token 计入、事件超过一页也按全部事件算。
#[tokio::test]
async fn stats_are_computed_from_all_events_and_catalog_prices() {
    use model::{Usage, catalog};
    use service::stats;

    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    // Keyring 里放一条 deepseek 连接（模型行没填价 → 花费必须回退目录标价）。
    let keyring = credentials::Keyring::open(
        data.path().to_path_buf(),
        Some(&data.path().join("identity.txt")),
    )
    .expect("keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "https://api.deepseek.com/anthropic".to_owned(),
            api_key: "test-key".to_owned(),
            preset: Some("deepseek".to_owned()),
            models: None,
            auth_header: None,
        })
        .expect("存连接");

    // 三轮共 51 条消息（超过一页 50 条的下限）：只有第 1 轮带 usage（含缓存读写）。
    let mut store = SessionStore::create(data.path(), cwd.path()).expect("建会话");
    let conversation = store.id().clone();
    let mut kinds = Vec::new();
    for turn in 1..=3 {
        kinds.push(EventKind::TurnStart { turn });
        if turn == 1 {
            kinds.push(assignment(
                "{\"task_id\":\"task-stats\",\"title\":\"算统计\"}",
            ));
            kinds.push(EventKind::AssistantMessage {
                turn,
                step: 1,
                message: Message::text(Role::Assistant, "带用量的回答"),
                stream: Vec::new(),
                usage: Some(Usage {
                    input_tokens: Some(1_000),
                    output_tokens: Some(500),
                    cache_read_input_tokens: Some(250_000),
                    // 目录里 deepseek-flash 只有 cache_read 价、没有 cache_write 价：写缓存有量时
                    // 这一轮本就无价（T012 规则），所以这里缓存写留 0，专测「缓存读计入 + 目录价折算」。
                    cache_creation_input_tokens: Some(0),
                }),
                interrupted: None,
                effort: None,
                effort_wanted: None,
            });
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
    let task_id = seed_task(data.path(), cwd.path(), conversation.as_str(), "算统计");

    let Response::TaskRead(read) = read(&host, &task_id, None).await else {
        panic!("task-read 响应");
    };
    let stats = read.stats.expect("task-read 要带 stats");
    assert_eq!(stats.turns, 3, "轮数按全部事件算，不受分页影响");
    assert_eq!(stats.input_tokens, 1_000);
    assert_eq!(stats.output_tokens, 500);
    assert_eq!(stats.cache_read_tokens, 250_000, "缓存读 token 要计入");
    assert_eq!(stats.cache_write_tokens, 0);
    assert!(
        read.events
            .iter()
            .filter(|event| event.event_type == "assistant/message")
            .count()
            < 52,
        "事件确实被分页截过（页内不是全部）"
    );
    assert!(
        stats.cost_usd.is_some() && stats.cost_usd.unwrap() > 0.0,
        "模型行没填价时花费要回退目录标价算出来：{:?}",
        stats.cost_usd
    );
    assert!(!stats.unpriced, "目录有价时不算 unpriced");

    // 和 Kind status 同一函数：直接用服务端 session_stats 复算，选中的字段应一致。
    let snapshot = SessionStore::open_read_only(data.path(), &conversation).expect("只读打开");
    let events = snapshot.events();
    let view = keyring.view();
    let selection = session::ModelSelection {
        connection: "deepseek".to_owned(),
        model: "deepseek-flash".to_owned(),
        effort_wanted: "high".to_owned(),
    };
    let expected = stats::session_stats(
        events,
        &view.connections,
        &catalog::snapshot(),
        &selection,
        stats.context_used,
        stats.context_window,
    );
    assert_eq!(stats.turns, expected.turns);
    assert_eq!(stats.steps, expected.steps);
    assert_eq!(stats.input_tokens, expected.input_tokens);
    assert_eq!(stats.output_tokens, expected.output_tokens);
    assert_eq!(stats.cache_read_tokens, expected.cache_read_tokens);
    assert_eq!(stats.cache_write_tokens, expected.cache_write_tokens);
    assert_eq!(stats.cost_usd, expected.cost_usd, "花费和 Kind 同一算法");
    assert_eq!(stats.unpriced, expected.unpriced);
}

/// T051：页头词表要的事实由服务端给：report/review/accepted。review 与 relay 的 card_state:review
/// 共用 is_review_report；accepted 只认「当前版本 + 这份报告」的接受记录。
async fn read_for_report(host: &RootHost, task_id: &TaskId) -> service::TaskReadResponse {
    let Response::TaskRead(read) = read(host, task_id, None).await else {
        panic!("task-read 响应");
    };
    read
}

fn seed_write_task(data: &Path, cwd: &Path, conversation: &str) -> (TaskId, AttemptId) {
    let store = Store::open(data.join(TASKS_DB_FILE)).expect("任务库");
    let conversation = ConversationId::new(conversation.to_owned());
    let task = store
        .create_task_with(
            draft(&conversation, cwd, "写任务"),
            TaskPolicy {
                write_scope: Some("notes.md".to_owned()),
                worktree: false,
                bash: false,
            },
            1_000,
        )
        .expect("建写任务");
    let attempt = store.begin_attempt(&task.id, 1_001).expect("开 Attempt");
    store
        .attach_session(&task.id, attempt, conversation)
        .expect("写回会话");
    (task.id, attempt)
}

#[tokio::test]
async fn header_carries_report_review_and_acceptance() {
    use service::{TaskReadReportOrigin, TaskReadReportState};

    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);
    let store = Store::open(data.path().join(TASKS_DB_FILE)).expect("任务库");

    // failed / agent：页头能拿到「失败」要的事实。
    let conversation = seed_session(data.path(), cwd.path(), 1);
    let (task_id, attempt) = seed_write_task(data.path(), cwd.path(), conversation.as_str());
    store
        .record_report(
            &task_id,
            attempt,
            NewReport {
                state: ReportState::Failed,
                origin: ReportOrigin::Agent,
                summary: "跑挂了".to_owned(),
                question: None,
            },
            2_000,
        )
        .expect("写报告");
    let failed = read_for_report(&host, &task_id).await;
    let report = failed.task.report.expect("report");
    assert_eq!(report.state, TaskReadReportState::Failed);
    assert_eq!(report.origin, TaskReadReportOrigin::Agent);
    assert!(!failed.task.review, "失败不是待你核");
    assert!(!failed.task.accepted);
    assert_eq!(failed.task.reported_at, Some(2_000));

    // blocked / package：页头能区分「没开跑」和「卡住了」。
    // 一次 Attempt 只允许一份终态报告，所以用新的任务/Attempt。
    let conversation = seed_session(data.path(), cwd.path(), 1);
    let (task_id, attempt) = seed_write_task(data.path(), cwd.path(), conversation.as_str());
    store
        .record_report(
            &task_id,
            attempt,
            NewReport {
                state: ReportState::Blocked,
                origin: ReportOrigin::Package,
                summary: "缺任务包".to_owned(),
                question: None,
            },
            2_100,
        )
        .expect("写报告");
    let blocked = read_for_report(&host, &task_id).await;
    let report = blocked.task.report.expect("report");
    assert_eq!(report.state, TaskReadReportState::Blocked);
    assert_eq!(report.origin, TaskReadReportOrigin::Package);
    assert!(!blocked.task.review);

    // 写任务当前版本的 ready：review = true（和 relay 加 card_state:review 同一判断）。
    let conversation = seed_session(data.path(), cwd.path(), 1);
    let (task_id, attempt) = seed_write_task(data.path(), cwd.path(), conversation.as_str());
    store
        .record_report(
            &task_id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "写完了".to_owned(),
                question: None,
            },
            2_200,
        )
        .expect("写报告");
    let ready = read_for_report(&host, &task_id).await;
    assert_eq!(
        ready.task.report.expect("report").state,
        TaskReadReportState::Ready
    );
    assert!(ready.task.review, "写任务当前版本 ready → 待你核");
    assert!(!ready.task.accepted);

    // 接受以后：accepted = true（绑定当前版本和这份报告）。
    store.accept(&task_id, 3_000).expect("接受");
    let accepted = read_for_report(&host, &task_id).await;
    assert!(accepted.task.review && accepted.task.accepted, "已接受");

    // 要求升版以后：旧版本的 ready 不再算「待你核」，接受记录也不再匹配。
    store
        .update_write_scope(&task_id, "notes.md", 4_000)
        .expect("升版");
    let bumped = read_for_report(&host, &task_id).await;
    assert!(!bumped.task.review, "旧版本 ready 不待你核");
    assert!(!bumped.task.accepted, "旧版本的接受不认");
}

/// T051：只读任务即使 ready 也不是「待你核」（review 只给写任务）。
#[tokio::test]
async fn read_only_ready_report_is_not_review() {
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(data.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let conversation = seed_session(data.path(), cwd.path(), 1);
    let task_id = seed_task(data.path(), cwd.path(), conversation.as_str(), "只读调查");
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
                summary: "查完了".to_owned(),
                question: None,
            },
            2_000,
        )
        .expect("写报告");
    let read = read_for_report(&host, &task_id).await;
    assert!(read.task.report.is_some());
    assert!(!read.task.review, "只读任务 ready 不是待你核");
    assert!(!read.task.accepted);
}
