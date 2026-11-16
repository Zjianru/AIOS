//! 上报与 relay：ready/waiting/failed/blocked、提问、交回。

use super::*;

#[tokio::test]
async fn task_status_returns_the_full_report_and_attempts() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;

    // 任务的发起会话不是根会话，relay 会 Later，不会在断言前 Steer 醒 Kind。
    router.push_thin(crate::common::tool_use_sse(
        0,
        "call-report",
        "task_report",
        &serde_json::json!({"state": "ready", "summary": "完整答案：第一行\n第二行"}).to_string(),
    ));
    router.push_thin(crate::common::text_sse(""));
    let store = tasks_store(data.path());
    let task = store
        .create_task(
            tasks::NewTask {
                title: "查一下".to_owned(),
                requirement: "找答案".to_owned(),
                dod: "有答案".to_owned(),
                cwd: data.path().to_path_buf(),
                conversation: tasks::ConversationId::new("another-kind-session"),
                model: tasks::ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: service::THIN_TOOLS.map(str::to_owned).to_vec(),
            },
            1_700_000_000_000,
        )
        .unwrap();
    wait_until("ready 报告", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;

    let status_args = serde_json::json!({"task_id": task.id.as_str()}).to_string();
    router.push_kind(crate::common::tool_use_sse(
        1,
        "call-status",
        "task_status",
        &status_args,
    ));
    router.push_kind(crate::common::text_sse("我看一下"));
    host.dispatch(Call::Send(send(&host, "kind-1", "任务怎么样了")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;

    let events = crate::common::read_root_events(data.path());
    let result = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .find(|text| text.contains("Attempt 数"))
        .expect("task_status 结果");
    assert!(result.contains("阶段：待检查"), "{result}");
    assert!(result.contains("Attempt 数：1"), "{result}");
    assert!(result.contains("完整答案：第一行\n第二行"), "{result}");
    let unexpected = router.state.unexpected.lock().unwrap();
    assert!(
        unexpected.is_empty(),
        "假模型收到没有安排的请求：{unexpected:?}"
    );
}

#[tokio::test]
async fn a_missing_credential_opens_no_task_session_and_relays_what_is_missing() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let args = serde_json::json!({
        "title": "查一下",
        "requirement": "找答案",
        "dod": "有答案"
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));
    host.dispatch(Call::Send(send(&host, "kind-1", "查一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;

    let store = tasks_store(data.path());
    let task = one_task(&store);
    host.dispatch(Call::DisabledModelsSet(DisabledModelsSetRequest {
        models: vec!["deepseek-flash".to_owned()],
    }))
    .await
    .expect("屏蔽模型");
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(false);

    wait_until("缺项报告", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| !task.missing.is_empty())
    })
    .await;
    wait_until("缺项 relay", || {
        !relay_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;

    let task = store.task(&task.id).unwrap().unwrap();
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    assert!(
        task.missing
            .iter()
            .any(|item| item.as_str().contains("凭证")),
        "{:?}",
        task.missing
    );
    assert!(router.bodies(true).is_empty(), "缺项不能开任务会话");
    let session_files = std::fs::read_dir(data.path().join("sessions"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        })
        .count();
    assert_eq!(session_files, 1, "只有 Kind 的根会话");
    let relays = relay_messages(&crate::common::read_root_events(data.path()));
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "blocked");
    assert_eq!(relays[0].1["task"]["stage"], "needs_conditions");
    // 程序按任务包缺项写的 blocked：origin=package；缺项文本在程序写好的摘要里。
    assert_eq!(relays[0].1["origin"], "package");
    let summary = relays[0].1["summary"].as_str().unwrap();
    assert!(summary.contains("任务包缺"), "{summary}");
    assert!(summary.contains("deepseek-flash"), "{summary}");
}

#[tokio::test]
async fn a_turn_end_with_a_reply_synthesizes_waiting_then_a_status_report_replaces_it() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查，别急"));
    wait_until("waiting relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;

    let relays = waiting_relays(fixture.data());
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "waiting");
    assert_eq!(relays[0].1["summary"], "我还在查，别急");
    assert_eq!(relays[0].1["task"]["stage"], "running");
    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);

    // 要状态那次 turn 直接报 ready：waiting 能被终态覆盖，不报冲突。
    fixture.router.push_thin(report_sse(
        1,
        "call-report",
        "ready",
        "答案在第二行
有出处",
        None,
    ));
    fixture.router.push_thin(crate::common::text_sse("done"));
    let status_args = serde_json::json!({"task_id": task.id.as_str()}).to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        2,
        "call-status-1",
        "task_status",
        &status_args,
    ));
    fixture.router.push_kind(crate::common::tool_use_sse(
        3,
        "call-status-2",
        "task_status",
        &status_args,
    ));
    fixture.router.push_kind(crate::common::text_sse("好"));
    // relay 可能 Steer 进下一 step，多备一段回复。
    fixture.router.push_kind(crate::common::text_sse("收到"));

    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-2", "怎么样了")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;
    wait_until("ready relay", || waiting_relays(fixture.data()).len() == 2).await;

    let relays = waiting_relays(fixture.data());
    assert_eq!(relays[1].1["state"], "ready");
    assert_eq!(
        relays[1].1["summary"],
        "答案在第二行
有出处"
    );
    assert_eq!(relays[1].1["task"]["stage"], "ready_for_review");

    // 同一个 Attempt 只要一次报告：Kind 的两次 task_status 只落一条 status 请求。
    let task = store.task(&task.id).unwrap().unwrap();
    let attempt = task.latest_attempt().unwrap();
    let id = session::SessionId::parse(attempt.conversation.as_ref().unwrap().as_str()).unwrap();
    let thin = session::SessionStore::open_read_only(fixture.data(), &id).unwrap();
    let status_requests = thin
        .events()
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                session::EventKind::UserMessage { message, .. }
                    if matches!(&message.source, model::MessageSource::User { rpc_id: Some(rpc) } if rpc == &format!("status:{}:{}", task.id, attempt.id))
            )
        })
        .count();
    assert_eq!(status_requests, 1, "同一个 Attempt 只有一条状态请求");

    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);
}

#[tokio::test]
async fn a_turn_end_without_a_reply_relays_the_status_hint() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture.router.push_thin(crate::common::text_sse(""));
    wait_until("waiting relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;
    let relays = waiting_relays(fixture.data());
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "waiting");
    assert_eq!(
        relays[0].1["summary"],
        "ThinAgent 没上报，用 task_status 去问"
    );
    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);
}

#[tokio::test]
async fn a_blocked_report_carries_its_question() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture.router.push_thin(report_sse(
        1,
        "call-report",
        "blocked",
        "缺一份只有你能给的版本号",
        Some("用哪个版本？"),
    ));
    fixture.router.push_thin(crate::common::text_sse("等回答"));
    wait_until("blocked relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;
    let relays = waiting_relays(fixture.data());
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "blocked");
    assert_eq!(relays[0].1["question"], "用哪个版本？");
    assert_eq!(relays[0].1["task"]["stage"], "needs_conditions");
    // 缺项报告同样是 blocked + needs_conditions；.4 只靠 origin 区分「卡住了」和「没开跑」。
    assert_eq!(relays[0].1["origin"], "agent");
    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    assert!(
        store.task(&task.id).unwrap().unwrap().missing.is_empty(),
        "agent 报的 blocked 不是任务包缺项"
    );
}

#[tokio::test]
async fn task_handback_relays_one_blocked_card_with_the_fixed_question() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-handback",
        "task_handback",
        &serde_json::json!({"reason": "用户要改任务要求，这超出了任务范围"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::text_sse("交回了"));
    // 交回的 relay 由效果泵异步送进 Kind 根会话：等 relay 本身落进根日志再断言。
    // 不等 ThinAgent 的第二次请求——那时 relay 可能还没写，读到的会是 0 条（时红时绿）。
    wait_until("交回的 relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "交回只出恰一条 relay：{relays:?}");
    let body = &relays[0].1;
    assert_eq!(body["kind"], "task_report");
    assert_eq!(body["state"], "blocked");
    // 页面按 origin=agent 把 blocked 画成「卡住了」；包缺项才是「没开跑」。
    assert_eq!(body["origin"], "agent");
    assert_eq!(body["question"], "这超出了任务范围，要不要改任务？");
    assert_eq!(body["summary"], "用户要改任务要求，这超出了任务范围");
    assert_eq!(body["task"]["stage"], "needs_conditions");

    let store = tasks_store(fixture.data());
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    assert_eq!(
        attempt.end,
        Some(AttemptEnd::Reported),
        "交回也以 reported 结束这个 Attempt"
    );
    let events = thin_events(fixture.data(), &attempt);
    let (text, error) = tool_result(&events, "call-handback");
    assert_eq!(error, None, "{text}");
    assert!(text.contains("已交回"), "{text}");
}

#[tokio::test]
async fn task_handback_twice_returns_already_handed_back_without_a_second_relay() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-handback-1",
        "task_handback",
        &serde_json::json!({"reason": "第一次超出了任务范围"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::tool_use_sse(
        2,
        "call-handback-2",
        "task_handback",
        &serde_json::json!({"reason": "第二次也要交回"}).to_string(),
    ));
    fixture
        .router
        .push_thin(crate::common::text_sse("交回了两次"));
    wait_until("第二次交回的回执", || {
        let store = tasks_store(fixture.data());
        let Some(task_row) = store.task(&task.id).ok().flatten() else {
            return false;
        };
        let Some(attempt) = task_row.latest_attempt() else {
            return false;
        };
        if attempt.conversation.is_none() {
            return false;
        }
        thin_events(fixture.data(), attempt).iter().any(|event| {
            matches!(
                &event.kind,
                session::EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some("call-handback-2")
            )
        })
    })
    .await;

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "第二次交回不再出 relay：{relays:?}");
    let store = tasks_store(fixture.data());
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    let (first, first_error) = tool_result(&events, "call-handback-1");
    assert_eq!(first_error, None, "{first}");
    assert!(first.contains("已交回"), "{first}");
    let (second, second_error) = tool_result(&events, "call-handback-2");
    assert_eq!(second_error, None, "{second}");
    assert_eq!(second, "已交回");
    let reports: Vec<&tasks::Report> = task_row
        .reports
        .iter()
        .filter(|report| report.attempt_id == attempt.id)
        .collect();
    assert_eq!(reports.len(), 1, "同一个 Attempt 只留一份报告：{reports:?}");
    assert_ne!(reports[0].summary, "第二次也要交回");
}

#[tokio::test]
async fn thin_agent_handback_rule_and_tool_stay_off_kind() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查完了", None));
    fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 请求", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let thin_bodies = fixture.router.bodies(true);
    let thin: Value = serde_json::from_str(&thin_bodies[0]).unwrap();
    let thin_tools = tool_names(&thin_bodies[0]);
    assert!(
        thin_tools.contains(&"task_handback".to_owned()),
        "{thin_tools:?}"
    );
    let thin_system = thin["system"].as_str().expect("ThinAgent 系统提示");
    assert!(
        thin_system.contains("outside this task's scope"),
        "交回规则要进 ThinAgent 的 policy_prompt：{thin_system}"
    );
    assert!(
        thin_system.contains("do not do that work yourself"),
        "规则要写明不自己做：{thin_system}"
    );
    assert!(
        !thin_system.contains("You are AIOS's coordinator"),
        "Kind 的派任务说明不能进 ThinAgent：{thin_system}"
    );
    assert!(
        !thin_system.contains("这超出了任务范围，要不要改任务？"),
        "固定问句只能由程序写进 relay，不能抄进角色规则"
    );

    let kind_bodies = fixture.router.bodies(false);
    let kind_tools = tool_names(&kind_bodies[0]);
    assert!(
        !kind_tools.contains(&"task_handback".to_owned()),
        "task_handback 只给 ThinAgent：{kind_tools:?}"
    );
    let kind: Value = serde_json::from_str(&kind_bodies[0]).unwrap();
    let kind_system = kind["system"].as_str().expect("Kind 系统提示");
    assert!(!kind_system.contains("task_handback"), "{kind_system}");
    assert!(!kind_system.contains("这超出了任务范围，要不要改任务？"));
    assert_eq!(service::THIN_TOOLS.len(), 11, "ThinAgent 的工具表 11 件");
}

#[tokio::test]
async fn task_handback_rejects_a_reason_longer_than_300_characters() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    let too_long = "长".repeat(301);
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-handback-long",
        "task_handback",
        &serde_json::json!({"reason": too_long}).to_string(),
    ));
    fixture.router.push_thin(crate::common::text_sse("没交回"));
    wait_until("超长交回的回执", || {
        let store = tasks_store(fixture.data());
        let Some(task_row) = store.task(&task.id).ok().flatten() else {
            return false;
        };
        let Some(attempt) = task_row.latest_attempt() else {
            return false;
        };
        if attempt.conversation.is_none() {
            return false;
        }
        thin_events(fixture.data(), attempt).iter().any(|event| {
            matches!(
                &event.kind,
                session::EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some("call-handback-long")
            )
        })
    })
    .await;

    let store = tasks_store(fixture.data());
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    let (text, error) = tool_result(&events, "call-handback-long");
    assert_eq!(error, Some(true), "超长理由要当工具错误：{text}");
    assert!(text.contains("300"), "{text}");
    assert!(
        !task_row
            .reports
            .iter()
            .any(|report| report.attempt_id == attempt.id && report.state == ReportState::Blocked),
        "超长理由不能写成交回报告"
    );
    assert_eq!(attempt.end, None, "没交回，Attempt 不结束");
}

#[tokio::test]
async fn a_failed_report_relays_and_leaves_the_task_open() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    let long = "文档站打不开，已经试过两次。".repeat(70);
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "failed", &long, None));
    fixture.router.push_thin(crate::common::text_sse("失败"));
    wait_until("failed relay", || {
        !waiting_relays(fixture.data()).is_empty()
    })
    .await;
    let relays = waiting_relays(fixture.data());
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "failed");
    assert_eq!(relays[0].1["task"]["stage"], "needs_conditions");
    let summary = relays[0].1["summary"].as_str().unwrap();
    assert_eq!(summary.chars().count(), 600, "摘要截到 600 字");
    assert_eq!(
        relays[0].1["chars"].as_u64(),
        Some(long.chars().count() as u64)
    );
    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    assert!(store.task(&task.id).unwrap().is_some(), "failed 不关闭任务");
}

#[tokio::test]
async fn a_relay_for_another_kind_conversation_stays_later() {
    let fixture = Fixture::new().await;
    fixture.router.push_thin(report_sse(
        1,
        "call-report",
        "ready",
        "另一个对话的答案",
        None,
    ));
    fixture.router.push_thin(crate::common::text_sse("done"));
    let store = tasks_store(fixture.data());
    let draft = tasks::NewTask {
        title: "另一个对话派的任务".to_owned(),
        requirement: "找答案".to_owned(),
        dod: "有答案".to_owned(),
        cwd: fixture.data().to_path_buf(),
        conversation: tasks::ConversationId::new("another-kind-session"),
        model: tasks::ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: service::THIN_TOOLS.map(str::to_owned).to_vec(),
    };
    let task = store.create_task(draft, 1_700_000_000_000).unwrap();
    wait_until("report ready", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        waiting_relays(fixture.data()).is_empty(),
        "发起的 Kind 会话不是当前根会话：先 Later"
    );
    let task = store.task(&task.id).unwrap().unwrap();
    let report = task.latest_report().unwrap();
    assert!(
        store
            .effect_pending(tasks::EffectKind::Relay, report.id.as_str())
            .unwrap(),
        "relay 还在等 Kind 打开"
    );
}

/// 要求版本变过以后，旧版本的报告不能接受：卡上的「接受」回拒绝，接受记录不落，阶段不变。
#[tokio::test]
async fn a_report_from_before_a_requirement_change_is_refused() {
    let fixture = Fixture::new().await;
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            write_file_sse("call-w1", "a.py", "one\n"),
            report_sse(2, "call-report", "ready", "改好了", None),
            crate::common::text_sse("done"),
        ],
    )
    .await;

    let (relay_id, _) = ready_relay(&fixture).await;
    let store = tasks_store(fixture.data());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    store
        .update_write_scope(&task.id, "other.py", now + 1)
        .expect("改写入范围");

    let decision = accept_result_card(&fixture.host, &relay_id).await;
    assert_eq!(decision["phase"], "refused", "{decision}");
    let summary = decision["data"]["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("要求改过了"),
        "拒绝原因写明要求改过了：{summary}"
    );
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.requirement_version(), 2);
    assert!(stored.acceptances.is_empty(), "旧版本的报告不写接受记录");
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::ReadyForReview,
        "拒绝也不改阶段"
    );

    let again = accept_result_card(&fixture.host, &relay_id).await;
    assert_eq!(again, decision, "拒绝也幂等");
    assert_eq!(
        card_decisions(&crate::common::read_root_events(fixture.data()), &relay_id).len(),
        1
    );
}

#[tokio::test]
async fn gate_judge_rejects_stale_report_and_candidate_changed() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, _downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    // 不是门的最新报告：已经变了，不记。
    let stale = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: "deadbeefdeadbeefdeadbeefdeadbeef".to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("旧报告判定");
    let Response::GateJudge(stale) = stale else {
        panic!("gate-judge 响应");
    };
    assert!(stale.changed);
    assert!(stale.verdict.is_none());
    assert_eq!(
        tasks_store(data.path())
            .task(&gate.id)
            .unwrap()
            .unwrap()
            .verdicts
            .len(),
        0
    );

    // B 又交一份新结果 → 候选变了；这时点旧门报告：已经变了，不记。
    router.push_thin_for(
        guarded.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b2-report",
            "task_report",
            r#"{"state":"ready","summary":"第二次结果：5 行"}"#,
        ),
    );
    router.push_thin_for(guarded.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        gate.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-gate2-report",
            "task_report",
            r#"{"state":"ready","summary":"第二次核查"}"#,
        ),
    );
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("好"));
    wait_until("B 第一轮结束", || {
        tasks_store(data.path())
            .task(&guarded.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    wait_until("门第一轮结束", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    host.dispatch(Call::TaskSay(TaskSayRequest {
        task_id: guarded.id.as_str().to_owned(),
        text: "再改一版".to_owned(),
        pastes: Vec::new(),
        attachments: Vec::new(),
    }))
    .await
    .expect("B 第二轮");
    wait_until("门重跑到第二份候选", || {
        tasks_store(data.path())
            .task(&gate.id)
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
                        .is_some_and(|report| report.summary.contains("第二次核查"))
            })
    })
    .await;
    let stale_again = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("候选变后的旧报告判定");
    let Response::GateJudge(stale_again) = stale_again else {
        panic!("gate-judge 响应");
    };
    assert!(stale_again.changed, "候选已经变了");
    let gate_now = tasks_store(data.path()).task(&gate.id).unwrap().unwrap();
    assert!(gate_now.verdicts.is_empty(), "已经变了不记判定");
    assert_eq!(
        gate_now.needs[0].handoff.as_ref().unwrap().summary,
        "第二次结果：5 行"
    );
}

/// ：同一任务后面又交了新结果，旧卡上的接受回「已经变了」，不记。
#[tokio::test]
async fn accepting_an_older_report_after_a_new_one_is_refused() {
    let fixture = Fixture::new().await;
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            report_sse(2, "call-report-1", "ready", "第一次结果", None),
            crate::common::text_sse("好"),
            report_sse(
                2,
                "call-report-2",
                "ready",
                "第二次结果：又核对了一遍",
                None,
            ),
            crate::common::text_sse("好"),
        ],
    )
    .await;
    let (first_relay, _) = ready_relay(&fixture).await;
    fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "再核对一遍".to_owned(),
            pastes: Vec::new(),
            attachments: Vec::new(),
        }))
        .await
        .expect("再核对");
    wait_until("第二张结果卡 relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .filter(|(id, body)| {
                id.starts_with("task-report:") && body["task"]["id"] == task.id.as_str()
            })
            .count()
            >= 2
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let second_relay = relays
        .iter()
        .map(|(id, _)| id.clone())
        .find(|id| id != &first_relay && id.starts_with("task-report:"))
        .expect("第二张结果卡");
    assert_ne!(first_relay, second_relay);

    let decision = accept_result_card(&fixture.host, &first_relay).await;
    assert_eq!(decision["phase"], "refused", "{decision}");
    let summary = decision["data"]["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("已经变了"), "回原因：{summary}");
    let store = tasks_store(fixture.data());
    assert!(
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .acceptances
            .is_empty()
    );
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::ReadyForReview,
        "拒绝不改阶段"
    );

    // 最新那张能接受。
    let decision = accept_result_card(&fixture.host, &second_relay).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
}

/// ：组接受每记下一项，给那份报告的 relay 卡补一条 card/decision（Kind 卡上回执）。
#[tokio::test]
async fn task_accept_many_records_a_card_decision_for_each_accepted_report() {
    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let (task_id, report_id) =
        seed_ready_task(&store, fixture.data(), "组接受回执", 1_800_100_000_000);
    let response = fixture
        .host
        .dispatch(Call::TaskAcceptMany(service::TaskAcceptManyRequest {
            picks: vec![service::work::WorkPick {
                task_id: task_id.as_str().to_owned(),
                report_id: report_id.as_str().to_owned(),
                version: 1,
            }],
        }))
        .await
        .expect("task-accept-many");
    let Response::TaskAcceptMany(response) = response else {
        panic!("task-accept-many 响应");
    };
    assert!(response.outcomes[0].refused.is_none());

    let call_id = format!("task-report:{}:{}", task_id.as_str(), report_id.as_str());
    let events = crate::common::read_root_events(fixture.data());
    let decision = events.iter().find_map(|event| {
        let session::EventKind::CardDecision {
            call_id: event_call,
            action,
            phase,
            data,
            ..
        } = &event.kind
        else {
            return None;
        };
        (event_call == &call_id).then(|| (action.clone(), *phase, data.clone()))
    });
    let (action, phase, data) = decision.expect("接受后应补 card/decision");
    assert_eq!(action.as_deref(), Some("accept_task_result"));
    assert_eq!(phase, session::CardDecisionPhase::Confirmed);
    let data = data.expect("回执 data");
    assert_eq!(data["summary"], "你接受了");
    assert_eq!(data["report_id"], report_id.as_str());
}
