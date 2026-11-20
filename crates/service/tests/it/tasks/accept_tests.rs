//! 受理与读任务：幂等、冻结、状态/页头、ThinAgent 工具面、记忆注入。

use super::*;

#[tokio::test]
async fn accept_freezes_the_task_and_the_launch_uses_it() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 存 Exa key 只是为了加载 web skill 后两件联网工具能开出（条件照旧），不真的联网。
    credentials::Keyring::open(data.path(), Some(&data.path().join("identity.txt")))
        .unwrap()
        .save_exa_key("test-exa-key")
        .unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    let args = serde_json::json!({
        "title": "查一下 Exa 怎么用",
        "requirement": "读官方文档，给出能操作的例子",
        "dod": "给出例子和出处"
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    // 同一个 ToolCall id 再放一次：受理要幂等。
    router.push_kind(crate::common::tool_use_sse(
        1,
        "call-accept",
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));

    host.dispatch(Call::Send(send(&host, "kind-1", "查一下 Exa 怎么用")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;

    let store = tasks_store(data.path());
    let task = one_task(&store);
    let root_id = host.info().session_id.expect("根会话 id");
    assert_eq!(task.conversation.as_str(), root_id);
    assert_eq!(task.cwd, std::fs::canonicalize(cwd.path()).unwrap());
    assert_eq!(task.model.model, "deepseek-flash");
    assert_eq!(task.tools, service::THIN_TOOLS.map(str::to_owned).to_vec());

    let events = crate::common::read_root_events(data.path());
    let results: Vec<String> = events
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
        .collect();
    assert!(results[0].contains("已派出任务"), "{results:?}");
    assert!(
        results[1].contains("Already accepted as") && results[1].contains(task.id.as_str()),
        "{results:?}"
    );

    // Kind 的工具表：有 task_accept/task_status，没有 task_report。
    let kind_bodies = router.bodies(false);
    let names = tool_names(&kind_bodies[0]);
    assert!(names.contains(&"task_accept".to_owned()), "{names:?}");
    assert!(names.contains(&"task_status".to_owned()), "{names:?}");
    assert!(!names.contains(&"task_report".to_owned()), "{names:?}");
    assert!(
        !kind_bodies[0].contains("investigator"),
        "默认角色的 skill 不能进 Kind"
    );

    // 受理后改默认模型；launch 仍要用受理时冻结的那个。
    host.dispatch(Call::SessionModelSelect(SessionModelSelectRequest {
        connection: None,
        model: Some("deepseek-pro".to_owned()),
        effort_wanted: None,
    }))
    .await
    .expect("换模型");
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(false);
    router.push_thin(crate::common::tool_use_sse(
        2,
        "call-skill",
        "skill",
        &serde_json::json!({"name": "web"}).to_string(),
    ));
    router.push_thin(crate::common::tool_use_sse(
        3,
        "call-report",
        "task_report",
        &serde_json::json!({"state": "ready", "summary": "官方文档给了一个例子。"}).to_string(),
    ));
    router.push_thin(crate::common::text_sse("done"));

    let store = tasks_store(data.path());
    wait_until("ready 报告", || {
        let task = store.task(&task.id).ok().flatten();
        task.and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("Kind 收到 relay", || {
        !relay_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;

    let task = store.task(&task.id).unwrap().unwrap();
    let attempt = task.latest_attempt().expect("Attempt");
    let conversation = attempt.conversation.clone().expect("会话 id");
    let id = session::SessionId::parse(conversation.as_str()).unwrap();
    let thin_session = session::SessionStore::open_read_only(data.path(), &id).expect("会话日志");
    let header: Value = serde_json::from_str(
        std::fs::read_to_string(thin_session.path())
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        header["cwd"],
        std::fs::canonicalize(cwd.path()).unwrap().to_str().unwrap()
    );
    assert!(
        thin_session.events().iter().any(|event| {
            matches!(
                &event.kind,
                session::EventKind::UserMessage { message, .. }
                    if matches!(&message.source, model::MessageSource::User { rpc_id: Some(id) } if id == &format!("assignment:{}", task.id))
            )
        }),
        "第一条消息的 request id 是 assignment:<taskId>"
    );

    let thin_bodies = router.bodies(true);
    assert!(!thin_bodies.is_empty(), "ThinAgent 至少发了一次请求");
    for body in &thin_bodies {
        let request: Value = serde_json::from_str(body).unwrap();
        assert_eq!(request["model"], "deepseek-flash", "冻结的模型");
    }
    // 第一份请求只开出无条件的 7 件；加载 web skill（Exa key 已存）后开出 9 件。
    let loaded = thin_bodies
        .iter()
        .find(|body| body.contains("\"name\":\"web_search\""))
        .expect("加载 web 后的请求");
    let mut names = tool_names(loaded);
    names.sort();
    let mut expected: Vec<String> = service::THIN_TOOLS.map(str::to_owned).to_vec();
    expected.sort();
    assert_eq!(names, expected, "ThinAgent 的工具表正好 9 件");
    let first_names = tool_names(&thin_bodies[0]);
    assert!(!first_names.contains(&"bash".to_owned()), "{first_names:?}");
    assert!(
        !first_names.contains(&"write".to_owned()),
        "{first_names:?}"
    );
    assert!(
        thin_bodies[0].contains("investigator"),
        "默认角色的 skill 要进 ThinAgent"
    );

    let relays = relay_messages(&crate::common::read_root_events(data.path()));
    assert_eq!(relays.len(), 1, "Kind 只收到一条 relay：{relays:?}");
    let body = &relays[0].1;
    assert_eq!(body["kind"], "task_report");
    assert_eq!(body["state"], "ready");
    assert_eq!(body["task"]["id"], task.id.as_str());
    assert_eq!(body["task"]["stage"], "ready_for_review");
    assert_eq!(body["summary"], "官方文档给了一个例子。");
}

/// 装了任务执行者：Kind 第一个请求的 system 以派任务说明结尾。
#[tokio::test]
async fn the_kind_system_prompt_ends_with_the_task_policy() {
    let fixture = Fixture::new().await;
    fixture.router.push_kind(crate::common::text_sse("好"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-1", "你好")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;

    let bodies = fixture.router.bodies(false);
    assert_eq!(bodies.len(), 1, "Kind 第一个请求");
    let system = request_system(&bodies[0]);
    assert!(
        system.ends_with(EXPECTED_KIND_POLICY),
        "Kind 的系统提示要以内核策略结尾：{system}"
    );
}

/// 没装任务执行者（CLI/老测试）：不给这段说明，它提到 Kind 没有的工具。
#[tokio::test]
async fn a_root_session_without_a_task_runtime_has_no_task_policy() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut options = crate::common::options(data.path());
    options.cwd = cwd.path().to_path_buf();
    options.model.base_url = server.uri();
    options.config = AgentConfig {
        instruction_roots: vec![data.path().join("agents-home")],
        skill_roots: vec![data.path().join("skills-home")],
        ..AgentConfig::default()
    };
    let root = service::open_root_session(options).await.expect("根会话");
    let host = service::ServiceHost::new(root.agent.clone());
    router.push_kind(crate::common::text_sse("好"));
    host.send(SendRequest {
        request_id: "kind-1".to_owned(),
        content: "你好".to_owned(),
        delivery: DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
    })
    .await
    .expect("发送");
    root.agent.idle().await;

    let bodies = router.bodies(false);
    assert_eq!(bodies.len(), 1, "根会话只发一次请求");
    let system = request_system(&bodies[0]);
    assert!(
        !system.contains("When the user wants a file changed"),
        "没有任务执行者时不追加派任务说明：{system}"
    );
    assert!(
        system.starts_with("You are AIOS's coordinator: you talk with the user, verify facts, and hand work to ThinAgent."),
        "Kind 身份按协调者：{system}"
    );
}

#[tokio::test]
async fn the_tasks_skill_is_bundled_for_kind_only() {
    let fixture = Fixture::new().await;
    fixture.router.push_thin(crate::common::text_sse("查完"));
    let args = serde_json::json!({
        "title": "查一下",
        "requirement": "找答案",
        "dod": "有答案"
    })
    .to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept",
        "task_accept",
        &args,
    ));
    fixture.router.push_kind(crate::common::text_sse("已派出"));
    // ThinAgent 的 waiting relay 可能插进受理这一轮，也可能之后唤醒一轮；预置一条回复。
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-skill", "查一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    wait_until("ThinAgent 请求", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let kind_body = &fixture.router.bodies(false)[0];
    assert!(
        kind_body.contains("aios-tasks"),
        "Kind 的 skill 目录要有 aios-tasks"
    );
    assert!(
        tool_names(kind_body).contains(&"task_stop".to_owned()),
        "Kind 的工具表要有 task_stop"
    );
    let thin_body = &fixture.router.bodies(true)[0];
    assert!(
        !thin_body.contains("aios-tasks"),
        "ThinAgent 看不到 aios-tasks"
    );
}

/// ThinAgent 有约束力记忆：真实派任务路径下，第一次请求带 binding 条目，不带 Kind 的笔记。
#[tokio::test]
async fn thin_agent_request_has_binding_memory_but_not_kind_notes() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    {
        let memory = memory::MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
        memory
            .write(
                memory::types::WriteOp::Remember {
                    title: "回答先给结论".to_owned(),
                    summary: String::new(),
                    body: "先给结论再给理由".to_owned(),
                    source_kind: "user".to_owned(),
                    source_ref: "thin-u1".to_owned(),
                },
                &memory::types::WriteContext::user(),
            )
            .unwrap();
        memory
            .write(
                memory::types::WriteOp::Note {
                    title: "GBK CSV".to_owned(),
                    summary: String::new(),
                    body: "读之前先探编码".to_owned(),
                    source_kind: "kind".to_owned(),
                    source_ref: "thin-k1".to_owned(),
                },
                &memory::types::WriteContext::kind(),
            )
            .unwrap();
    }
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    for script in ready_scripts() {
        router.push_thin(script);
    }
    accept_task(
        &host,
        &router,
        data.path(),
        "kind-thin-memory",
        "call-accept-thin-memory",
        1,
        "查记忆",
    )
    .await;
    wait_until("ThinAgent 第一次请求", || {
        !router.bodies(true).is_empty()
    })
    .await;
    let bodies = router.bodies(true);
    let first = &bodies[0];
    assert!(
        first.contains("有约束力的记忆"),
        "第一次请求要带记忆节：{first}"
    );
    assert!(
        first.contains("先给结论再给理由"),
        "binding 条目要进去：{first}"
    );
    assert!(
        !first.contains("GBK CSV") && !first.contains("读之前先探编码"),
        "Kind 的笔记不注入 ThinAgent：{first}"
    );
    host.shutdown().await;
}

/// 没有 binding 条目时，ThinAgent 请求里不出现空的记忆节。
#[tokio::test]
async fn thin_agent_request_has_no_memory_section_without_binding_items() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    {
        let memory = memory::MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
        memory
            .write(
                memory::types::WriteOp::Note {
                    title: "GBK CSV".to_owned(),
                    summary: String::new(),
                    body: "读之前先探编码".to_owned(),
                    source_kind: "kind".to_owned(),
                    source_ref: "thin-k2".to_owned(),
                },
                &memory::types::WriteContext::kind(),
            )
            .unwrap();
    }
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    for script in ready_scripts() {
        router.push_thin(script);
    }
    accept_task(
        &host,
        &router,
        data.path(),
        "kind-thin-nomemory",
        "call-accept-thin-nomemory",
        1,
        "空记忆",
    )
    .await;
    wait_until("ThinAgent 第一次请求", || {
        !router.bodies(true).is_empty()
    })
    .await;
    let bodies = router.bodies(true);
    let first = &bodies[0];
    assert!(
        !first.contains("有约束力的记忆"),
        "没有 binding 时不出现空记忆节：{first}"
    );
    assert!(
        !first.contains("读之前先探编码"),
        "笔记不注入 ThinAgent：{first}"
    );
    host.shutdown().await;
}

/// 写任务 ready：卡状态固定「待你核」的键（review），阶段仍是待检查；
/// 清单来自所有 Attempt 的写类工具成功 diffs，去重、相对 cwd；固定一行 N/范围。
#[tokio::test]
async fn a_write_task_ready_card_is_review_with_deduped_relative_changes() {
    let fixture = Fixture::new().await;
    let outside = fixture.data().join("outside.py");
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            write_file_sse("call-w1", "a.py", "one\n"),
            write_file_sse("call-w2", "b.py", "b\n"),
            read_file_sse("call-read", "a.py"),
            edit_file_sse("call-edit", "a.py", "one", "two"),
            write_file_sse("call-outside", &outside.to_string_lossy(), "no\n"),
            report_sse(2, "call-report", "ready", "改好了", None),
            crate::common::text_sse("done"),
        ],
    )
    .await;

    let (_, body) = ready_relay(&fixture).await;
    assert_eq!(body["state"], "ready", "{body}");
    assert_eq!(body["task"]["stage"], "ready_for_review", "阶段仍是待检查");
    assert_eq!(body["card_state"], "review", "写任务 ready 的卡状态");
    assert_eq!(body["write_scope"], ".", "{body}");
    assert_eq!(
        body["changed"],
        serde_json::json!(["a.py", "b.py"]),
        "两次写同一文件与一次编辑只算一条；范围外的失败写不进清单：{body}"
    );
    assert_eq!(body["changed_line"], "改了 2 个文件（写入范围 .）");
    assert!(!body.to_string().contains("完成"), "卡上不写完成：{body}");
    assert!(body.get("bash_note").is_none(), "{body}");
    assert!(body.get("worktree_path").is_none(), "{body}");

    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);
    let stored = store.task(&task.id).unwrap().unwrap();
    assert!(stored.acceptances.is_empty(), "还没接受");
    let attempt = stored.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    assert_eq!(
        tool_result(&events, "call-outside").1,
        Some(true),
        "范围外的写确实失败"
    );
}

/// 接受写任务的结果：服务端记下版本和报告，卡变「你已接受」，阶段不变；重复点击幂等。
#[tokio::test]
async fn accepting_a_ready_write_result_records_once_and_keeps_the_stage() {
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

    let (relay_id, body) = ready_relay(&fixture).await;
    assert_eq!(body["card_state"], "review", "{body}");
    assert!(!body.to_string().contains("完成"), "卡上不写完成：{body}");
    let report_id = relay_id
        .strip_prefix("task-report:")
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, report)| report.to_owned())
        .expect("结果卡的 report id");

    let decision = accept_result_card(&fixture.host, &relay_id).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    assert_eq!(decision["action"], "accept_task_result", "{decision}");
    assert_eq!(decision["data"]["summary"], "你接受了", "{decision}");
    assert_eq!(decision["data"]["version"], 1, "{decision}");
    assert_eq!(decision["data"]["report_id"], report_id, "{decision}");
    assert!(decision["data"]["at"].is_number(), "回执带时间：{decision}");
    assert!(!decision.to_string().contains("完成"));

    let store = tasks_store(fixture.data());
    let stored = store.task(&task.id).unwrap().unwrap();
    let acceptance = stored.acceptances.last().cloned().expect("接受记录");
    assert_eq!(acceptance.version, 1);
    assert_eq!(acceptance.report_id.as_str(), report_id);
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::Closed,
        "接受后任务关闭"
    );
    assert_eq!(
        card_decisions(&crate::common::read_root_events(fixture.data()), &relay_id).len(),
        1,
        "决定记进 Kind 的日志"
    );

    let second = accept_result_card(&fixture.host, &relay_id).await;
    assert_eq!(second, decision, "重复点击返回同一条决定");
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        after.acceptances, stored.acceptances,
        "重复点击不改接受记录"
    );
    assert_eq!(
        card_decisions(&crate::common::read_root_events(fixture.data()), &relay_id).len(),
        1,
        "重复点击不再记决定"
    );
}

/// 调查任务没有写集，卡上也有「接受」；接受后任务关闭。
#[tokio::test]
async fn an_investigation_card_can_be_accepted_and_closes_the_task() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查好了：3 行", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let (relay_id, body) = ready_relay(&fixture).await;
    assert_eq!(body["task"]["id"], task.id.as_str());
    let report_id = body["report_id"].as_str().expect("卡上报告 id").to_owned();

    // card-act 带卡上的 report id。
    let response = fixture
        .host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: relay_id.clone(),
            verb: CardVerb::Confirm,
            option: None,
            note: None,
            report_id: Some(report_id.clone()),
        }))
        .await
        .expect("接受");
    let decision = serde_json::to_value(response).unwrap()["decision"].clone();
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    assert_eq!(decision["data"]["summary"], "你接受了", "{decision}");
    assert_eq!(decision["data"]["report_id"], report_id, "{decision}");
    assert!(decision["data"]["at"].is_number(), "{decision}");

    let store = tasks_store(fixture.data());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.acceptances.len(), 1);
    assert_eq!(
        stored.acceptances[0].report_id.as_str(),
        report_id,
        "接受绑卡上的报告"
    );
}

/// 门没判通过前，接受被服务端拒（正常按钮也不会出现）。
#[tokio::test]
async fn accepting_before_the_gate_passes_is_refused() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, _downstream, _gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let relay = relay_messages(&crate::common::read_root_events(data.path()))
        .into_iter()
        .find(|(_, body)| body["task"]["id"] == guarded.id.as_str())
        .map(|(id, _)| id)
        .expect("B 结果卡");
    let decision = accept_result_card(&host, &relay).await;
    assert_eq!(decision["phase"], "refused", "{decision}");
    let summary = decision["data"]["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("核查还没判通过"), "回原因：{summary}");
    assert!(
        tasks_store(data.path())
            .task(&guarded.id)
            .unwrap()
            .unwrap()
            .acceptances
            .is_empty()
    );
}

/// 判通过后接受 → 被把关任务和门任务关闭；下游也接受后总任务关闭。
#[tokio::test]
async fn accepting_after_the_gate_passes_closes_gate_task_and_graph() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (root, guarded, downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    wait_until("B 结果卡 relay", || {
        relay_messages(&crate::common::read_root_events(data.path()))
            .iter()
            .any(|(_, body)| body["task"]["id"] == guarded.id.as_str())
    })
    .await;
    let relay_of = |task_id: &str| -> String {
        let relays = relay_messages(&crate::common::read_root_events(data.path()));
        relays
            .into_iter()
            .find(|(_, body)| body["task"]["id"].as_str() == Some(task_id))
            .map(|(id, _)| id)
            .expect("结果卡 relay")
    };
    let guarded_relay = relay_of(guarded.id.as_str());

    // 判通过后 C 自动开跑：先备好 C 的报告脚本再判。
    router.push_thin_for(
        downstream.id.as_str(),
        report_sse(2, "call-c-report", "ready", "估算：少了 3 条", None),
    );
    router.push_thin_for(downstream.id.as_str(), crate::common::text_sse("好"));
    let judged = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("判通过");
    assert!(matches!(judged, Response::GateJudge(_)));

    let decision = accept_result_card(&host, &guarded_relay).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    let store = tasks_store(data.path());
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::Closed);
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::Closed);

    wait_until("C 结果卡 relay", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
            && relay_messages(&crate::common::read_root_events(data.path()))
                .iter()
                .any(|(_, body)| body["task"]["id"] == downstream.id.as_str())
    })
    .await;
    let downstream_relay = relay_of(downstream.id.as_str());
    let decision = accept_result_card(&host, &downstream_relay).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    assert_eq!(store.stage(&downstream.id).unwrap(), Stage::Closed);
    assert_eq!(store.stage(&root.id).unwrap(), Stage::Closed);
}

/// 已关闭的任务还能再改要求：新版本、同任务新 Attempt、旧接受留在历史。
#[tokio::test]
async fn a_closed_task_can_change_requirement_again_and_keeps_old_acceptance() {
    let fixture = Fixture::new_with_actions().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report-v1", "ready", "查好了 v1", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    let task_id = task.id.as_str().to_owned();
    // 已关闭后从 Kind 说一句再改：按请求内容动态回（触发词 → skill；skill 结果 → propose_action）。
    fixture.router.enable_update_kind(
        &task_id,
        vec![(
            "再改一点：按域名归并".to_owned(),
            "call-update-skill-2".to_owned(),
            "call-update-card-2".to_owned(),
            "再按域名归并一次".to_owned(),
        )],
    );
    let (_relay_id, _) = ready_relay(&fixture).await;
    wait_kind_idle(&fixture.host).await;
    wait_until("v1 Attempt 结束", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.ended_at.is_some())
    })
    .await;
    // 直接用任务库的接受把任务关掉：这条用例要的是「已关闭还能改要求」，
    // 接受回执本身由 的 card-act 用例覆盖；走卡片会多起一轮 Kind，抢回放脚本。
    let store = tasks_store(fixture.data());
    let v1_report = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .expect("第 1 版报告")
        .id
        .clone();
    let old_acceptance = store
        .accept(&task.id, &v1_report, 2_000)
        .expect("接受第 1 版");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-2",
            "再改一点：按域名归并",
        )))
        .await
        .expect("发送");
    let mut card2 = false;
    for _ in 0..900 {
        if crate::common::read_root_events(fixture.data())
            .iter()
            .any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::ToolResult { message, meta, .. }
                        if message.tool_call_id.as_deref() == Some("call-update-card-2")
                            && meta.as_ref().is_some_and(|meta| meta.get("card").is_some())
                )
            })
        {
            card2 = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(card2, "第二张改要求卡");

    fixture.router.push_thin_for(
        task_id.as_str(),
        report_sse(1, "call-report-v2", "ready", "按域名归并好了", None),
    );
    fixture
        .router
        .push_thin_for(task_id.as_str(), crate::common::text_sse("好"));
    fixture
        .host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-update-card-2".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),
            note: None,
            report_id: None,
        }))
        .await
        .expect("确认再改要求");
    wait_until("第 2 次运行开始", || {
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
    assert_eq!(updated.requirement, "再按域名归并一次");
    assert_eq!(updated.acceptances, vec![old_acceptance], "旧接受留历史");
    assert_ne!(store.stage(&task.id).unwrap(), Stage::Closed);
}

/// 从调用驱动 Work——任务库 + Kind 会话派生成一份快照，结果卡在「等你」里带 pick。
#[tokio::test]
async fn work_call_returns_task_facts_and_the_accept_wait_item() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-work-report", "ready", "查好了", None));
    fixture.router.push_thin(crate::common::text_sse("好"));
    let task = fixture.accept(false).await;
    wait_until("任务报回 ready", || {
        tasks_store(fixture.data())
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
                    && task
                        .latest_attempt()
                        .is_some_and(|attempt| attempt.ended_at.is_some())
            })
    })
    .await;

    let response = fixture
        .host
        .dispatch(service::Call::Work(service::WorkRequest {}))
        .await
        .expect("work");
    let service::Response::Work(work) = response else {
        panic!("work 响应");
    };
    assert!(work.generated_at > 0);
    let fact = work
        .tasks
        .iter()
        .find(|fact| fact.id == task.id.as_str())
        .expect("任务事实");
    assert_eq!(fact.title, task.title);
    assert_eq!(fact.stage, "ready_for_review");
    assert_eq!(fact.version, 1);
    assert!(!fact.running);
    let latest = fact.latest.as_ref().expect("最近报告");
    assert_eq!(latest.state, "ready");
    assert_eq!(latest.summary, "查好了");

    let accept = work
        .wait
        .iter()
        .find(|item| item.kind == "accept" && item.task_id.as_deref() == Some(task.id.as_str()))
        .expect("结果等你接受");
    let pick = accept.pick.as_ref().expect("pick");
    assert_eq!(pick.report_id, latest.report_id);
    assert_eq!(pick.version, 1);
    assert!(!accept.title.is_empty());
    assert!(!accept.need.is_empty());
}

/// 从调用驱动组接受——一项旧报告逐项回原话，另一项照记；被接受了才关闭。
#[tokio::test]
async fn task_accept_many_call_accepts_the_fresh_one_and_refuses_the_stale_one() {
    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let (fresh_id, fresh_report) =
        seed_ready_task(&store, fixture.data(), "新结果", 1_800_000_000_000);
    let (stale_id, stale_report) =
        seed_ready_task(&store, fixture.data(), "旧结果", 1_800_000_010_000);
    // 旧任务再开一次 Attempt，交一份新结果：勾选时看到的那份报告已经不是最新。
    let claim = store
        .claim_effect(EffectKind::Launch, 1_800_000_020_000)
        .unwrap();
    if let Some(claim) = claim {
        store.complete_effect(&claim).unwrap();
    }
    let attempt = store.begin_attempt(&stale_id, 1_800_000_021_000).unwrap();
    store
        .record_report(
            &stale_id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "更新的结果".to_owned(),
                question: None,
            },
            1_800_000_022_000,
        )
        .unwrap();

    let request = service::TaskAcceptManyRequest {
        picks: vec![
            service::work::WorkPick {
                task_id: stale_id.as_str().to_owned(),
                report_id: stale_report.as_str().to_owned(),
                version: 1,
            },
            service::work::WorkPick {
                task_id: fresh_id.as_str().to_owned(),
                report_id: fresh_report.as_str().to_owned(),
                version: 1,
            },
        ],
    };
    let response = fixture
        .host
        .dispatch(Call::TaskAcceptMany(request))
        .await
        .expect("task-accept-many");
    let Response::TaskAcceptMany(response) = response else {
        panic!("task-accept-many 响应");
    };
    assert!(response.accepted_at > 0);
    assert_eq!(response.outcomes.len(), 2);
    assert_eq!(response.outcomes[0].task_id, stale_id.as_str());
    assert!(
        response.outcomes[0]
            .refused
            .as_deref()
            .is_some_and(|reason| reason.contains("已经变了")),
        "{:?}",
        response.outcomes[0].refused
    );
    assert_eq!(response.outcomes[1].task_id, fresh_id.as_str());
    assert!(response.outcomes[1].refused.is_none());
    assert_eq!(store.stage(&fresh_id).unwrap(), Stage::Closed);
    assert_eq!(store.stage(&stale_id).unwrap(), Stage::ReadyForReview);
}

#[tokio::test]
async fn task_accept_constraints_must_be_verbatim_user_words() {
    let fixture = Fixture::new().await;
    let task = fixture
        .accept_args(
            false,
            serde_json::json!({
                "title": "带约束的调查",
                "requirement": "找答案",
                "dod": "有答案",
                "constraints": ["改一下"],
            }),
            0,
        )
        .await;
    assert_eq!(task.current_constraints(), ["改一下"]);
    // 任务包第一条 assignment 的 ledger 字段带着这条约束（ThinAgent 能看到）。
    let mut assignment = String::new();
    for _ in 0..250 {
        let latest = tasks_store(fixture.data())
            .task(&task.id)
            .unwrap()
            .expect("任务还在");
        let Some(conversation) = latest
            .latest_attempt()
            .and_then(|attempt| attempt.conversation.clone())
        else {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        };
        let id = session::SessionId::parse(conversation.as_str()).expect("会话 id");
        let Ok(log) = session::SessionStore::open_read_only(fixture.data(), &id) else {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        };
        let text_of = |message: &model::Message| -> Option<String> {
            let text = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect::<Vec<_>>()
                .join(
                    "
",
                );
            (text.trim_start().starts_with('{') && text.contains("ledger")).then_some(text)
        };
        let found = log.events().iter().find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => text_of(message),
            session::EventKind::InboxSpliced { inserted, .. } => inserted.iter().find_map(&text_of),
            _ => None,
        });
        if let Some(text) = found {
            assignment = text;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!assignment.is_empty(), "没等到 assignment");
    let package: Value = serde_json::from_str(&assignment).expect("assignment JSON");
    let ledger = package["ledger"].as_str().expect("assignment 带 ledger");
    assert!(ledger.contains("「改一下」"), "{ledger}");
    assert!(ledger.contains("你说的"), "{ledger}");
}

#[tokio::test]
async fn task_accept_rejects_a_constraint_the_user_never_said() {
    let fixture = Fixture::new().await;
    let args = serde_json::json!({
        "title": "伪造约束",
        "requirement": "找答案",
        "dod": "有答案",
        "constraints": ["用户没说过这句"],
    });
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-accept-bad-constraint",
        "task_accept",
        &args.to_string(),
    ));
    fixture
        .router
        .push_kind(crate::common::text_sse("没派出去"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-1", "改一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    assert!(tasks_store(fixture.data()).task_ids().unwrap().is_empty());
    let events = crate::common::read_root_events(fixture.data());
    let found = events.iter().any(|event| {
        serde_json::to_string(&event.kind)
            .unwrap_or_default()
            .contains("约束要用用户原话")
    });
    assert!(found, "Kind 会话里要留下拒绝原因");
}

#[tokio::test]
async fn task_read_exposes_ledger_constraints_notes_and_history() {
    let fixture = Fixture::new().await;
    let task = fixture
        .accept_args(
            false,
            serde_json::json!({
                "title": "读账本",
                "requirement": "找答案",
                "dod": "有答案",
                "constraints": ["改一下"],
            }),
            0,
        )
        .await;
    let store = tasks_store(fixture.data());
    let attempt = task.latest_attempt().expect("Attempt").id;
    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[tasks::NewLedgerNote {
                kind: tasks::LedgerNoteKind::Rejected,
                topic: "cache".to_owned(),
                text: "缓存不可行".to_owned(),
            }],
            1_700_000_000_000_i64,
        )
        .unwrap();
    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[tasks::NewLedgerNote {
                kind: tasks::LedgerNoteKind::Rejected,
                topic: "cache".to_owned(),
                text: "缓存换成手工".to_owned(),
            }],
            1_700_000_000_001_i64,
        )
        .unwrap();
    let response = fixture
        .host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(response) = response else {
        panic!("task-read 响应");
    };
    assert_eq!(response.ledger.constraints.len(), 1);
    assert_eq!(response.ledger.constraints[0].text, "改一下");
    assert!(response.ledger.constraints[0].from.is_none());
    assert_eq!(response.ledger.notes.len(), 1);
    assert_eq!(response.ledger.notes[0].text, "缓存换成手工");
    assert_eq!(response.ledger.history.len(), 1);
    assert_eq!(response.ledger.history[0].text, "缓存不可行");
    assert_eq!(response.ledger.history[0].closed_why, "被新的一条替换");
}

/// 任务库任意写入都从 Store 唯一出口发 changed，根宿主拿到同一频道。
#[tokio::test]
async fn task_writes_reach_the_root_ws_changed_channel() {
    let fixture = Fixture::new_with_actions().await;
    let runtime = fixture.host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);
    let mut changed = fixture
        .host
        .subscribe_task_changed()
        .expect("根宿主有任务事实变化频道");
    let task = runtime
        .store()
        .create_task(
            NewTask {
                title: "查一下 changed".to_owned(),
                requirement: "有事实就重读".to_owned(),
                dod: "页面不用轮询".to_owned(),
                cwd: fixture.cwd.path().to_path_buf(),
                conversation: ConversationId::new("kind-session"),
                model: ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: service::THIN_TOOLS
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
            },
            1_700_000_000_000,
        )
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(1), changed.recv())
        .await
        .expect("建任务要发一次 changed")
        .expect("changed 频道没关");
    assert_eq!(received, task.id.as_str());
}

#[tokio::test]
async fn kind_first_request_has_project_rules_path_not_the_full_file() {
    let fixture = Fixture::new().await;
    std::fs::create_dir(fixture.cwd.path().join(".git")).unwrap();
    std::fs::write(fixture.cwd.path().join("AGENTS.md"), "KIND-PROJECT-SECRET").unwrap();
    fixture.router.push_kind(crate::common::text_sse("好"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-agents", "你好")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    let bodies = fixture.router.bodies(false);
    assert_eq!(bodies.len(), 1);
    let body = &bodies[0];
    assert!(
        body.contains("Project rules are at"),
        "Kind 固定块只有项目规则路径：{body}"
    );
    assert!(body.contains("AGENTS.md"), "路径里带文件名：{body}");
    assert!(
        !body.contains("KIND-PROJECT-SECRET"),
        "Kind 不拿整份项目规则：{body}"
    );
}
