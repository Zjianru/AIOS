//! 全局暂停与席位：排队、开闸、改席位的准入。

use super::*;

/// 全局暂停：Kind 用确认卡开/关。开着时派的两个任务都不起跑、排队不清；
/// 关掉后按受理顺序开跑，relay 照常送到 Kind。
#[tokio::test]
async fn global_pause_holds_new_tasks_then_runs_the_queue_in_acceptance_order() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());

    // 第一轮：Kind 加载 aios-tasks，出确认卡；事实行是 关 → 开。
    kind_turn(
        &host,
        &router,
        data.path(),
        "kind-pause",
        "暂停新任务",
        1,
        vec![
            skill_sse("call-load-tasks", "aios-tasks"),
            pause_card_sse("call-pause", true),
            crate::common::text_sse("暂停了。"),
        ],
    )
    .await;
    let events = crate::common::read_root_events(data.path());
    assert_eq!(
        card_facts(&events, "call-pause"),
        vec![serde_json::json!({ "name": "全局暂停", "value": "关 → 开" })]
    );
    let decision = confirm_pause_card(&host, "call-pause").await;
    assert_eq!(decision["phase"], "confirmed");
    assert!(
        service::settings::load(data.path()).exec.paused,
        "确认后设置文件里是开"
    );

    // 暂停时派两个任务：都只留 launch 效果，不开会话。
    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        2,
        "甲",
    )
    .await;
    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        3,
        "乙",
    )
    .await;
    assert!(router.bodies(true).is_empty(), "暂停时不能开任务会话");
    for task in [&first, &second] {
        assert!(task.latest_attempt().is_none(), "暂停时不能记 Attempt");
        assert!(
            store
                .effect_pending(EffectKind::Launch, task.id.as_str())
                .unwrap(),
            "排队留着，不取消不清队"
        );
    }
    assert!(
        relay_messages(&crate::common::read_root_events(data.path())).is_empty(),
        "没起跑就没有 relay"
    );

    // 关掉暂停；排队的按受理顺序开跑。两个任务各自的回复、两条 relay 的 Kind 轮都先排好。
    for script in ready_scripts() {
        router.push_thin_for(first.id.as_str(), script);
    }
    for script in ready_scripts() {
        router.push_thin_for(second.id.as_str(), script);
    }
    kind_turn(
        &host,
        &router,
        data.path(),
        "kind-resume",
        "继续开跑",
        4,
        vec![
            skill_sse("call-load-tasks", "aios-tasks"),
            pause_card_sse("call-resume", false),
            crate::common::text_sse("开跑了。"),
            crate::common::text_sse("收到"),
            crate::common::text_sse("收到"),
        ],
    )
    .await;
    let decision = confirm_pause_card(&host, "call-resume").await;
    assert_eq!(decision["phase"], "confirmed");
    assert!(!service::settings::load(data.path()).exec.paused);

    wait_until("两个任务都报 ready", || {
        [&first, &second].iter().all(|task| {
            store
                .task(&task.id)
                .ok()
                .flatten()
                .and_then(|task| task.latest_report().map(|report| report.state))
                == Some(ReportState::Ready)
        })
    })
    .await;
    wait_until("两条 relay 到 Kind", || {
        relay_messages(&crate::common::read_root_events(data.path())).len() == 2
    })
    .await;

    let bodies = router.bodies(true);
    assert!(
        bodies.iter().any(|body| body.contains("甲"))
            && bodies.iter().any(|body| body.contains("乙")),
        "两个任务都开了会话：{bodies:?}"
    );
    // 请求到达的先后由假模型的两个会话各自排程，不能当开跑顺序；开跑顺序看库里
    // 每次 Attempt 的开始时间（效果泵按效果表 seq 逐个领取、逐个开）。
    let started = |task: &Task| {
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .map(|attempt| attempt.started_at)
    };
    let first_started = started(&first).expect("甲有 Attempt");
    let second_started = started(&second).expect("乙有 Attempt");
    assert!(
        first_started <= second_started,
        "按受理顺序开跑：{first_started} / {second_started}"
    );
    let relays = relay_messages(&crate::common::read_root_events(data.path()));
    assert!(
        relays.iter().all(|(_, body)| body["state"] == "ready"),
        "{relays:?}"
    );
}

/// 全局暂停不掐断在跑的一轮：暂停时这一轮照常交出报告，relay 也照常送到 Kind；
/// 暂停期间新派的任务排队。
#[tokio::test]
async fn global_pause_lets_the_running_turn_finish_and_relays_still_arrive() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    // 让 ThinAgent 的回复慢到暂停确认之后，好在「这一轮在跑」时按下暂停。
    router.set_thin_delay(Duration::from_millis(4_000));
    for script in ready_scripts() {
        router.push_thin(script);
    }

    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    wait_until("ThinAgent 的请求在途", || {
        !router.bodies(true).is_empty()
    })
    .await;

    kind_turn(
        &host,
        &router,
        data.path(),
        "kind-pause",
        "暂停新任务",
        2,
        vec![
            skill_sse("call-load-tasks", "aios-tasks"),
            pause_card_sse("call-pause", true),
            crate::common::text_sse("暂停了。"),
            // 这一轮的 relay 会 Steer 进 Kind，备一条回复。
            crate::common::text_sse("收到"),
        ],
    )
    .await;
    let decision = confirm_pause_card(&host, "call-pause").await;
    assert_eq!(decision["phase"], "confirmed");

    // 确认暂停时这一轮还没交出报告：暂停没有取消它。
    let in_flight = store.task(&first.id).unwrap().unwrap();
    assert!(
        in_flight
            .latest_attempt()
            .and_then(|attempt| attempt.conversation.clone())
            .is_some(),
        "任务会话还开着"
    );
    assert_eq!(attempt_end(&store, &first.id), None, "这一轮还没结束");

    wait_until("Kind 收到 relay", || {
        relay_messages(&crate::common::read_root_events(data.path())).len() == 1
    })
    .await;
    assert!(
        service::settings::load(data.path()).exec.paused,
        "relay 到达时暂停还开着"
    );
    assert_eq!(attempt_end(&store, &first.id), Some(AttemptEnd::Reported));
    assert_eq!(
        store
            .task(&first.id)
            .unwrap()
            .unwrap()
            .latest_report()
            .map(|report| report.state),
        Some(ReportState::Ready)
    );

    // 暂停期间再派一个任务：照样排队，不开会话。
    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        4,
        "乙",
    )
    .await;
    assert!(second.latest_attempt().is_none());
    assert!(
        store
            .effect_pending(EffectKind::Launch, second.id.as_str())
            .unwrap(),
        "暂停期间的任务在排队"
    );
    assert!(
        !router.bodies(true).iter().any(|body| body.contains("乙")),
        "暂停期间没有第二个任务会话"
    );
}

/// 设置 › 执行 的暂停入口（SettingsUpdate）真的关闸：开着时派的任务只排队、不开会话；
/// 关掉后按排队顺序开跑。不是只写 settings.json。
#[tokio::test]
async fn settings_update_pauses_launch_admission() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());

    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        paused: Some(true),
        ..Default::default()
    }))
    .await
    .expect("暂停");

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
    assert!(task.latest_attempt().is_none(), "暂停时不开跑");
    assert!(
        store
            .effect_pending(EffectKind::Launch, task.id.as_str())
            .unwrap(),
        "暂停时 launch 留在队列里"
    );
    assert!(router.bodies(true).is_empty(), "暂停时不开任务会话");

    for script in ready_scripts() {
        router.push_thin_for(task.id.as_str(), script);
    }
    router.push_kind(crate::common::text_sse("收到"));
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        paused: Some(false),
        ..Default::default()
    }))
    .await
    .expect("开闸");
    wait_until("开闸后任务开跑", || {
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
}

/// 席位 1：后两个任务在各自前一个报完前只排队，排队不算效果失败；
/// 释放后按受理顺序开跑。席位从 settings.json 重启读回，再进 launch 准入。
#[tokio::test]
async fn seats_one_holds_the_second_task_until_the_first_reports() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut saved = service::settings::load(data.path());
    saved.exec.seats = 1;
    service::settings::save(data.path(), &saved).expect("保存席位");
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    router.set_thin_delay(Duration::from_millis(4_000));
    for title in ["甲", "乙", "丙"] {
        for script in ready_scripts() {
            router.push_thin_for_title(title, script);
        }
    }

    let first = accept_task(
        &host,
        &router,
        data.path(),
        "kind-a",
        "call-accept-a",
        1,
        "甲",
    )
    .await;
    wait_until("第一个任务开跑", || {
        attempt_in_flight(&store, &first.id)
    })
    .await;

    let second = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        2,
        "乙",
    )
    .await;
    assert!(
        second.latest_attempt().is_none(),
        "席位满时第二个任务只排队"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, second.id.as_str())
            .unwrap(),
        "第二个任务的 launch 还在队列里"
    );
    assert_eq!(
        store.effect_health(EffectKind::Launch).unwrap().attempts,
        0,
        "满席位返回 Later，不算效果失败"
    );
    assert!(
        !router.bodies(true).iter().any(|body| body.contains("乙")),
        "第二个任务还没开会话"
    );

    let third = accept_task(
        &host,
        &router,
        data.path(),
        "kind-c",
        "call-accept-c",
        3,
        "丙",
    )
    .await;
    assert!(
        third.latest_attempt().is_none(),
        "席位 1 时第三个任务也排队"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, third.id.as_str())
            .unwrap(),
        "第三个任务的 launch 还在队列里"
    );
    assert!(
        !router.bodies(true).iter().any(|body| body.contains("丙")),
        "第三个任务还没开会话"
    );

    for _ in 0..3 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    // 第一个任务的回复已经在途（延迟在请求时定住）；后面的任务不必再慢。
    router.clear_thin_delay();
    wait_until("第一个任务报 ready", || {
        reported_ready(&store, &first.id)
    })
    .await;
    let first_report_at = store
        .task(&first.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .map(|report| report.at)
        .expect("第一个任务的报告");
    wait_until("第二个任务开跑", || {
        store
            .task(&second.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
    let second_started_at = store
        .task(&second.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .map(|attempt| attempt.started_at)
        .expect("第二个任务开跑");
    assert!(
        second_started_at >= first_report_at,
        "第二个任务在第一个报完之后才开：{second_started_at} / {first_report_at}"
    );
    wait_until("第二个任务报 ready", || {
        reported_ready(&store, &second.id)
    })
    .await;
    let second_report_at = store
        .task(&second.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .map(|report| report.at)
        .expect("第二个任务的报告");
    wait_until("第三个任务开跑", || {
        store
            .task(&third.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
    let third_started_at = store
        .task(&third.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .map(|attempt| attempt.started_at)
        .expect("第三个任务开跑");
    assert!(
        third_started_at >= second_report_at,
        "释放后按受理顺序开跑：{third_started_at} / {second_report_at}"
    );
    wait_until("三个任务都报 ready", || {
        reported_ready(&store, &first.id)
            && reported_ready(&store, &second.id)
            && reported_ready(&store, &third.id)
    })
    .await;
    wait_until("三条 relay 到 Kind", || {
        relay_messages(&crate::common::read_root_events(data.path())).len() == 3
    })
    .await;
    assert_eq!(attempt_end(&store, &second.id), Some(AttemptEnd::Reported));
    assert_eq!(attempt_end(&store, &third.id), Some(AttemptEnd::Reported));
}

/// 席位只收 1–16：越界拒存，已有的值不动。
#[tokio::test]
async fn set_exec_seats_accepts_one_through_sixteen_and_rejects_outside() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;

    host.set_exec_seats(1).expect("保存 1 个席位");
    assert_eq!(service::settings::load(data.path()).exec.seats, 1);
    host.set_exec_seats(16).expect("保存 16 个席位");
    assert_eq!(service::settings::load(data.path()).exec.seats, 16);

    for seats in [0_u8, 17] {
        let error = host.set_exec_seats(seats).expect_err("越界拒存");
        assert!(error.to_string().contains("1–16"), "{error}");
        assert_eq!(
            service::settings::load(data.path()).exec.seats,
            16,
            "拒存不改已保存的值"
        );
    }
}

/// 设置写不进去时只报错、不改内存闸门：暂停没生效，任务照常开跑。
#[tokio::test]
async fn a_failed_settings_write_keeps_the_pause_gate_open() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 设置文件本身可读；临时文件路径被目录占住，原子写一定失败（不靠权限位）。
    let saved = service::settings::load(data.path());
    service::settings::save(data.path(), &saved).expect("先写一份可读的设置");
    std::fs::create_dir(data.path().join("settings.json.tmp")).expect("占住临时文件路径");
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    router.push_thin_for_title("甲", crate::common::text_sse("还没上报"));

    let error = host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            paused: Some(true),
            ..Default::default()
        }))
        .await
        .expect_err("写盘失败要报错");
    assert!(error.to_string().contains("设置没保存"), "{error}");
    assert!(
        !service::settings::load(data.path()).exec.paused,
        "文件没被改"
    );

    // 内存里也没改成暂停：写盘失败后派的第一个任务照常开跑。
    let task = dispatch_task(&host, &router, data.path(), "kind-a", "call-accept-a", "甲").await;
    wait_until("写盘失败后任务照常开跑", || {
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
}

/// 席位写不进去时只报错、不改内存上限：还是 1 个席位，第二个任务排队。
#[tokio::test]
async fn a_failed_settings_write_keeps_the_seat_limit() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut saved = service::settings::load(data.path());
    saved.exec.seats = 1;
    service::settings::save(data.path(), &saved).expect("先保存 1 个席位");
    std::fs::create_dir(data.path().join("settings.json.tmp")).expect("占住临时文件路径");
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    router.set_thin_delay(Duration::from_millis(3_000));
    router.push_thin_for_title("甲", crate::common::text_sse("还没上报"));

    let error = host.set_exec_seats(2).expect_err("写盘失败要报错");
    assert!(error.to_string().contains("设置没保存"), "{error}");
    assert_eq!(
        service::settings::load(data.path()).exec.seats,
        1,
        "文件没被改"
    );

    let first = dispatch_task(&host, &router, data.path(), "kind-a", "call-accept-a", "甲").await;
    wait_until("第一个任务开跑", || {
        store
            .task(&first.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
    let second = dispatch_task(&host, &router, data.path(), "kind-b", "call-accept-b", "乙").await;
    assert!(
        second.latest_attempt().is_none(),
        "内存上限还是 1 个席位，第二个任务只排队"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, second.id.as_str())
            .unwrap(),
        "第二个任务的 launch 留在队列里"
    );
}

/// ：全局暂停时确认任务图——先都排着，开闸后按先后跑（A 先，B 等 A 的 ready）。
#[tokio::test]
async fn graph_confirm_while_paused_queues_then_runs_in_order_after_unpause() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        paused: Some(true),
        ..Default::default()
    }))
    .await
    .expect("暂停");

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
    let Response::CardAct(response) = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),

            note: None,
            report_id: None,
        }))
        .await
        .expect("确认")
    else {
        panic!("card-act 响应");
    };
    assert!(response.decision.is_some());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let store = tasks_store(data.path());
    assert!(
        store
            .task(&children[0].id)
            .expect("读任务")
            .expect("A")
            .latest_attempt()
            .is_none(),
        "暂停时确认也不开跑"
    );
    assert!(router.bodies(true).is_empty(), "暂停时不开任务会话");
    assert!(
        store
            .effect_pending(EffectKind::Launch, children[0].id.as_str())
            .expect("效果")
            || store
                .task(&children[0].id)
                .expect("读任务")
                .expect("A")
                .stage(false)
                == Stage::Pending,
        "A 的 launch 还排着"
    );

    // 开闸前备好两个子任务的 ThinAgent 脚本与 Kind 后续文本。
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
    for _ in 0..6 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    runtime.set_pump_paused(false);
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        paused: Some(false),
        ..Default::default()
    }))
    .await
    .expect("开闸");

    wait_until("A 开跑", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().map(|attempt| attempt.started_at))
            .is_some()
    })
    .await;
    wait_until("B 自动开跑", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().map(|attempt| attempt.started_at))
            .is_some()
    })
    .await;
    let store = tasks_store(data.path());
    let a_started = store
        .task(&children[0].id)
        .expect("读任务")
        .expect("A")
        .latest_attempt()
        .expect("A Attempt")
        .started_at;
    let b_started = store
        .task(&children[1].id)
        .expect("读任务")
        .expect("B")
        .latest_attempt()
        .expect("B Attempt")
        .started_at;
    assert!(a_started <= b_started, "开闸后按先后跑：A 先于 B");
    let total = store.task(&root.id).expect("读任务").expect("总任务");
    assert!(total.latest_attempt().is_none(), "总任务始终不跑");
}
