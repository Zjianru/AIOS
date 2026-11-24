//! 重启恢复：中断任务、恢复卡、确认/忽略、恢复后的说法。

use super::*;

/// 保存在 settings.json 里的暂停在重启后生效：任务只排队，不开会话。
#[tokio::test]
async fn a_saved_pause_holds_launches_after_a_restart() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut saved = service::settings::load(data.path());
    saved.exec.paused = true;
    service::settings::save(data.path(), &saved).expect("保存暂停设置");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;

    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    assert!(task.latest_attempt().is_none());
    assert!(router.bodies(true).is_empty(), "重启后暂停仍然生效");
    let store = tasks_store(data.path());
    assert!(
        store
            .effect_pending(EffectKind::Launch, task.id.as_str())
            .unwrap(),
        "任务在排队"
    );
}

/// 停/结束后在任务对话里说话：接在原会话上开新 Attempt，请求带着原来的历史。
#[tokio::test]
async fn saying_after_a_report_resumes_the_same_conversation_and_carries_history() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    for _ in 0..3 {
        fixture.router.push_kind(crate::common::text_sse("收到"));
    }
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "第一轮的答案", None));
    fixture
        .router
        .push_thin(crate::common::text_sse("第一轮收尾"));
    let store = tasks_store(fixture.data());
    wait_until("第一轮报 ready", || reported_ready(&store, &task.id)).await;

    let before = store.task(&task.id).unwrap().unwrap();
    let first = before.latest_attempt().cloned().expect("第一轮 Attempt");
    let conversation = first.conversation.clone().expect("会话 id");
    let attempts_before = attempt_count(&before);

    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("接着做"));
    let said = say_in_task_dialog(&fixture.host, &task, "再接着找").await;
    assert_eq!(
        said.new_attempt,
        Some(first.id.0 + 1),
        "回执预览下一次 Attempt"
    );

    wait_until("接着做开了新 Attempt", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) > attempts_before)
    })
    .await;
    let after = store.task(&task.id).unwrap().unwrap();
    let second = after.latest_attempt().expect("第 2 次 Attempt");
    assert_eq!(second.id.0, first.id.0 + 1);
    assert_eq!(
        second.conversation.as_ref(),
        Some(&conversation),
        "会话 id 不变"
    );
    assert_eq!(second.trigger, tasks::AttemptTrigger::UserMessage);
    assert!(
        second.ended_at.is_none(),
        "新 Attempt 还没结束（{:?} / {:?}）",
        second.end,
        store.effect_health(EffectKind::Delivery).unwrap()
    );

    wait_until("接着做的请求带上新句子", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("再接着找"))
    })
    .await;
    let resumed = fixture
        .router
        .bodies(true)
        .last()
        .cloned()
        .expect("接着做的请求");
    assert!(
        resumed.contains("第一轮的答案"),
        "接着做的请求带着原来的历史"
    );
}

/// 先在任务对话里说话，再点卡上那一项：reconcile 回「已经接着做了」，只跑一个 Attempt。
#[tokio::test]
async fn talking_in_the_task_dialog_before_confirming_says_already_resumed() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let task_refs: Vec<&Task> = crashed.iter().collect();
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    assert_eq!(
        store
            .task(&crashed[0].id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .end,
        Some(AttemptEnd::Interrupted),
        "启动扫描先把上次的 Attempt 收尾"
    );
    wait_until("接续卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    // 用户在任务对话里先说一句：打断后的说话按接续开新 Attempt，第一条带着恢复说明。
    router.push_thin_for(crashed[0].id.as_str(), crate::common::text_sse("收到"));
    router.push_kind(crate::common::text_sse("收到"));
    say_in_task_dialog(&host, &crashed[0], "把没跑完的补上").await;
    wait_until("任务对话先接上了", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&crashed[0].id).unwrap().unwrap();
    let attempt = resumed.latest_attempt().cloned().expect("新 Attempt");
    assert_eq!(attempt.trigger, tasks::AttemptTrigger::Resume);
    wait_until("恢复说明进了任务会话", || {
        recovery_notes(&thin_events(data.path(), &attempt)).len() == 1
    })
    .await;
    let notes = recovery_notes(&thin_events(data.path(), &attempt));
    assert_eq!(
        notes[0].0,
        format!("resume:{}:{}", crashed[0].id, attempt.id),
        "恢复说明的判重键"
    );
    let note_at = notes[0]
        .1
        .find(RECOVERY_NOTE)
        .expect("恢复说明在同一条消息里");
    let words_at = notes[0]
        .1
        .find("把没跑完的补上")
        .expect("用户的话在同一条消息里");
    assert!(note_at < words_at, "恢复说明在用户的话前面：{}", notes[0].1);

    let decision = confirm_card_option(&host, "call-restart-card", crashed[0].id.as_str()).await;
    assert_eq!(decision["phase"], "refused", "{decision}");
    let summary = decision["data"]["summary"].as_str().expect("拒绝原因");
    assert!(summary.contains("已经接着做了"), "{summary}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        attempt_count(&store.task(&crashed[0].id).unwrap().unwrap()),
        2,
        "只跑一个 Attempt"
    );
    drop(host);
}

/// 全局暂停开着时重启：汇报不送；开闸后送一条，而且只有一条。
#[tokio::test]
async fn a_paused_restart_waits_for_the_gate_then_reports_once() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let task_refs: Vec<&Task> = crashed.iter().collect();
    let requests_before = router.bodies(false).len();
    let mut saved = service::settings::load(data.path());
    saved.exec.paused = true;
    service::settings::save(data.path(), &saved).expect("保存暂停");
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;

    // 扫描照做；效果不送。
    assert_eq!(
        store
            .task(&crashed[0].id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .end,
        Some(AttemptEnd::Interrupted)
    );
    wait_until("restart 留在库里", || {
        store
            .effect_health(EffectKind::Restart)
            .expect("效果健康")
            .waiting
            == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        restart_messages(&crate::common::read_root_events(data.path())).is_empty(),
        "暂停时不送 restart"
    );
    assert_eq!(
        router.bodies(false).len(),
        requests_before,
        "暂停时不叫模型"
    );

    host.dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
        currency: None,
        paused: Some(false),
        seats: None,
        bash: None,
    }))
    .await
    .expect("开闸");
    wait_until("开闸后汇报", || {
        restart_messages(&crate::common::read_root_events(data.path())).len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        restart_messages(&crate::common::read_root_events(data.path())).len(),
        1
    );
    drop(host);
}

/// 模型不可用时汇报按 Later 留在库里：不算失败，不发请求，等模型恢复。
#[tokio::test]
async fn a_restart_report_stays_later_while_the_model_is_unavailable() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let task = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&task)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[task]);
    let requests_before = router.bodies(false).len();
    let factory: service::ModelFactory = Arc::new(|_selection| Err("还没有接模型".to_owned()));
    let host = open_host_with(&server, data.path(), cwd.path(), |options| {
        options.actions = Arc::new(service::ActionRegistry::settings(
            data.path().to_path_buf(),
            options.identity.clone(),
        ));
        options.model_factory = Some(factory.clone());
    })
    .await;
    assert!(
        !matches!(host.info().root, service::RootState::Ok),
        "模型不可用：根会话起不来"
    );
    assert_eq!(
        store
            .task(&crashed[0].id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .end,
        Some(AttemptEnd::Interrupted),
        "扫描照做"
    );
    wait_until("restart 留在库里", || {
        store
            .effect_health(EffectKind::Restart)
            .expect("效果健康")
            .waiting
            == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let health = store.effect_health(EffectKind::Restart).expect("效果健康");
    assert_eq!(health.waiting, 1, "Later 不丢效果");
    assert_eq!(health.attempts, 0, "Later 不算失败");
    assert_eq!(
        router.bodies(false).len(),
        requests_before,
        "模型不可用不发请求"
    );
    assert!(restart_messages(&crate::common::read_root_events(data.path())).is_empty());
    drop(host);
}

/// 汇报里的 last_report 是最近一份报告，超过 300 字截断；attempts 是总次数。
#[tokio::test]
async fn a_restart_report_carries_the_last_report_capped_at_300_characters() {
    let (server, router, data, cwd) = restart_setup().await;
    let store = tasks_store(data.path());
    let long = "报".repeat(400);
    let draft = tasks::NewTask {
        quote: None,
        quote_source: None,
        kind_note: None,
        project_id: None,
        cwd_source: None,
        title: "有旧报告的".to_owned(),
        requirement: "找答案".to_owned(),
        dod: "有答案".to_owned(),
        cwd: data.path().to_path_buf(),
        conversation: tasks::ConversationId::new("kind-root"),
        model: tasks::ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: service::THIN_TOOLS.map(str::to_owned).to_vec(),
    };
    let task = store.create_task(draft, 1_700_000_000_000).unwrap();
    let attempt = store.begin_attempt(&task.id, 1_700_000_000_001).unwrap();
    store
        .attach_session(
            &task.id,
            attempt,
            tasks::ConversationId::new("task-session"),
        )
        .unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            tasks::NewReport {
                state: ReportState::Waiting,
                origin: tasks::ReportOrigin::Executor,
                summary: long.clone(),
                question: None,
            },
            1_700_000_000_002,
        )
        .unwrap();
    // 报告之后又记了输入：启动扫描按「报告后又输入」记 interrupted。
    store
        .say_input(
            &task.id,
            tasks::NewInput {
                text: "继续".to_owned(),
                ..tasks::NewInput::default()
            },
            1_700_000_000_003,
        )
        .unwrap();

    router.push_kind(crate::common::text_sse("收到"));
    let _host = open_host(&server, data.path(), cwd.path()).await;
    wait_until("Kind 收到重启汇报", || {
        !restart_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;
    let reports = restart_messages(&crate::common::read_root_events(data.path()));
    assert_eq!(reports.len(), 1);
    let entry = &reports[0].1["tasks"][0];
    assert_eq!(entry["id"], task.id.as_str());
    assert_eq!(entry["attempts"], 1);
    let summary = entry["last_report"].as_str().expect("最近报告");
    assert_eq!(summary.chars().count(), 300, "最多 300 字");
    assert!(summary.chars().all(|ch| ch == '报'));
}

/// 越界交回后，用户在 Kind 对话里同意改范围：task_update 落第 2 版要求，
/// 原会话开新 Attempt（trigger requirement_changed），第一条输入写明新范围，新范围能写。
#[tokio::test]
async fn task_update_records_a_new_version_and_resumes_with_the_new_scope() {
    let fixture = Fixture::new().await;
    let target = fixture.cwd.path().join("target.py");
    let outside = fixture.cwd.path().join("outside.py");
    std::fs::write(&target, "inside\n").unwrap();

    // 第一个 Attempt：越界写被拒，交回。
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-write-1",
        "write",
        &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::tool_use_sse(
        2,
        "call-handback",
        "task_handback",
        &serde_json::json!({"reason": "outside.py 不在写入范围里"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::text_sse("交回"));
    // 第二个 Attempt：范围扩大后写成功、报 ready。
    fixture.router.push_thin(crate::common::tool_use_sse(
        3,
        "call-write-2",
        "write",
        &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
    ));
    fixture
        .router
        .push_thin(report_sse(4, "call-report", "ready", "改完了", None));
    fixture.router.push_thin(crate::common::text_sse("done"));

    let task = fixture.accept_write(true, "target.py").await;
    // 交回 relay 会叫醒 Kind 一轮；先给它一条备用回复。
    fixture.router.push_kind(crate::common::text_sse("收到卡"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("交回 relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;

    let scope = std::fs::canonicalize(fixture.cwd.path())
        .unwrap()
        .join("outside.py");
    let update_args = serde_json::json!({
        "task_id": task.id.as_str(),
        "write_scope": "outside.py"
    })
    .to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        5,
        "call-update",
        "task_update",
        &update_args,
    ));
    fixture
        .router
        .push_kind(crate::common::text_sse("好，范围扩大了"));
    // 新 Attempt 报 ready 之后还会叫醒 Kind 一轮。
    fixture
        .router
        .push_kind(crate::common::text_sse("收到结果"));
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-2",
            "我同意把写入范围扩大到 outside.py",
        )))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 3).await;

    let store = tasks_store(fixture.data());
    wait_until("第二个 Attempt 报 ready", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            task.requirement_version() == 2
                && attempt_count(&task) == 2
                && task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;

    let updated = store.task(&task.id).unwrap().unwrap();
    assert_eq!(updated.versions.len(), 2, "{:?}", updated.versions);
    assert_eq!(
        updated.versions[1].write_scope.as_deref(),
        Some(scope.as_path())
    );
    assert_eq!(
        updated.versions[1].source,
        tasks::VersionSource::UserAgreed,
        "第 2 版来自用户在 Kind 对话里的同意"
    );
    let first = updated.runs[0].attempts[0].clone();
    let second = updated.latest_attempt().expect("新 Attempt").clone();
    assert_eq!(second.id.0, first.id.0 + 1);
    assert_eq!(second.trigger, tasks::AttemptTrigger::RequirementChanged);
    assert_eq!(
        second.conversation, first.conversation,
        "还在原任务会话里接着做"
    );

    let events = thin_events(fixture.data(), &second);
    let request_id = format!("task-update:{}:{}", updated.id, second.id);
    let note = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => match &message.source {
                model::MessageSource::User { rpc_id: Some(id) } if id == &request_id => Some(
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                ),
                _ => None,
            },
            _ => None,
        })
        .expect("新 Attempt 的第一条输入");
    assert!(
        note.contains(&scope.display().to_string()),
        "第一条输入写明新范围：{note}"
    );
    assert!(note.contains("写入范围"), "{note}");

    let (write_text, write_error) = tool_result(&events, "call-write-2");
    assert_eq!(write_error, None, "{write_text}");
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "changed\n",
        "新范围里的文件写成了"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "inside\n");

    let root_events = crate::common::read_root_events(fixture.data());
    let (update_text, update_error) = tool_result(&root_events, "call-update");
    assert_eq!(update_error, None, "{update_text}");
    assert!(update_text.contains("第 2 版"), "{update_text}");
    assert!(
        update_text.contains(&scope.display().to_string()),
        "{update_text}"
    );
}

/// 工作树任务跨一次服务重启：任务行里的工作树还在，停后说话仍开在同一棵树，不再 add。
#[tokio::test]
async fn a_worktree_task_keeps_its_tree_across_a_service_restart() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-read-1",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-write-1",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "first\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(3, "call-report-1", "ready", "第一轮", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));
    let task = fixture.accept_args(false, worktree_task_args(), 1).await;
    let store = tasks_store(fixture.data.path());
    wait_until("worktree 第一轮 ready", || {
        reported_ready(&store, &task.id)
    })
    .await;
    let before = store.task(&task.id).unwrap().unwrap();
    let first = before.latest_attempt().cloned().expect("第一个 Attempt");
    let worktree_cwd = worktree_cwd(&fixture, &task);
    let listed_before = listed_worktrees(fixture.cwd.path());
    wait_until("第一轮收尾", || {
        session_turn_ended(fixture.data.path(), &first)
    })
    .await;
    wait_until("ready relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data.path())).is_empty()
    })
    .await;

    // 模拟进程退出：换一个新根宿主，任务行里的工作树要还在。
    fixture.host.task_runtime().expect("runtime").crash();
    drop(fixture.host);
    let host = reopen_actions_host(&fixture._server, fixture.data.path(), fixture.cwd.path()).await;

    fixture.router.push_thin_for(
        task.id.as_str(),
        crate::common::tool_use_sse(
            4,
            "call-read-2",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for(
        task.id.as_str(),
        crate::common::tool_use_sse(
            5,
            "call-write-2",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "second\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for(
        task.id.as_str(),
        report_sse(6, "call-report-2", "ready", "第二轮", None),
    );
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("done"));
    fixture.router.push_kind(crate::common::text_sse("收到"));
    let said = say_in_task_dialog(&host, &task, "再改一下").await;
    assert_eq!(said.new_attempt, Some(first.id.0 + 1));
    wait_until("重启后的第二个 Attempt 报 ready", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            attempt_count(&task) == 2
                && task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;

    let after = store.task(&task.id).unwrap().unwrap();
    let second = after.latest_attempt().expect("第二个 Attempt");
    assert_eq!(second.trigger, tasks::AttemptTrigger::UserMessage);
    wait_until("重启后接着做的请求", || {
        fixture
            .router
            .bodies(true)
            .iter()
            .any(|body| body.contains("再改一下"))
    })
    .await;
    let resumed = fixture
        .router
        .bodies(true)
        .into_iter()
        .find(|body| body.contains("再改一下"))
        .expect("接着做的请求");
    assert_eq!(
        request_working_directory(&resumed),
        worktree_cwd.display().to_string(),
        "重启后的新 Attempt 的 cwd 仍在工作树里"
    );
    let second_events = thin_events(fixture.data.path(), second);
    let (write_text, write_error) = tool_result(&second_events, "call-write-2");
    assert_eq!(write_error, None, "重启后写失败：{write_text}");
    assert_eq!(
        std::fs::read_to_string(worktree_cwd.join("app.py")).unwrap(),
        "second\n",
        "重启后写进同一个工作树"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("sub").join("app.py")).unwrap(),
        "old\n",
        "原目录文件不改"
    );
    assert_eq!(
        listed_worktrees(fixture.cwd.path()),
        listed_before,
        "重启后不再 git worktree add"
    );
}

/// 工作树任务交回、用户在 Kind 对话里同意扩大范围：新 Attempt 还在同一个工作树，
/// 新范围按同样的相对位置换算，第一条输入写明换算后的范围。
#[tokio::test]
async fn task_update_in_a_worktree_resumes_with_the_converted_scope() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    // 第一个 Attempt：越界写被拒、交回。
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-write-1",
            "write",
            &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-handback",
            "task_handback",
            &serde_json::json!({"reason": "outside.py 不在写入范围里"}).to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("交回"));
    // 第二个 Attempt：范围扩大后写成功、报 ready。
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            3,
            "call-write-2",
            "write",
            &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(4, "call-report", "ready", "改完了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));

    let task = fixture.accept_args(true, worktree_task_args(), 0).await;
    // 交回 relay 会叫醒 Kind 一轮。
    fixture.router.push_kind(crate::common::text_sse("收到卡"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("交回 relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;
    let worktree_cwd = worktree_cwd(&fixture, &task);
    let scope = worktree_cwd.join("outside.py");

    // 用户在 Kind 对话里同意扩大范围。
    let update_args = serde_json::json!({
        "task_id": task.id.as_str(),
        "write_scope": "outside.py"
    })
    .to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        5,
        "call-update",
        "task_update",
        &update_args,
    ));
    fixture
        .router
        .push_kind(crate::common::text_sse("好，范围扩大了"));
    fixture
        .router
        .push_kind(crate::common::text_sse("收到结果"));
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-2",
            "我同意把写入范围扩大到 outside.py",
        )))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 3).await;

    let store = tasks_store(fixture.data());
    wait_until("第二个 Attempt 报 ready", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            task.requirement_version() == 2
                && attempt_count(&task) == 2
                && task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;

    let updated = store.task(&task.id).unwrap().unwrap();
    let second = updated.latest_attempt().expect("第二个 Attempt").clone();
    assert_eq!(second.trigger, tasks::AttemptTrigger::RequirementChanged);
    let events = thin_events(fixture.data(), &second);
    let request_id = format!("task-update:{}:{}", updated.id, second.id);
    let note = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => match &message.source {
                model::MessageSource::User { rpc_id: Some(id) } if id == &request_id => Some(
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                ),
                _ => None,
            },
            _ => None,
        })
        .expect("新 Attempt 的第一条输入");
    assert!(
        note.contains(&scope.display().to_string()),
        "第一条输入写明换算后的范围：{note}"
    );
    assert!(note.contains("写入范围"), "{note}");

    let (write_text, write_error) = tool_result(&events, "call-write-2");
    assert_eq!(write_error, None, "{write_text}");
    assert_eq!(
        std::fs::read_to_string(&scope).unwrap(),
        "changed\n",
        "新范围里的文件在工作树里写成"
    );
    assert!(
        !fixture.cwd.path().join("sub").join("outside.py").exists(),
        "原目录没多出文件"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("sub").join("app.py")).unwrap(),
        "old\n",
        "原目录文件不改"
    );
    assert_eq!(listed_worktrees(fixture.cwd.path()).len(), 2);
}

/// A 报回 ready 的那一刻崩溃——重启后 B 照样被放出，且只放一次（效果表重放）。
#[tokio::test]
async fn graph_crash_after_ready_releases_downstream_exactly_once_after_restart() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);
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
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),

        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");
    router.push_thin_for(
        children[0].id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-a-report",
            "task_report",
            r#"{"state":"ready","summary":"查完了：notes.md 有 3 行"}"#,
        ),
    );
    router.push_thin_for(
        children[1].id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b-report",
            "task_report",
            r#"{"state":"ready","summary":"写好了：other.txt 里是 3 行"}"#,
        ),
    );
    // A 的报告和「放出 B」的 launch 已经落库；relay 还没投递就崩溃。
    runtime.crash_before_next_relay();
    runtime.set_pump_paused(false);
    wait_until("A 的 ready 落库", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("B 的 launch 留在队列里", || {
        tasks_store(data.path())
            .effect_pending(EffectKind::Launch, children[1].id.as_str())
            .unwrap_or(false)
    })
    .await;
    drop(host);
    for _ in 0..8 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    let mut reopened = false;
    for _ in 0..250 {
        let host = open_graph_host(&server, data.path(), cwd.path()).await;
        if matches!(host.info().root, service::RootState::Ok) {
            reopened = true;
            wait_until("重启后 B 开跑", || {
                tasks_store(data.path())
                    .task(&children[1].id)
                    .ok()
                    .flatten()
                    .and_then(|task| task.latest_attempt().cloned())
                    .is_some_and(|attempt| attempt.conversation.is_some())
            })
            .await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let store = tasks_store(data.path());
            let b = store.task(&children[1].id).expect("读任务").expect("B");
            let attempts = b.runs.iter().map(|run| run.attempts.len()).sum::<usize>();
            assert_eq!(attempts, 1, "崩溃重放只放一次");
            assert!(
                !store
                    .effect_pending(EffectKind::Launch, children[1].id.as_str())
                    .unwrap_or(false),
                "没有残留的 launch"
            );
            drop(host);
            break;
        }
        drop(host);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reopened, "重开根宿主 250 次都没到 RootState::Ok");
}

/// F1：上游二次 ready 更新摘要后，下游第二次 Attempt 的会话里随接续带上「上游结果（最新）」。
#[tokio::test]
async fn resume_attempt_rehands_the_latest_upstream_result() {
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
    wait_until("建图并出卡", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),

        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");

    let report_a1 = crate::common::tool_use_sse(
        2,
        "call-a1-report",
        "task_report",
        r#"{"state":"ready","summary":"第一次结果：notes.md 有 3 行"}"#,
    );
    let report_b1 = crate::common::tool_use_sse(
        2,
        "call-b1-report",
        "task_report",
        r#"{"state":"ready","summary":"第一版写好了"}"#,
    );
    router.push_thin_for(children[0].id.as_str(), report_a1);
    router.push_thin_for(children[0].id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(children[1].id.as_str(), report_b1);
    router.push_thin_for(children[1].id.as_str(), crate::common::text_sse("好"));
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    wait_until("A1 ready", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("B1 开跑", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .and_then(|task| {
                task.latest_attempt()
                    .and_then(|attempt| attempt.conversation.clone())
            })
            .is_some()
    })
    .await;
    wait_until("A1 轮结束", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;

    // A 第二次 ready：更新交接摘要。
    let report_a2 = crate::common::tool_use_sse(
        2,
        "call-a2-report",
        "task_report",
        r#"{"state":"ready","summary":"第二次结果：notes.md 现在 5 行"}"#,
    );
    router.push_thin_for(children[0].id.as_str(), report_a2);
    router.push_thin_for(children[0].id.as_str(), crate::common::text_sse("好"));
    host.dispatch(Call::TaskSay(service::TaskSayRequest {
        task_id: children[0].id.as_str().to_owned(),
        text: "再查一遍".to_owned(),
        pastes: Vec::new(),
        attachments: Vec::new(),
    }))
    .await
    .expect("A2 输入");
    wait_until("A2 ready 且摘要更新", || {
        tasks_store(data.path())
            .task(&children[0].id)
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

    // B 第二次 Attempt：接续时应该带上最新的上游结果。
    wait_until("B1 轮结束", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    let report_b2 = crate::common::tool_use_sse(
        2,
        "call-b2-report",
        "task_report",
        r#"{"state":"ready","summary":"照最新的 5 行写好了"}"#,
    );
    router.push_thin_for(children[1].id.as_str(), report_b2);
    router.push_thin_for(children[1].id.as_str(), crate::common::text_sse("好"));
    host.dispatch(Call::TaskSay(service::TaskSayRequest {
        task_id: children[1].id.as_str().to_owned(),
        text: "照最新的再写一遍".to_owned(),
        pastes: Vec::new(),
        attachments: Vec::new(),
    }))
    .await
    .expect("B2 输入");
    wait_until("B2 开跑", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
            })
    })
    .await;
    let b = tasks_store(data.path())
        .task(&children[1].id)
        .expect("读任务")
        .expect("B");
    assert_eq!(
        b.runs.iter().map(|run| run.attempts.len()).sum::<usize>(),
        2,
        "A2 不再重新放 B（只有两次 Attempt）"
    );
    let conversation = b
        .latest_attempt()
        .and_then(|attempt| attempt.conversation.clone())
        .expect("B2 会话");
    let session_id = session::SessionId::parse(conversation.as_str()).expect("会话 id");
    // 固定块先于 assignment 落日志，读日志前等交接消息真的写进去。
    for _ in 0..500 {
        let session =
            session::SessionStore::open_read_only(data.path(), &session_id).expect("B2 日志");
        let found = session.events().iter().any(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => message
                .content
                .iter()
                .filter_map(|block| match block {
                    model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
                .contains("上游结果（最新）"),
            _ => false,
        });
        if found {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let session = session::SessionStore::open_read_only(data.path(), &session_id).expect("B2 日志");
    let texts: Vec<String> = session
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("上游结果（最新）") && text.contains("5 行")),
        "B2 会话里要有最新的上游结果：{texts:?}"
    );
}

/// 判定刚落库就崩溃 → 重启后下游恰好放一次、判定 relay 恰好送一次。
#[tokio::test]
async fn gate_verdict_crash_replays_downstream_launch_and_relay_once() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    let guarded_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();

    // 判定事务提交后、relay 投递前崩溃。
    host.task_runtime()
        .expect("runtime")
        .crash_before_next_relay();
    let response = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("判定");
    let Response::GateJudge(judged) = response else {
        panic!("gate-judge 响应");
    };
    assert!(!judged.changed, "判定要记下");
    // 下游的 launch 还在效果表里（或崩溃前已经被领走，但绝不该重复）。
    wait_until("C 的 launch 留在队列或已开跑", || {
        let store = tasks_store(data.path());
        store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap_or(false)
            || store
                .task(&downstream.id)
                .ok()
                .flatten()
                .is_some_and(|task| task.latest_attempt().is_some())
    })
    .await;
    drop(host);
    // 重开根宿主（模拟进程重启）。
    let mut reopened = false;
    for _ in 0..250 {
        let host = open_graph_host(&server, data.path(), cwd.path()).await;
        if matches!(host.info().root, service::RootState::Ok) {
            reopened = true;
            wait_until("重启后 C 开跑", || {
                tasks_store(data.path())
                    .task(&downstream.id)
                    .ok()
                    .flatten()
                    .and_then(|task| task.latest_attempt().cloned())
                    .is_some_and(|attempt| attempt.conversation.is_some())
            })
            .await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let store = tasks_store(data.path());
            let c = store.task(&downstream.id).unwrap().unwrap();
            let attempts = c.runs.iter().map(|run| run.attempts.len()).sum::<usize>();
            assert_eq!(attempts, 1, "崩溃重放只放一次");
            assert_eq!(
                c.needs[0]
                    .handoff
                    .as_ref()
                    .map(|handoff| handoff.report_id.clone()),
                Some(guarded_report.id.clone()),
                "交接的是判通过的那份"
            );
            assert_eq!(
                store.task(&gate.id).unwrap().unwrap().verdicts.len(),
                1,
                "判定只记一次"
            );
            // 判定 relay 恰好一次。
            let count_verdict_relay = || {
                crate::common::read_root_events(data.path())
                    .iter()
                    .filter(|event| {
                        let session::EventKind::UserMessage { message, .. } = &event.kind else {
                            return false;
                        };
                        matches!(
                            &message.source,
                            model::MessageSource::User { rpc_id: Some(rpc_id) }
                                if rpc_id.starts_with("gate-verdict:")
                        )
                    })
                    .count()
            };
            wait_until("判定 relay 送到", || count_verdict_relay() >= 1).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(count_verdict_relay(), 1, "判定 relay 只送一次");
            drop(host);
            break;
        }
        drop(host);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reopened, "重开根宿主 250 次都没到 RootState::Ok");
}
