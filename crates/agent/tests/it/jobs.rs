//! 接缝 1：后台任务完成通知与 JobCompleted 事件（测试清单第 4 条 + 票面加项）。



use std::fs;
use std::time::Duration;

use agent::Delivery;
use crate::common::{FINAL_TEXT_SSE, SequenceResponder, setup_agent, sse, tool_calls_with_inputs_sse};
use model::UserMessage;
use serde_json::json;
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

#[tokio::test]
async fn background_job_completion_notifies_next_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "bash",
                json!({
                    "description": "test command", "command": "while [ ! -f go ]; do :; done; printf 'done\n'",
                    "run_in_background": true
                }),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, _data_dir, cwd, _id) = setup_agent(&server.uri(), ToolSet::builtin()).await;

    agent
        .send(UserMessage::text("后台跑一下"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "turn1 应有两次请求"
    );

    let mut events = agent.subscribe();
    fs::write(cwd.path().join("go"), "").unwrap();

    let notified = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if server.received_requests().await.unwrap().len() >= 3 {
                return true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or(false);
    assert!(notified, "完成通知没有开启下一轮请求");
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let last = String::from_utf8(requests.last().unwrap().body.clone()).unwrap();
    assert!(last.contains("background job bash-"), "{last}");
    assert!(last.contains("finished"), "{last}");

    let mut saw_job_completed = false;
    while let Ok(event) = events.try_recv() {
        if matches!(event, agent::AgentEvent::JobCompleted { .. }) {
            saw_job_completed = true;
        }
    }
    assert!(saw_job_completed, "缺 JobCompleted 事件");
}

/// 动态 mock：第 1 次回 bash run_in_background，第 2 次从请求体里取出 job id 回
/// job_output(wait)，之后回 FINAL。这样「忙碌→NextStep」的测试不依赖固定 id。
struct BusyScriptResponder {
    step: std::sync::Mutex<usize>,
}

impl wiremock::Respond for BusyScriptResponder {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let mut step = self.step.lock().expect("BusyScript 锁被毒化");
        let current = *step;
        *step += 1;
        drop(step);
        match current {
            0 => sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "bash",
                json!({
                    "description": "test command", "command": "while [ ! -f go ]; do :; done; printf 'job done\n'",
                    "run_in_background": true
                }),
            )])),
            1 => {
                let body = String::from_utf8_lossy(&request.body);
                let marker = "started background job ";
                let Some(start) = body.find(marker) else {
                    return sse(FINAL_TEXT_SSE.to_owned());
                };
                let rest = &body[start + marker.len()..];
                let end = rest
                    .find(|character: char| {
                        !(character.is_ascii_alphanumeric() || character == '-')
                    })
                    .unwrap_or(rest.len());
                let job_id = &rest[..end];
                sse(tool_calls_with_inputs_sse(&[(
                    "call_1",
                    "job_output",
                    json!({"job_id": job_id, "wait": true, "timeout_ms": 5000}),
                )]))
            }
            _ => sse(FINAL_TEXT_SSE.to_owned()),
        }
    }
}

#[tokio::test]
async fn job_completion_during_turn_arrives_as_next_step() {
    // 忙碌路径：turn 内 job 完成 → 通知进 NextStep（同一轮的后续请求带上），不另开一轮。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(BusyScriptResponder {
            step: std::sync::Mutex::new(0),
        })
        .mount(&server)
        .await;
    let (agent, data_dir, cwd, session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;

    let cwd_path = cwd.path().to_path_buf();
    let notifier = tokio::spawn(async move {
        // 等第一次请求已经在处理（turn 已开始）再放行后台任务。
        for _ in 0..5000 {
            if cwd_path.join("go").exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        std::fs::write(cwd_path.join("go"), "").unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    std::fs::write(cwd.path().join("go"), "").unwrap();
    notifier.abort();

    agent
        .send(UserMessage::text("一边跑后台一边干活"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert!(requests.len() >= 3, "至少 3 次请求：{}", requests.len());
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    let notified_step = bodies[1..]
        .iter()
        .position(|body| body.contains("background job bash-"));
    assert!(
        notified_step.is_some(),
        "忙碌路径的通知应在同一轮的后续请求（NextStep）"
    );
    assert!(bodies[1..].iter().any(|body| body.contains("finished")));

    // 同一轮内送达：日志里只有 1 个 turn/start，没有因通知另开一轮。
    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let turns = store
        .events()
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::TurnStart { .. }))
        .count();
    assert_eq!(turns, 1, "忙碌路径不应另开一轮");
}

/// 从请求体里取出「started background job <id>」的 id。
fn started_job_id(body: &str) -> String {
    let marker = "started background job ";
    let start = body.find(marker).expect("请求里有后台任务 id") + marker.len();
    let rest = &body[start..];
    let end = rest
        .find(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .unwrap_or(rest.len());
    rest[..end].to_owned()
}

/// 从请求体里取出完成通知「background job <id> ...」的 id。
fn noticed_job_id(body: &str) -> String {
    let marker = "background job ";
    let start = body.find(marker).expect("请求里有完成通知") + marker.len();
    let rest = &body[start..];
    let end = rest
        .find(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .unwrap_or(rest.len());
    rest[..end].to_owned()
}

/// 请求体里各条消息的 text 块（原样解 JSON；不靠转义串匹配换行）。
fn request_message_texts(body: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_str(body).expect("请求体是 JSON");
    let mut texts = Vec::new();
    if let Some(messages) = value["messages"].as_array() {
        for message in messages {
            if let Some(content) = message["content"].as_array() {
                for block in content {
                    if let Some(text) = block["text"].as_str() {
                        texts.push(text.to_owned());
                    }
                }
            }
        }
    }
    texts
}

/// 动态 mock：完成通知到达后的下一轮让模型 job_output 读一次这个任务。
struct NoticeThenReadResponder {
    step: std::sync::Mutex<usize>,
    command: &'static str,
}

impl wiremock::Respond for NoticeThenReadResponder {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let mut step = self.step.lock().expect("NoticeThenRead 锁被毒化");
        let current = *step;
        *step += 1;
        drop(step);
        match current {
            0 => sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "bash",
                json!({
                    "description": "test command", "command": self.command, "run_in_background": true
                }),
            )])),
            2 => {
                let body = String::from_utf8_lossy(&request.body);
                let job_id = noticed_job_id(&body);
                sse(tool_calls_with_inputs_sse(&[(
                    "call_1",
                    "job_output",
                    json!({"job_id": job_id}),
                )]))
            }
            _ => sse(FINAL_TEXT_SSE.to_owned()),
        }
    }
}

/// 轮询到 mock 收到至少 n 次请求（受控时序；不靠固定 sleep 猜时长）。
async fn wait_requests(server: &MockServer, n: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while server.received_requests().await.unwrap().len() < n {
        assert!(
            std::time::Instant::now() < deadline,
            "mock 没有在 5s 内收到 {n} 次请求"
        );
        tokio::task::yield_now().await;
    }
}
/// T006.9（D4）：前台 bash 也照 DSH 记 job/registered + job/settled（有 registry 时每次调用都登记）。
#[tokio::test]
async fn foreground_bash_records_job_lifecycle_events() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "bash",
                json!({"description": "test command", "command": "printf 'hi'"}),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    agent
        .send(UserMessage::text("前台跑一下"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    // 生命周期事件由后台任务异步写日志，轮询到两条齐。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let (registered, settled) = loop {
        let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
        let events = store.events();
        let registered = events
            .iter()
            .find(|event| matches!(event.kind, session::EventKind::JobRegistered { .. }))
            .cloned();
        let settled = events
            .iter()
            .find(|event| matches!(event.kind, session::EventKind::JobSettled { .. }))
            .cloned();
        if let (Some(registered), Some(settled)) = (registered, settled) {
            break (registered, settled);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "5s 内没有写齐 job/registered 与 job/settled"
        );
        tokio::task::yield_now().await;
    };
    assert!(
        registered.seq < settled.seq,
        "job/registered 先于 job/settled"
    );
    // T006.9c（DSH registry.start）：job/registered 先于该工具自己的 tool/result 提交。
    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let events = store.events().to_vec();
    let result_seq = events
        .iter()
        .find(|event| matches!(event.kind, session::EventKind::ToolResult { .. }))
        .expect("有 tool/result")
        .seq;
    assert!(
        registered.seq < result_seq,
        "job/registered 先于 tool/result（registered={} result={}）",
        registered.seq,
        result_seq
    );
    let session::EventKind::JobRegistered {
        job_id,
        owner,
        kind,
        label,
        started_at,
        tool_call,
    } = registered.kind
    else {
        unreachable!()
    };
    assert_eq!(owner.as_deref(), Some(session_id.as_str()));
    assert_eq!(kind, "bash");
    assert_eq!(label, "printf 'hi'");
    assert!(started_at > 0);
    let tool_call = tool_call.expect("前台 bash 的 job/registered 带 toolCall");
    assert_eq!(tool_call.call_id, "call_0");
    assert_eq!(tool_call.root_call_id, "call_0");
    let session::EventKind::JobSettled {
        job_id: settled_job_id,
        status,
        cause,
        producer_reported,
        detail,
        finished_at,
        ..
    } = settled.kind
    else {
        unreachable!()
    };
    assert_eq!(settled_job_id, job_id);
    assert_eq!(status, "completed");
    assert_eq!(cause, "producer");
    assert!(producer_reported);
    assert_eq!(detail.as_deref(), Some("exit code: 0"));
    assert!(finished_at >= started_at);
    // 日志记录不进派生对话。
    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let derived = store.derive_messages();
    assert!(
        derived
            .iter()
            .all(|message| !format!("{message:?}").contains("job/")),
        "生命周期事件不进派生视图"
    );
}
/// T006.9（D3）：完成通知照 DSH tool-jobs 文案——能内联就 `Output:`，并消耗这次游标读。
#[tokio::test]
async fn background_job_completion_notice_matches_dsh_inline() {
    let command = "while [ ! -f go ]; do :; done; printf 'done\\n'";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(NoticeThenReadResponder {
            step: std::sync::Mutex::new(0),
            command,
        })
        .mount(&server)
        .await;
    let (agent, _data_dir, cwd, _session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;

    agent
        .send(UserMessage::text("后台跑一下"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    wait_requests(&server, 2).await;
    std::fs::write(cwd.path().join("go"), "").unwrap();
    wait_requests(&server, 4).await;
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    let job_id = noticed_job_id(&bodies[2]);
    let expected = format!(
        "background job {job_id} (bash: {command}) finished [status: completed, exit code: 0]. Output:\ndone"
    );
    let texts = request_message_texts(&bodies[2]);
    assert!(
        texts.iter().any(|text| text == &expected),
        "完成通知照 DSH：缺 {expected:?} 在 {texts:?}"
    );
    // 通知已经内联这段输出（DSH held read），随后 job_output 不应重复返回。
    assert!(
        bodies[3].contains("(no new output)"),
        "内联通知消耗了游标：{}",
        bodies[3]
    );
}

/// T006.9（D3）：输出装不下时通知只让模型去 job_output 读，游标不消耗、数据不丢。
#[tokio::test]
async fn background_job_completion_notice_falls_back_without_consuming_output() {
    let command = "while [ ! -f go ]; do :; done; head -c 3000 /dev/zero | tr '\\0' 'x'";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(NoticeThenReadResponder {
            step: std::sync::Mutex::new(0),
            command,
        })
        .mount(&server)
        .await;
    let (agent, _data_dir, cwd, _session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;

    agent
        .send(UserMessage::text("后台跑一下"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    wait_requests(&server, 2).await;
    std::fs::write(cwd.path().join("go"), "").unwrap();
    wait_requests(&server, 4).await;
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    assert!(
        bodies[2].contains(". Read its output with job_output."),
        "大输出只给 job_output 指引：{}",
        bodies[2]
    );
    assert!(
        bodies[3].matches('x').count() >= 3000,
        "通知没消耗游标，job_output 能读到完整输出：{}",
        &bodies[3][..bodies[3].len().min(400)]
    );
}

/// T006.9（D3）：空输出照 DSH 回 `. No new output.`。
#[tokio::test]
async fn background_job_completion_notice_reports_no_new_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "bash",
                json!({
                    "description": "test command",
                    "command": "while [ ! -f go ]; do :; done",
                    "run_in_background": true
                }),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, _data_dir, cwd, _session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    agent
        .send(UserMessage::text("后台跑一下"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    wait_requests(&server, 2).await;
    std::fs::write(cwd.path().join("go"), "").unwrap();
    wait_requests(&server, 3).await;
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let last = String::from_utf8_lossy(&requests.last().unwrap().body.clone()).into_owned();
    let job_id = started_job_id(&String::from_utf8_lossy(&requests[1].body));
    let expected = format!(
        "background job {job_id} (bash: while [ ! -f go ]; do :; done) finished [status: completed, exit code: 0]. No new output."
    );
    assert!(
        last.contains(&expected),
        "空输出通知缺 {expected:?} 在 {last}"
    );
}
/// T006.9b（DSH killedByModel）：模型自己 job_kill 后，结算不再投完成通知（不会多开一轮）；
/// job/settled 仍写，status=killed / cause=kill。
#[tokio::test]
async fn model_job_kill_suppresses_completion_notice() {
    /// step0 起后台任务；step1 取 job id 回 job_kill；其后回 FINAL。
    struct KillThenFinishResponder {
        step: std::sync::Mutex<usize>,
    }

    impl wiremock::Respond for KillThenFinishResponder {
        fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
            let mut step = self.step.lock().expect("KillThenFinish 锁被毒化");
            let current = *step;
            *step += 1;
            drop(step);
            match current {
                0 => sse(tool_calls_with_inputs_sse(&[(
                    "call_0",
                    "bash",
                    json!({
                        "description": "test command", "command": "sleep 30", "run_in_background": true
                    }),
                )])),
                1 => {
                    let body = String::from_utf8_lossy(&request.body);
                    let job_id = started_job_id(&body);
                    sse(tool_calls_with_inputs_sse(&[(
                        "call_1",
                        "job_kill",
                        json!({"job_id": job_id}),
                    )]))
                }
                _ => sse(FINAL_TEXT_SSE.to_owned()),
            }
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(KillThenFinishResponder {
            step: std::sync::Mutex::new(0),
        })
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    agent
        .send(UserMessage::text("后台跑一下再停掉"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    wait_requests(&server, 3).await;
    agent.idle().await;

    // job/settled 仍写：status=killed、cause=kill（可与自然结束区分）。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let settled = loop {
        let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
        let settled = store.events().iter().find_map(|event| match &event.kind {
            session::EventKind::JobSettled {
                job_id,
                status,
                cause,
                ..
            } => Some((job_id.clone(), status.clone(), cause.clone())),
            _ => None,
        });
        if let Some(settled) = settled {
            break settled;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "5s 内没有写 job/settled"
        );
        tokio::task::yield_now().await;
    };
    assert_eq!(settled.1, "killed");
    assert_eq!(settled.2, "kill");

    // 若错误地投递完成通知，会唤醒新一轮（第 4 次请求）；1s 内不应出现。
    let extra = tokio::time::timeout(Duration::from_secs(1), wait_requests(&server, 4)).await;
    assert!(
        extra.is_err(),
        "模型自己 job_kill 后不应再投完成通知（不应多开一轮）"
    );

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    let job_id = started_job_id(&bodies[1]);
    let notice = format!("background job {job_id} (bash:");
    assert!(
        !bodies.iter().any(|body| body.contains(&notice)),
        "模型自己 job_kill 后不应有重复完成通知：{job_id}"
    );
    assert!(
        bodies[2].contains("requested cancellation of job"),
        "job_kill 的工具结果本身就是交付：{}",
        bodies[2]
    );
}

/// T006.9b fix-now：模型自己 job_kill 后任务**立刻自然完成**（命令 trap 记到 SIGTERM 后自己
/// exit 0，不是被信号杀死）：job/settled 照写 status=completed/cause=kill，且不投重复完成通知。
/// 用于覆盖 request_kill 与 killedByModel 标记之间的自然完成窗口（标记必须原子置位）。
#[tokio::test]
async fn fast_natural_finish_after_model_kill_suppresses_completion_notice() {
    struct NaturalFinishResponder {
        step: std::sync::Mutex<usize>,
    }

    impl wiremock::Respond for NaturalFinishResponder {
        fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
            let mut step = self.step.lock().expect("NaturalFinish 锁被毒化");
            let current = *step;
            *step += 1;
            drop(step);
            match current {
                0 => sse(tool_calls_with_inputs_sse(&[(
                    "call_0",
                    "bash",
                    json!({
                        "description": "test command",
                        "command": "trap 'printf x > got-term' TERM; while [ ! -f go ]; do :; done; printf 'done'",
                        "run_in_background": true
                    }),
                )])),
                1 => {
                    let body = String::from_utf8_lossy(&request.body);
                    let job_id = started_job_id(&body);
                    sse(tool_calls_with_inputs_sse(&[(
                        "call_1",
                        "job_kill",
                        json!({"job_id": job_id}),
                    )]))
                }
                _ => sse(FINAL_TEXT_SSE.to_owned()),
            }
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(NaturalFinishResponder {
            step: std::sync::Mutex::new(0),
        })
        .mount(&server)
        .await;
    let (agent, data_dir, cwd, session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    // 看到 SIGTERM 已到达（request_kill 原子置标记之后才发信号）再让命令自然结束。
    let cwd_path = cwd.path().to_path_buf();
    let watcher = tokio::spawn(async move {
        for _ in 0..5000 {
            if cwd_path.join("got-term").exists() {
                std::fs::write(cwd_path.join("go"), "").unwrap();
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("5s 内没等到 SIGTERM");
    });
    agent
        .send(UserMessage::text("后台跑一下再停掉"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    watcher.await.unwrap();
    wait_requests(&server, 3).await;
    agent.idle().await;

    // job/settled 仍写：自然完成 status=completed、cause=kill（DSH settleCause）。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let settled = loop {
        let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
        let settled = store.events().iter().find_map(|event| match &event.kind {
            session::EventKind::JobSettled {
                job_id,
                status,
                cause,
                ..
            } => Some((job_id.clone(), status.clone(), cause.clone())),
            _ => None,
        });
        if let Some(settled) = settled {
            break settled;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "5s 内没有写 job/settled"
        );
        tokio::task::yield_now().await;
    };
    assert_eq!(settled.1, "completed", "自然完成照 producer 事实");
    assert_eq!(
        settled.2, "kill",
        "kill 请求后自然完成，DSH settleCause 仍是 kill"
    );

    // 若错误地投递完成通知，会唤醒新一轮（第 4 次请求）；1s 内不应出现。
    let extra = tokio::time::timeout(Duration::from_secs(1), wait_requests(&server, 4)).await;
    assert!(
        extra.is_err(),
        "kill 请求后立刻自然完成也不应投重复完成通知"
    );
    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    let job_id = started_job_id(&bodies[1]);
    let notice = format!("background job {job_id} (bash:");
    assert!(
        !bodies.iter().any(|body| body.contains(&notice)),
        "不应有重复完成通知：{job_id}"
    );
}

/// T006.9c：job/settled 在结算时写；对后台任务允许晚于 turn/end（DSH jobs docs）。
#[tokio::test]
async fn settled_event_may_follow_turn_end_for_background_job() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "bash",
                json!({
                    "description": "slow background",
                    "command": "sleep 1; printf done",
                    "run_in_background": true
                }),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    agent
        .send(UserMessage::text("后台跑一下"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let (turn_end, settled) = loop {
        let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
        let events = store.events();
        let turn_end = events
            .iter()
            .find(|event| matches!(event.kind, session::EventKind::TurnEnd { .. }))
            .cloned();
        let settled = events
            .iter()
            .find(|event| matches!(event.kind, session::EventKind::JobSettled { .. }))
            .cloned();
        if let (Some(turn_end), Some(settled)) = (turn_end, settled) {
            break (turn_end, settled);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "10s 内没有写齐 turn/end 与 job/settled"
        );
        tokio::task::yield_now().await;
    };
    assert!(
        settled.seq > turn_end.seq,
        "后台任务结算可以晚于 turn/end（DSH jobs docs）；实际 turn/end={} settled={}",
        turn_end.seq,
        settled.seq
    );
}
