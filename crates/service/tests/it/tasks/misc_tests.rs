//! 其余任务闭环用例。

use super::*;

#[tokio::test]
async fn a_task_frozen_with_the_eight_tool_list_still_launches() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "旧任务也能跑", None));
    let store = tasks_store(fixture.data());
    let mut tools: Vec<String> = service::THIN_TOOLS.map(str::to_owned).to_vec();
    tools.retain(|name| name != "task_handback" && !name.starts_with("task_ledger_"));
    let draft = tasks::NewTask {
        quote: None,
        quote_source: None,
        kind_note: None,
        project_id: None,
        cwd_source: None,
        title: "升级前受理的任务".to_owned(),
        requirement: "找答案".to_owned(),
        dod: "有答案".to_owned(),
        cwd: fixture.data().to_path_buf(),
        conversation: tasks::ConversationId::new("legacy-kind-session"),
        model: tasks::ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools,
    };
    let task = store.create_task(draft, 1_700_000_000_000).unwrap();
    wait_until("升级前任务开跑", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            !task.missing.is_empty()
                || task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    assert!(
        task_row.missing.is_empty(),
        "旧任务冻结的 8 件工具不能算缺项：{:?}",
        task_row.missing
    );
    assert_eq!(
        task_row.latest_report().map(|report| report.state),
        Some(ReportState::Ready)
    );
}

#[tokio::test]
async fn task_stream_is_live_while_the_task_is_running() {
    let server = MockServer::start().await;
    let router = Router::new();
    // ThinAgent 的响应慢一点，让测试能在任务跑着的时候接上 live。
    router.set_thin_delay(Duration::from_millis(800));
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;

    let args = serde_json::json!({
        "title": "查一下 hello",
        "quote": "查一下 hello",
        "requirement": "读 hello.txt",
        "dod": "说出内容"
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));
    router.push_thin(crate::common::tool_use_sse(
        1,
        "call-read",
        "read",
        &serde_json::json!({"file_path": "hello.txt"}).to_string(),
    ));
    router.push_thin(crate::common::text_sse("查完了"));

    host.dispatch(Call::Send(send(&host, "kind-live", "查一下 hello")))
        .await
        .expect("发送");
    let store = tasks_store(data.path());
    wait_until("任务建好", || {
        store.task_ids().map(|ids| ids.len()).unwrap_or(0) == 1
    })
    .await;
    let mut ids = store.task_ids().unwrap();
    let task_id = ids.remove(0);

    let mut opened = None;
    for _ in 0..200 {
        if let Ok(stream) = host.task_stream(task_id.as_str(), 0).await
            && let Some(live) = stream.live
        {
            opened = Some((stream.follow.last_seq, live));
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (last, mut live) = opened.expect("任务在跑时要能接上 live 订阅");

    let event = tokio::time::timeout(Duration::from_secs(5), live.events.recv())
        .await
        .expect("在跑的任务要送来 live 事件")
        .expect("live 订阅还开着");
    assert!(
        event.seq > last,
        "live 事件在打开时的补齐之后：{} <= {last}",
        event.seq
    );
}

/// task_continue 的拒绝项——没有可认的 cause、任务在跑。
#[tokio::test]
async fn task_continue_rejects_without_cause_or_while_running() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, _downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    // 用户还没判：没有可认的 cause。
    let error = host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: guarded.id.as_str().to_owned(),
            note: None,
        }))
        .await
        .expect_err("没有 cause 应拒绝");
    assert!(
        error.to_string().contains("没找到让它接着做的事实"),
        "拒绝原因：{error}"
    );

    // 判需修正并接着做；它还在跑时再 continue 一次要拒绝。
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    host.dispatch(Call::GateJudge(GateJudgeRequest {
        gate_task: gate.id.as_str().to_owned(),
        gate_report: gate_report.as_str().to_owned(),
        conclusion: GateJudgeConclusion::NeedsFix,
        note: Some("再改一版".to_owned()),
    }))
    .await
    .expect("判需修正");
    // 判完等上一轮 Attempt 收尾落库：并发跑测试时 record_report 后的 end 写入可能稍晚。
    wait_until("B 上一轮 Attempt 结束", || {
        tasks_store(data.path())
            .task(&guarded.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    router.set_thin_delay(Duration::from_secs(2));
    router.push_thin_for(guarded.id.as_str(), crate::common::text_sse("还在改"));
    // 并发跑整套时执行席位/会话写者可能还没让出来，服务按 Later 回「现在接不上」；
    // 用户再点一次就行，这里也重试到接上（第一次 Attempt 没有副作用，重试安全）。
    let mut continued = false;
    for _ in 0..100 {
        match host
            .dispatch(Call::TaskContinue(service::TaskContinueRequest {
                task_id: guarded.id.as_str().to_owned(),
                note: Some("再改一版".to_owned()),
            }))
            .await
        {
            Ok(_) => {
                continued = true;
                break;
            }
            Err(error) if error.to_string().contains("没有空位或会话还在收尾") => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!("第一次接着做：{error}"),
        }
    }
    assert!(continued, "第一次接着做一直接不上");
    wait_until("B 在跑", || {
        tasks_store(data.path())
            .task(&guarded.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_none())
    })
    .await;
    let error = host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: guarded.id.as_str().to_owned(),
            note: Some("再改一版".to_owned()),
        }))
        .await
        .expect_err("在跑应拒绝");
    assert!(error.to_string().contains("还在跑"), "拒绝原因：{error}");
}

/// 上游 A 又交新结果 → B 需重评、Kind 收到 relay、阶段不回退 → task_continue 按新结果核对 → 清除。
#[tokio::test]
async fn upstream_new_result_marks_reevaluation_and_continue_checks_again() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    let (upstream, downstream) = (children[0].clone(), children[1].clone());
    router.push_thin_for(
        upstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-a1-report",
            "task_report",
            r#"{"state":"ready","summary":"A 第一次结果"}"#,
        ),
    );
    router.push_thin_for(upstream.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        downstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b1-report",
            "task_report",
            r#"{"state":"ready","summary":"B 第一次结果"}"#,
        ),
    );
    router.push_thin_for(downstream.id.as_str(), crate::common::text_sse("好"));
    for _ in 0..12 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),
        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");
    wait_until("B 第一次报回", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    // A 第二轮：说话接续，再报一份新结果。
    wait_until("A 第一轮结束", || {
        tasks_store(data.path())
            .task(&upstream.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    router.push_thin_for(
        upstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-a2-report",
            "task_report",
            r#"{"state":"ready","summary":"A 第二次结果：5 行"}"#,
        ),
    );
    router.push_thin_for(upstream.id.as_str(), crate::common::text_sse("好"));
    host.dispatch(Call::TaskSay(TaskSayRequest {
        task_id: upstream.id.as_str().to_owned(),
        text: "再查一版".to_owned(),
        pastes: Vec::new(),
        attachments: Vec::new(),
        object_refs: Vec::new(),
    }))
    .await
    .expect("A 第二轮输入");
    wait_until("A 第二次报回", || {
        tasks_store(data.path())
            .task(&upstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_report()
                        .is_some_and(|report| report.summary.contains("5 行"))
            })
    })
    .await;
    wait_until("B 需重评", || {
        tasks_store(data.path())
            .needs_reevaluation(&downstream.id)
            .unwrap_or(false)
    })
    .await;
    // 阶段不回退，Kind 收到 upstream_changed relay。
    let store = tasks_store(data.path());
    assert_eq!(store.stage(&downstream.id).unwrap(), Stage::ReadyForReview);
    wait_until("Kind 收到上游变了 relay", || {
        crate::common::read_root_events(data.path())
            .iter()
            .any(|event| {
                let session::EventKind::UserMessage { message, .. } = &event.kind else {
                    return false;
                };
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                text.contains("upstream_changed")
                    && text.contains("5 行")
                    && text.contains("call task_continue once for the downstream task")
            })
    })
    .await;
    let upstream_task = tasks_store(data.path())
        .task(&upstream.id)
        .unwrap()
        .unwrap();
    let latest_upstream = upstream_task.latest_report().cloned().unwrap();
    // Kind 用 task_continue 让 B 按新结果核对。
    router.push_thin_for(
        downstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b2-report",
            "task_report",
            r#"{"state":"ready","summary":"B 按新结果核对完了"}"#,
        ),
    );
    router.push_thin_for(downstream.id.as_str(), crate::common::text_sse("好"));
    let continued = host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: downstream.id.as_str().to_owned(),
            note: None,
        }))
        .await
        .expect("按新结果核对");
    let Response::TaskContinue(continued) = continued else {
        panic!("task-continue 响应");
    };
    wait_until("B 第二次 Attempt 开跑", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.latest_attempt()
                    .is_some_and(|attempt| attempt.id.0 == continued.attempt_id)
            })
    })
    .await;
    let b = tasks_store(data.path())
        .task(&downstream.id)
        .unwrap()
        .unwrap();
    let attempt = b.latest_attempt().unwrap();
    match &attempt.trigger {
        tasks::AttemptTrigger::KindContinue { cause, .. } => {
            assert!(
                matches!(
                    cause,
                    tasks::ContinueCause::UpstreamChanged { upstream_report }
                        if *upstream_report == latest_upstream.id
                ),
                "cause 是 {cause:?}"
            );
        }
        other => panic!("trigger 是 {other:?}"),
    }
    assert_eq!(
        attempt.handoffs[0].report_id, latest_upstream.id,
        "这次 Attempt 交接的是上游新结果"
    );
    wait_until("B 第二次 ready", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_report()
                        .is_some_and(|report| report.summary.contains("核对完"))
            })
    })
    .await;
    assert!(
        !tasks_store(data.path())
            .needs_reevaluation(&downstream.id)
            .unwrap(),
        "新结果报回后需重评清除"
    );
    assert_eq!(
        tasks_store(data.path()).stage(&downstream.id).unwrap(),
        Stage::ReadyForReview
    );
}

/// 带结果卡发消息 → 模型看到一行文字 → Kind 出 update_task_requirement 确认卡 →
/// 确认后同一任务开第 2 次 Attempt（RequirementChanged）、版本 2、新结果卡带版本。
#[tokio::test]
async fn update_task_requirement_card_round_trip_opens_a_new_attempt() {
    let fixture = Fixture::new_with_actions().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report-v1", "ready", "查好了 v1", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let task_id = task.id.as_str().to_owned();
    let title = task.title.clone();
    let requirement = "找答案；空标题也别丢，用域名加路径当键";
    // 改要求那一轮按请求内容动态回：触发词 → skill；skill 结果 → propose_action。
    // 不排静态队列，relay 轮和这条消息谁先跑都不影响脚本顺序。
    fixture.router.enable_update_kind(
        &task_id,
        vec![(
            "用户要求修改这张结果卡".to_owned(),
            "call-update-skill".to_owned(),
            "call-update-card".to_owned(),
            requirement.to_owned(),
            "空标题也别丢，用域名加路径当键".to_owned(),
        )],
    );
    let (_relay_id, body) = ready_relay(&fixture).await;
    assert_eq!(body["requirement_version"], 1, "{body}");
    // 报告落库和 Attempt 收尾是两个写；Kind 出卡前要求「不在跑」，等收尾落稳再发。
    wait_until("v1 Attempt 结束", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    wait_kind_idle(&fixture.host).await;

    fixture
        .host
        .dispatch(Call::Send(SendRequest {
            request_id: "card-send-1".to_owned(),
            content: "空标题也别丢，用域名加路径当键".to_owned(),
            delivery: service::DeliveryMode::FollowUp,
            pastes: Vec::new(),
            attachments: Vec::new(),
            object_refs: Vec::new(),
            canvas: None,
            result_card: Some(service::SendResultCard {
                task_id: task_id.clone(),
                title: title.clone(),
                version: 1,
            }),
            result_cards: Vec::new(),
            canvases: Vec::new(),
        }))
        .await
        .expect("带结果卡发消息");
    let mut card1 = false;
    for _ in 0..900 {
        if crate::common::read_root_events(fixture.data())
            .iter()
            .any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::ToolResult { message, meta, .. }
                        if message.tool_call_id.as_deref() == Some("call-update-card")
                            && meta.as_ref().is_some_and(|meta| meta.get("card").is_some())
                )
            })
        {
            card1 = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(card1, "Kind 提出改要求卡");

    // 模型收到的请求里有那行文字（排在用户文字前面）。
    let expected =
        format!("[用户要求修改这张结果卡：任务 {task_id}「{title}」，第 1 版要求的结果]");
    let bodies = fixture.router.bodies(false);
    let saw_line = bodies
        .iter()
        .any(|body| body.contains(&expected) && body.contains("空标题也别丢，用域名加路径当键"));
    assert!(saw_line, "模型请求里要有结果卡那行文字");
    assert!(
        bodies.iter().any(|body| body.contains("结果卡不是程序消息")
            && body.contains("update_task_requirement")
            && body.contains("直接回答")),
        "结果卡规则只在卡文字里"
    );

    // 新 Attempt 的脚本备好，再点确认。
    fixture.router.push_thin_for(
        task_id.as_str(),
        report_sse(1, "call-report-v2", "ready", "查好了 v2", None),
    );
    fixture
        .router
        .push_thin_for(task_id.as_str(), crate::common::text_sse("好"));
    let confirmed = fixture
        .host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-update-card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),
            note: None,
            report_id: None,
        }))
        .await
        .expect("确认改要求");
    let decision = serde_json::to_value(confirmed).unwrap()["decision"].clone();
    assert_eq!(decision["phase"], "confirmed", "{decision}");

    wait_until("新 Attempt 开跑", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().map(|attempt| attempt.id.0))
            == Some(2)
    })
    .await;
    let store = tasks_store(fixture.data());
    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.requirement_version(), 2);
    assert_eq!(updated.requirement, requirement);
    assert_eq!(
        updated.versions[1].quote.as_deref(),
        Some("空标题也别丢，用域名加路径当键")
    );
    assert!(matches!(
        updated.versions[1].quote_source,
        Some(tasks::QuoteSource::UserMessage { .. })
    ));
    match &updated.latest_attempt().expect("新 Attempt").trigger {
        tasks::AttemptTrigger::RequirementChanged => {}
        other => panic!("trigger 是 {other:?}"),
    }
    assert_eq!(
        updated.requirement_of_version(1),
        Some("找答案"),
        "第 1 版文本保留"
    );

    wait_until("第 2 版结果 relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["requirement_version"] == 2)
    })
    .await;
    let (v2_relay, v2_body) = relay_messages(&crate::common::read_root_events(fixture.data()))
        .into_iter()
        .find(|(_, body)| body["requirement_version"] == 2)
        .expect("第 2 版 relay");
    assert_eq!(v2_body["task"]["id"], task.id.as_str());
    // 接受第 2 版 → 已关闭；旧接受历史里没有第 1 版（这一轮才第一次接受）。
    let accepted = accept_result_card(&fixture.host, &v2_relay).await;
    assert_eq!(accepted["phase"], "confirmed", "{accepted}");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    let closed = store.task(&task.id).unwrap().unwrap();
    assert_eq!(closed.acceptances.len(), 1);
    assert_eq!(closed.acceptances[0].version, 2);
}

/// 动作的三种拒绝：在排队、在跑、要求和 DoD 都没变；reconcile 回「要求改过了」。
#[tokio::test]
async fn update_task_requirement_action_rejects_queued_running_and_unchanged() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await; // pause：launch 还排在队里
    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    let events = crate::common::read_root_events(fixture.data());
    let env = service::ActionEnv {
        data_dir: fixture.data(),
        identity: None,
        events: &events,
        conversation: None,
    };
    let queued = action.describe(
        &serde_json::json!({"task_id": task.id.as_str(), "quote": "改一下", "requirement": "新要求"}),
        &env,
    );
    assert!(
        queued
            .as_ref()
            .err()
            .is_some_and(|reason| reason.contains("排队")),
        "{queued:?}"
    );

    // 开闸后任务在跑：拒绝。
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("任务开跑", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_none())
    })
    .await;
    let events = crate::common::read_root_events(fixture.data());
    let env = service::ActionEnv {
        data_dir: fixture.data(),
        identity: None,
        events: &events,
        conversation: None,
    };
    let running = action.describe(
        &serde_json::json!({"task_id": task.id.as_str(), "quote": "改一下", "requirement": "新要求"}),
        &env,
    );
    assert!(
        running
            .as_ref()
            .err()
            .is_some_and(|reason| reason.contains("还在跑")),
        "{running:?}"
    );
}

/// 动作的 reconcile 在卡上「改前」已经不符时回固定原因；要求和 DoD 都没变也被拒。
#[tokio::test]
async fn update_task_requirement_reconcile_refuses_after_a_change() {
    let fixture = Fixture::new_with_actions().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report-v1", "ready", "查好了 v1", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let (relay_id, _) = ready_relay(&fixture).await;
    let _ = relay_id;
    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    let events = crate::common::read_root_events(fixture.data());
    let env = service::ActionEnv {
        data_dir: fixture.data(),
        identity: None,
        events: &events,
        conversation: None,
    };
    let task_id = task.id.as_str().to_owned();

    let details = serde_json::json!({
        "task_id": task_id,
        "quote": "改一下",
        "requirement": "第一版之外的第二个要求",
        "dod": "新 DoD",
    });
    let bad_quote = action.describe(
        &serde_json::json!({
            "task_id": task_id,
            "quote": "用户没说过这句",
            "requirement": "第一版之外的第二个要求",
        }),
        &env,
    );
    assert!(
        bad_quote
            .as_ref()
            .err()
            .is_some_and(|reason| reason.contains("派任务要用用户原话")),
        "{bad_quote:?}"
    );
    let facts = action.describe(&details, &env).expect("能出卡");
    assert_eq!(facts.len(), 3, "三行：你这次说的、Kind 的理解、版本");
    assert_eq!(facts[0].name, "你这次说的");
    assert_eq!(facts[0].value, "改一下");
    assert_eq!(facts[1].name, "要求（Kind 的理解）");
    assert!(
        facts[1].value.contains("第一版之外的第二个要求"),
        "{}",
        facts[1].value
    );
    assert!(
        facts[2].value.contains("第 1 版 → 第 2 版"),
        "{}",
        facts[2].value
    );
    // 要求和 DoD 都没变：describe 拒绝。
    let unchanged = action.describe(
        &serde_json::json!({"task_id": task_id, "quote": "改一下", "requirement": "找答案"}),
        &env,
    );
    assert!(
        unchanged
            .as_ref()
            .err()
            .is_some_and(|reason| reason.contains("都没变")),
        "{unchanged:?}"
    );

    // 有人先把要求改成了别的（例如另一张卡先确认）：这张卡再确认要回固定原因。
    let store = tasks_store(fixture.data());
    store
        .update_requirement(&task.id, "别人先改成的第三版要求", None, 9_000)
        .expect("先改要求");
    let refused = action.reconcile(&details, &facts, &env);
    assert_eq!(
        refused.err().as_deref(),
        Some("这个任务的要求已经改过了，这张卡是按改之前写的")
    );
}

/// 审计备注 2：从没开跑的任务改要求——先查后写，回同样的话、任务库不动。
#[tokio::test]
async fn update_task_requirement_act_rejects_a_never_started_task_without_writing() {
    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let task = store
        .create_task(
            NewTask {
                quote: None,
                quote_source: None,
                kind_note: None,
                project_id: None,
                cwd_source: None,
                title: "没开跑".to_owned(),
                requirement: "原要求".to_owned(),
                dod: "原 DoD".to_owned(),
                cwd: fixture.data().to_path_buf(),
                conversation: ConversationId::new("kind-work"),
                model: ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: vec!["read".to_owned()],
            },
            1_800_000_000_000,
        )
        .unwrap();
    // 把排队中的 launch 效果收掉但不开始 Attempt：模拟「从没开跑」的任务。
    let claim = store
        .claim_effect(EffectKind::Launch, 1_800_000_001_000)
        .unwrap()
        .expect("launch 在排队");
    store.complete_effect(&claim).unwrap();
    let before = store.task(&task.id).unwrap().unwrap();
    assert!(before.latest_attempt().is_none());

    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    let refused = service::ActionKind::act(
        action.as_ref(),
        &serde_json::json!({"task_id": task.id.as_str(), "quote": "改一下", "requirement": "新要求"}),
        &fixture.host,
    )
    .await
    .expect_err("从没开跑不能改");
    assert!(refused.contains("还没开跑"), "{refused}");

    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(after.requirement, before.requirement, "不写库");
    assert_eq!(after.dod, before.dod, "不写库");
    assert_eq!(
        after.requirement_version(),
        before.requirement_version(),
        "版本不动"
    );
    assert_eq!(after.versions.len(), before.versions.len(), "版本不动");
}

///.2：改要求动作的 constraints 必须逐字是用户原话；对不上整次拒、不写库。
#[tokio::test]
async fn update_requirement_constraints_must_be_user_words_and_land_on_the_new_version() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查好了", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("任务这一轮结束", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    // 报告落库后会话可能还在收尾；等写者让出来再改要求，避免 open_resume 撞 AlreadyOwned。
    wait_until("任务会话写者让出", || {
        let task = tasks_store(fixture.data()).task(&task.id).ok().flatten();
        let Some(conversation) = task.and_then(|task| {
            task.latest_attempt()
                .and_then(|attempt| attempt.conversation.clone())
        }) else {
            return false;
        };
        let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
            return false;
        };
        session::SessionStore::open(fixture.data(), &id).is_ok()
    })
    .await;
    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    let before = store.task(&task.id).unwrap().unwrap();

    let refused = service::ActionKind::act(
        action.as_ref(),
        &serde_json::json!({
            "task_id": task.id.as_str(),
            "quote": "改一下",
            "requirement": "新要求",
            "constraints": ["用户没说过这句"],
        }),
        &fixture.host,
    )
    .await
    .expect_err("非原话要整次拒");
    assert!(refused.contains("约束要用用户原话"), "{refused}");
    let unchanged = store.task(&task.id).unwrap().unwrap();
    assert_eq!(unchanged.requirement, before.requirement);
    assert_eq!(unchanged.dod, before.dod);
    assert_eq!(
        unchanged.requirement_version(),
        before.requirement_version()
    );
    assert_eq!(unchanged.versions.len(), before.versions.len());
    assert!(unchanged.current_constraints().is_empty());
    assert_eq!(unchanged.versions, before.versions);

    let summary = service::ActionKind::act(
        action.as_ref(),
        &serde_json::json!({
            "task_id": task.id.as_str(),
            "quote": "改一下",
            "requirement": "按用户原话改的新要求",
            "constraints": ["改一下"],
        }),
        &fixture.host,
    )
    .await
    .expect("用户原话约束应成功");
    assert!(summary.summary.contains("第 2 版"), "{}", summary.summary);
    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.requirement, "按用户原话改的新要求");
    assert_eq!(
        updated.requirement_version(),
        before.requirement_version() + 1
    );
    assert_eq!(updated.current_constraints(), ["改一下"]);
    assert_eq!(updated.versions[1].quote.as_deref(), Some("改一下"));
}

///.2：改要求不传 constraints 时沿用上一版。
#[tokio::test]
async fn update_requirement_without_constraints_keeps_the_previous_version() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查好了", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture
        .accept_args(
            false,
            serde_json::json!({
                "title": "沿约束",
                "requirement": "找答案",
                "dod": "有答案",
                "constraints": ["改一下"],
            }),
            0,
        )
        .await;
    let store = tasks_store(fixture.data());
    wait_until("任务这一轮结束", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    wait_until("任务会话写者让出", || {
        let task = tasks_store(fixture.data()).task(&task.id).ok().flatten();
        let Some(conversation) = task.and_then(|task| {
            task.latest_attempt()
                .and_then(|attempt| attempt.conversation.clone())
        }) else {
            return false;
        };
        let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
            return false;
        };
        session::SessionStore::open(fixture.data(), &id).is_ok()
    })
    .await;
    let registry = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = registry.get("update_task_requirement").expect("动作登记");
    service::ActionKind::act(
        action.as_ref(),
        &serde_json::json!({
            "task_id": task.id.as_str(),
            "quote": "改一下",
            "requirement": "只改要求文字，不传约束",
        }),
        &fixture.host,
    )
    .await
    .expect("改要求成功");
    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.current_constraints(), ["改一下"]);
}
