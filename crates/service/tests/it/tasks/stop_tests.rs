//! 停下/打断这一轮：立即停、收尾停、重复点、与候选/席位相关。

use super::*;

#[tokio::test]
async fn a_relay_interrupted_before_kind_append_is_delivered_once_after_restart() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;
    fixture.router.push_thin(report_sse(
        1,
        "call-report",
        "ready",
        "重启后只送一次的答案",
        None,
    ));
    fixture.router.push_thin(crate::common::text_sse("done"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .crash_before_next_relay();
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("report ready", || {
        let store = tasks_store(fixture.data());
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    let store = tasks_store(fixture.data());
    let report = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .unwrap()
        .id
        .clone();
    wait_until("relay left queued", || {
        store
            .effect_pending(tasks::EffectKind::Relay, report.as_str())
            .unwrap()
    })
    .await;
    assert!(waiting_relays(fixture.data()).is_empty(), "崩溃前不能送到");

    let Fixture {
        _server: server,
        data,
        cwd,
        router,
        host,
    } = fixture;
    drop(host);
    router.push_kind(crate::common::text_sse("收到"));
    let host = open_host(&server, data.path(), cwd.path()).await;
    wait_until("restart relay", || waiting_relays(data.path()).len() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(waiting_relays(data.path()).len(), 1, "重启后仍然只有一条");
    assert_eq!(
        waiting_relays(data.path())[0].1["summary"],
        "重启后只送一次的答案"
    );
    let store = tasks_store(data.path());
    assert!(
        !store
            .effect_pending(tasks::EffectKind::Relay, report.as_str())
            .unwrap()
    );
    drop(host);
}

#[tokio::test]
async fn stopping_a_running_task_persists_the_stop_before_cancelling_and_relays_once() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    // 钩子观察：ThinAgent 的取消收尾写进日志时，任务库里已经能读到停止事实。
    let seen = Arc::new(Mutex::new(Vec::new()));
    let task_slot: Arc<Mutex<Option<tasks::TaskId>>> = Arc::new(Mutex::new(None));
    let hook_data = data.path().to_path_buf();
    let hook_seen = Arc::clone(&seen);
    let hook_task = Arc::clone(&task_slot);
    let hook: session::AppendHookFactory = Arc::new(move |_id| {
        let data = hook_data.clone();
        let seen = Arc::clone(&hook_seen);
        let task_slot = Arc::clone(&hook_task);
        Arc::new(move |events: &[session::Event]| {
            let aborted = events.iter().any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::TurnEnd {
                        reason: session::TurnEndReason::Aborted { .. },
                        ..
                    }
                )
            });
            if !aborted {
                return;
            }
            let Some(task_id) = task_slot.lock().unwrap().clone() else {
                return;
            };
            let Ok(store) = Store::open(data.join(service::TASKS_DB_FILE)) else {
                return;
            };
            let Ok(Some(task)) = store.task(&task_id) else {
                return;
            };
            let Some(attempt) = task.latest_attempt() else {
                return;
            };
            let key = format!("stop:{task_id}:{}", attempt.id);
            let pending = store
                .effect_pending(EffectKind::Relay, &key)
                .unwrap_or(false);
            seen.lock().unwrap().push((attempt.end, pending));
        })
    });

    let host = open_host_with(&server, data.path(), cwd.path(), |options| {
        options.append_hook_factory = Some(hook);
    })
    .await;
    // 第一轮响应慢一点：停止要在 ThinAgent 真正跑着的时候发生。
    router.set_thin_delay(Duration::from_millis(2_000));
    router.push_thin(crate::common::text_sse("我还在查"));

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
    host.dispatch(Call::Send(send(&host, "kind-stop-1", "查一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;

    let store = tasks_store(data.path());
    let task = one_task(&store);
    *task_slot.lock().unwrap() = Some(task.id.clone());
    wait_until("ThinAgent 的请求在途", || {
        !router.bodies(true).is_empty()
    })
    .await;

    // 页头事实：这次 Attempt 正在跑、还没停；这决定「任务」菜单能不能点。
    let read = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(read) = read else {
        panic!("TaskRead 的响应形状不对");
    };
    assert!(
        read.task.running,
        "在跑的 Attempt 页头是执行中：{:?}",
        read.task
    );
    assert!(!read.task.stopped, "还没停下：{:?}", read.task);

    // relay 先不投递：停止后能查效果行还在排队。
    host.task_runtime().expect("runtime").set_pump_paused(true);
    let response = host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停止");
    let Response::TaskStop(stop) = response else {
        panic!("TaskStop 的响应形状不对");
    };
    assert!(stop.stopped, "{stop:?}");
    assert_eq!(stop.stage, "needs_conditions");

    // 停下落库后页头固定：「已停止」、不再在跑；刷新也读得到。
    let read = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(read) = read else {
        panic!("TaskRead 的响应形状不对");
    };
    assert!(!read.task.running, "停了就不再跑：{:?}", read.task);
    assert!(read.task.stopped, "页头固定已停止：{:?}", read.task);

    let task = store.task(&task.id).unwrap().unwrap();
    let attempt = task.latest_attempt().expect("Attempt");
    assert_eq!(attempt.end, Some(AttemptEnd::Stopped));
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::NeedsConditions,
        "停止后阶段是待补足条件"
    );
    assert!(
        store
            .effect_pending(
                EffectKind::Relay,
                &format!("stop:{}:{}", task.id, attempt.id)
            )
            .unwrap(),
        "停止 relay 还在排队"
    );

    // 取消后的收尾：ThinAgent 最后一轮是 aborted。
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("ThinAgent 的轮收尾", || {
        session::SessionStore::open_read_only(data.path(), &session_id).is_ok_and(|store| {
            store
                .events()
                .iter()
                .any(|event| matches!(&event.kind, session::EventKind::TurnEnd { .. }))
        })
    })
    .await;
    let thin = session::SessionStore::open_read_only(data.path(), &session_id).unwrap();
    assert_eq!(
        last_turn_end(thin.events()),
        Some(session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        })
    );

    // 顺序：取消的收尾写日志时，库里停止事实已提交、relay 已排队。
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "只应观察到一次取消收尾：{seen:?}");
        assert_eq!(
            seen[0],
            (Some(AttemptEnd::Stopped), true),
            "先持久后 cancel：收尾写日志时停止已在库里"
        );
    }
    assert!(
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_report()
            .is_none(),
        "停止不合成 waiting 报告"
    );

    host.task_runtime().expect("runtime").set_pump_paused(false);
    router.push_kind(crate::common::text_sse("收到"));
    wait_until("Kind 收到停止 relay", || {
        stopped_relays(&crate::common::read_root_events(data.path())).len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let relays = stopped_relays(&crate::common::read_root_events(data.path()));
    assert_eq!(relays.len(), 1, "Kind 只收到一条停止 relay：{relays:?}");
    let body = &relays[0].1;
    assert_eq!(body["kind"], "task_report");
    assert_eq!(body["state"], "stopped");
    assert_eq!(body["origin"], "executor");
    assert_eq!(body["task"]["id"], task.id.as_str());
    assert_eq!(body["task"]["stage"], "needs_conditions");
    assert!(
        body["summary"]
            .as_str()
            .is_some_and(|text| text.contains("立即停")),
        "{body:?}"
    );
}

#[tokio::test]
async fn kind_task_stop_now_stops_the_running_attempt_like_the_page() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(2_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查"));
    let task = fixture.accept(false).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let args = serde_json::json!({"task_id": task.id.as_str(), "mode": "now"}).to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        1,
        "call-stop",
        "task_stop",
        &args,
    ));
    fixture.router.push_kind(crate::common::text_sse("停了"));
    // 停止 relay 可能在这轮之后才到，预置它唤醒的那一轮的回复。
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-stop-2", "把任务停了")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;

    let events = crate::common::read_root_events(fixture.data());
    let (text, is_error) = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. }
                if message.tool_call_id.as_deref() == Some("call-stop") =>
            {
                Some((
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                    message.is_error,
                ))
            }
            _ => None,
        })
        .expect("task_stop 的 tool/result");
    assert!(!is_error.unwrap_or(false), "{text}");
    assert!(text.contains("已立即停"), "{text}");

    let store = tasks_store(fixture.data());
    let task = store.task(&task.id).unwrap().unwrap();
    let attempt = task.latest_attempt().expect("Attempt");
    assert_eq!(attempt.end, Some(AttemptEnd::Stopped));
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("ThinAgent 的轮收尾", || {
        session::SessionStore::open_read_only(fixture.data(), &session_id).is_ok_and(|store| {
            store
                .events()
                .iter()
                .any(|event| matches!(&event.kind, session::EventKind::TurnEnd { .. }))
        })
    })
    .await;
    let thin = session::SessionStore::open_read_only(fixture.data(), &session_id).unwrap();
    assert_eq!(
        last_turn_end(thin.events()),
        Some(session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        })
    );

    wait_until("Kind 收到停止 relay", || {
        stopped_relays(&crate::common::read_root_events(fixture.data())).len() == 1
    })
    .await;
    assert_eq!(
        stopped_relays(&crate::common::read_root_events(fixture.data()))[0].1["state"],
        "stopped"
    );
}

#[tokio::test]
async fn task_stop_for_a_task_that_is_not_running_reports_the_fixed_reason() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;

    let response = fixture
        .host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停止");
    let Response::TaskStop(stop) = response else {
        panic!("TaskStop 的响应形状不对");
    };
    assert!(!stop.stopped, "{stop:?}");
    assert_eq!(stop.message, "这个任务没在跑");
    assert_eq!(stop.stage, "pending");

    let store = tasks_store(fixture.data());
    let task = store.task(&task.id).unwrap().unwrap();
    assert!(
        task.latest_attempt().is_none(),
        "没开跑的任务不该有 Attempt"
    );
}

#[tokio::test]
async fn kind_task_stop_reports_not_running_for_both_modes() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;

    let now_args = serde_json::json!({"task_id": task.id.as_str(), "mode": "now"}).to_string();
    let after_args =
        serde_json::json!({"task_id": task.id.as_str(), "mode": "after_report"}).to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        1,
        "call-stop-now",
        "task_stop",
        &now_args,
    ));
    fixture.router.push_kind(crate::common::tool_use_sse(
        2,
        "call-stop-after",
        "task_stop",
        &after_args,
    ));
    fixture.router.push_kind(crate::common::text_sse("没停成"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-stop-3", "停一下")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;

    let events = crate::common::read_root_events(fixture.data());
    let result = |call: &str| {
        events
            .iter()
            .find_map(|event| match &event.kind {
                session::EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some(call) =>
                {
                    Some((
                        message
                            .content
                            .iter()
                            .filter_map(model::ContentBlock::as_text)
                            .collect::<String>(),
                        message.is_error,
                    ))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{call} 的 tool/result"))
    };
    let (not_running, is_error) = result("call-stop-now");
    assert!(!is_error.unwrap_or(false), "{not_running}");
    assert_eq!(not_running, "这个任务没在跑");

    // 两种停法对没在跑的任务回同一句固定原因，都不是工具错误。
    let (after_report, is_error) = result("call-stop-after");
    assert!(!is_error.unwrap_or(false), "{after_report}");
    assert_eq!(after_report, "这个任务没在跑");
}

/// 停下这一轮：会话这一轮 aborted、Attempt 没结束、不出 relay/waiting；
/// 再说一句还是同一个 Attempt，接着原会话跑。
#[tokio::test]
async fn interrupting_a_running_turn_keeps_the_attempt_and_the_next_sentence_continues_it() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(3_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;
    wait_until("ThinAgent 请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;
    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("第一轮 Attempt");

    let interrupt = interrupt_task(&fixture.host, &task).await;
    assert!(interrupt.interrupted, "{interrupt:?}");
    assert_eq!(interrupt.message, "已停下这一轮");

    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("这一轮以用户取消收尾", || {
        session::SessionStore::open_read_only(fixture.data(), &session_id).is_ok_and(|store| {
            store.events().iter().any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::TurnEnd {
                        reason: session::TurnEndReason::Aborted {
                            reason: session::AbortCause::User,
                        },
                        ..
                    }
                )
            })
        })
    })
    .await;
    let thin = session::SessionStore::open_read_only(fixture.data(), &session_id).unwrap();
    assert_eq!(
        last_turn_end(thin.events()),
        Some(session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        })
    );

    // 只停这一轮：Attempt 不结束、不合成 waiting、不排 relay。
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = store.task(&task.id).unwrap().unwrap();
    let kept = after.latest_attempt().cloned().expect("Attempt 还在");
    assert_eq!(kept.id, attempt.id);
    assert!(kept.ended_at.is_none(), "停下这一轮不写 Attempt 结束");
    assert!(
        after.latest_report().is_none(),
        "停下这一轮不合成 waiting 报告"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);
    let root_events = crate::common::read_root_events(fixture.data());
    assert!(
        relay_messages(&root_events).is_empty(),
        "停下这一轮不出结果 relay"
    );
    assert!(
        stopped_relays(&root_events).is_empty(),
        "停下这一轮不是立即停，没有停止 relay"
    );
    let read = fixture
        .host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: task.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(read) = read else {
        panic!("TaskRead 的响应形状不对");
    };
    assert!(!read.task.running, "这一轮已经停下");
    assert!(!read.task.stopped, "任务没停，只是停下这一轮");

    // 再说一句：同一 Attempt 接着做，请求里带着下一句。
    fixture.router.clear_thin_delay();
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture.router.push_thin_for(
        task.id.as_str(),
        report_sse(1, "call-report", "ready", "接着做完了", None),
    );
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收尾"));
    let said = say_in_task_dialog(&fixture.host, &task, "再接着说").await;
    assert_eq!(said.new_attempt, None, "同一个 Attempt 接着做，不开新的");
    wait_until("下一句进了任务请求", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("再接着说"))
    })
    .await;
    let resumed = store.task(&task.id).unwrap().unwrap();
    assert_eq!(attempt_count(&resumed), 1, "Attempt 数不变");
    let resumed_attempt = resumed.latest_attempt().cloned().expect("Attempt");
    assert_eq!(resumed_attempt.id, attempt.id);
    assert_eq!(
        resumed_attempt.conversation.as_ref(),
        Some(&conversation),
        "还是原会话"
    );
    wait_until("接着做的这一轮报 ready", || {
        reported_ready(&store, &task.id)
    })
    .await;
    wait_until("Kind 收到结果 relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data())).len() == 1
    })
    .await;
    let final_task = store.task(&task.id).unwrap().unwrap();
    let states: Vec<_> = final_task
        .reports
        .iter()
        .map(|report| report.state)
        .collect();
    assert_eq!(
        states,
        vec![ReportState::Ready],
        "停下这一轮没有留下 waiting 报告"
    );
}

/// 停下这一轮不丢已经排队的输入：取消前 Steer 进去的那句，下一句来时一起被领取。
#[tokio::test]
async fn interrupting_keeps_the_queued_sentence_for_the_next_turn() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(500));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;
    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("第一轮 Attempt");

    // 在跑时说一句：delivery 把它 Steer 进当前这一轮的输入队列。
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收到排队的那句"));
    let said = say_in_task_dialog(&fixture.host, &task, "排队的一句").await;
    assert_eq!(said.new_attempt, None);
    wait_until("这句话已经送进会话输入队列", || {
        !store
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap()
    })
    .await;

    let interrupt = interrupt_task(&fixture.host, &task).await;
    assert!(interrupt.interrupted, "{interrupt:?}");
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("这一轮以用户取消收尾", || {
        session::SessionStore::open_read_only(fixture.data(), &session_id).is_ok_and(|store| {
            store.events().iter().any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::TurnEnd {
                        reason: session::TurnEndReason::Aborted {
                            reason: session::AbortCause::User,
                        },
                        ..
                    }
                )
            })
        })
    })
    .await;

    // 取消不会把排队的那句冲掉：下一句进来时，两句话一起进请求；Attempt 还是同一个。
    fixture.router.clear_thin_delay();
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收到两句"));
    let said = say_in_task_dialog(&fixture.host, &task, "再接着说").await;
    assert_eq!(said.new_attempt, None, "还是同一个 Attempt");
    wait_until("下一句进来时排队的那句也一起送到", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("排队的一句") && body.contains("再接着说"))
    })
    .await;
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(attempt_count(&after), 1, "不丢输入也不开新 Attempt");
    assert_eq!(
        after.latest_attempt().map(|attempt| attempt.id),
        Some(attempt.id)
    );
}

/// 没在跑时停下这一轮：返回固定原因，不动任务。
#[tokio::test]
async fn interrupting_a_task_that_is_not_running_reports_the_fixed_reason() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;

    let interrupt = interrupt_task(&fixture.host, &task).await;
    assert!(!interrupt.interrupted, "{interrupt:?}");
    assert_eq!(interrupt.message, "这个任务没在跑");

    let store = tasks_store(fixture.data());
    let task = store.task(&task.id).unwrap().unwrap();
    assert!(
        task.latest_attempt().is_none(),
        "没开跑的任务不该多出 Attempt"
    );
    assert!(task.latest_report().is_none(), "没在跑不写报告");
}

/// 立即停仍走 ：停下这一轮之后点立即停，Attempt 照样结束并排停止 relay。
#[tokio::test]
async fn stopping_now_after_an_interrupt_still_ends_the_attempt_and_relays() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(1_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查"));
    let task = fixture.accept(false).await;
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;

    let interrupt = interrupt_task(&fixture.host, &task).await;
    assert!(interrupt.interrupted, "{interrupt:?}");
    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("Attempt");
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).unwrap();
    wait_until("这一轮以用户取消收尾", || {
        session::SessionStore::open_read_only(fixture.data(), &session_id).is_ok_and(|store| {
            store.events().iter().any(|event| {
                matches!(
                    &event.kind,
                    session::EventKind::TurnEnd {
                        reason: session::TurnEndReason::Aborted {
                            reason: session::AbortCause::User,
                        },
                        ..
                    }
                )
            })
        })
    })
    .await;

    fixture.router.push_kind(crate::common::text_sse("收到"));
    let stop = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stop.stopped, "{stop:?}");
    assert_eq!(
        attempt_end(&store, &task.id),
        Some(AttemptEnd::Stopped),
        "立即停照样结束 Attempt"
    );
    wait_until("Kind 收到停止 relay", || {
        stopped_relays(&crate::common::read_root_events(fixture.data())).len() == 1
    })
    .await;
}

#[tokio::test]
async fn task_stop_after_report_for_a_task_that_is_not_running_reports_the_fixed_reason() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;

    let stop = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(!stop.stopped, "{stop:?}");
    assert_eq!(stop.message, "这个任务没在跑");
    assert_eq!(stop.stage, "pending");
}

#[tokio::test]
async fn after_report_stop_injects_the_note_denies_other_tools_and_ends_reported() {
    let fixture = Fixture::new().await;
    // 收尾停要落在 ThinAgent 跑着的时候：把第一次响应拖慢，留出插进去的时机。
    fixture.router.set_thin_delay(Duration::from_millis(2_500));
    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-read",
        "read",
        &serde_json::json!({"file_path": "not-there.txt"}).to_string(),
    ));
    fixture.router.push_thin(report_sse(
        2,
        "call-report",
        "ready",
        "查完了，答案在第二行",
        None,
    ));
    fixture.router.push_thin(crate::common::text_sse("报完了"));
    // ready relay 会 Steer 进 Kind：备用回复在受理派发前就排好。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let stop = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(stop.stopped, "{stop:?}");
    assert!(stop.message.contains("已收尾停"), "{stop:?}");

    // 插进去的固定说明在下一步请求里。
    wait_until("ThinAgent 的下一步请求", || {
        fixture.router.bodies(true).len() >= 2
    })
    .await;
    let thin_bodies = fixture.router.bodies(true);
    assert!(
        thin_bodies[1].contains(
            "用户要求收尾停：做完手上这一步，用 task_report 报一次当前结果，然后结束。不再调其他工具。"
        ),
        "下一个请求要带固定停止说明：{}",
        thin_bodies[1]
    );

    let store = tasks_store(fixture.data());
    wait_until("ready 报告", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    assert_eq!(
        attempt.end,
        Some(AttemptEnd::Reported),
        "报了以后 Attempt 以 reported 结束"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);

    // 会话日志：固定说明一条；read 先记 tool/call 再给错误 result；task_report 放行。
    let events = thin_events(fixture.data(), &attempt);
    assert_eq!(
        injected_stop_notes(&events, &task, &attempt),
        1,
        "固定说明按 request id 只插一条"
    );
    let (read_text, read_error) = tool_result(&events, "call-read");
    assert_eq!(read_error, Some(true), "{read_text}");
    assert_eq!(read_text, "Error: 已收尾停，只能用 task_report 上报");
    let call_position = events
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                session::EventKind::ToolCall { call_id, .. } if call_id == "call-read"
            )
        })
        .expect("read 的 tool/call");
    let result_position = events
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                session::EventKind::ToolResult { message, .. }
                    if message.tool_call_id.as_deref() == Some("call-read")
            )
        })
        .expect("read 的 tool/result");
    assert!(
        call_position < result_position,
        "先记 tool/call，再给错误的 tool/result"
    );
    let (report_text, report_error) = tool_result(&events, "call-report");
    assert_eq!(report_error, None, "task_report 放行：{report_text}");
    assert!(report_text.contains("已上报"), "{report_text}");

    // 卡照报的状态出：Kind 收到一条 ready relay，阶段是待检查。
    wait_until("Kind 收到 ready relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "ready");
    assert_eq!(relays[0].1["task"]["stage"], "ready_for_review");
}

#[tokio::test]
async fn after_report_stop_without_a_report_synthesizes_waiting_then_records_stopped() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(2_500));
    fixture
        .router
        .push_thin(crate::common::text_sse("我还在查，别急"));
    // 插进去的说明会开下一步；这一步 ThinAgent 没报，turn 才收尾。
    fixture.router.push_thin(crate::common::text_sse("不报了"));
    // waiting relay 会 Steer 进 Kind：备用回复在受理派发前就排好。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let stop = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(stop.stopped, "{stop:?}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    let latest = task_row.latest_report().expect("waiting 报告");
    assert_eq!(latest.state, ReportState::Waiting, "没报就走 waiting 合成");
    assert_eq!(latest.summary, "不报了");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);

    wait_until("Kind 收到 waiting relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "waiting");
    assert_eq!(relays[0].1["task"]["stage"], "needs_conditions");
}

#[tokio::test]
async fn after_report_stop_on_an_idle_task_records_stopped_without_another_relay() {
    let fixture = Fixture::new().await;
    fixture.router.push_thin(crate::common::text_sse("还在查"));
    // waiting relay 会 Steer 进 Kind：备用回复在受理派发前就排好，免得撞上没脚本的请求。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("waiting relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;

    // 这一轮已经结束：收尾停落在空闲的会话上，没有下一步可等。
    let stop = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(stop.stopped, "{stop:?}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        task_row.latest_report().map(|report| report.state),
        Some(ReportState::Waiting),
        "已有的 waiting 报告就是收尾停时的事实"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        relay_messages(&crate::common::read_root_events(fixture.data())).len(),
        1,
        "没多出一条 relay"
    );
    assert!(
        fixture.router.state.unexpected.lock().unwrap().is_empty(),
        "没有请求落在脚本之外"
    );
}

#[tokio::test]
async fn stopping_after_report_twice_injects_the_fixed_note_once() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(6_000));
    fixture.router.push_thin(crate::common::text_sse("还在查"));
    // 插进去的说明会开下一步；这一步 ThinAgent 没报，turn 才收尾。
    fixture.router.push_thin(crate::common::text_sse("不报了"));
    // waiting relay 会 Steer 进 Kind：备用回复在受理派发前就排好。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let first = stop_task(&fixture, &task, StopMode::AfterReport).await;
    let second = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(first.stopped, "{first:?}");
    assert!(second.stopped, "{second:?}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    assert_eq!(
        injected_stop_notes(&events, &task, &attempt),
        1,
        "重复点收尾停用同一个 request id：只插一条"
    );
}

#[tokio::test]
async fn stopping_now_after_an_after_report_stop_uses_the_immediate_stop() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(2_000));
    fixture.router.push_thin(crate::common::text_sse("还在查"));
    // 立即停的 relay 会 Steer 进 Kind：备用回复在受理派发前就排好。
    let task = fixture.accept_with_kind_replies(false, 1).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let after_report = stop_task(&fixture, &task, StopMode::AfterReport).await;
    assert!(after_report.stopped, "{after_report:?}");
    let now = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(now.stopped, "{now:?}");
    assert!(now.message.contains("已立即停"), "{now:?}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    wait_until("Kind 收到停止 relay", || {
        stopped_relays(&crate::common::read_root_events(fixture.data())).len() == 1
    })
    .await;
    let relays = stopped_relays(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays[0].1["state"], "stopped");

    // 立即停照 ：会话最后一轮以取消收尾，不再合成 waiting 报告。
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    // 取消的收尾由会话驱动异步写日志：等它落盘再读，别抢在停止 relay 前面。
    wait_until("会话这一轮以取消收尾", || {
        last_turn_end(&thin_events(fixture.data(), &attempt))
            == Some(session::TurnEndReason::Aborted {
                reason: session::AbortCause::User,
            })
    })
    .await;
    let events = thin_events(fixture.data(), &attempt);
    assert_eq!(
        last_turn_end(&events),
        Some(session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        })
    );
    assert_eq!(
        task_row.latest_report().map(|report| report.state),
        None,
        "立即停不合成 waiting 报告"
    );
}

#[tokio::test]
async fn kind_task_stop_after_report_stops_after_the_next_turn() {
    let fixture = Fixture::new().await;
    fixture.router.set_thin_delay(Duration::from_millis(2_500));
    fixture.router.push_thin(crate::common::text_sse("还在查"));
    // 插进去的说明会开下一步；这一步 ThinAgent 没报，turn 才收尾。
    fixture.router.push_thin(crate::common::text_sse("不报了"));
    let task = fixture.accept(false).await;
    wait_until("ThinAgent 的请求在途", || {
        !fixture.router.bodies(true).is_empty()
    })
    .await;

    let args = serde_json::json!({"task_id": task.id.as_str(), "mode": "after_report"}).to_string();
    fixture.router.push_kind(crate::common::tool_use_sse(
        1,
        "call-stop-after",
        "task_stop",
        &args,
    ));
    fixture.router.push_kind(crate::common::text_sse("好"));
    // waiting relay 唤醒的轮、以及 relay 插进 step 的备用回复。
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture.router.push_kind(crate::common::text_sse("收到"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-after-1", "收尾停")))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;

    let events = crate::common::read_root_events(fixture.data());
    let (text, is_error) = tool_result(&events, "call-stop-after");
    assert!(!is_error.unwrap_or(false), "{text}");
    assert!(text.contains("已收尾停"), "{text}");

    let store = tasks_store(fixture.data());
    wait_until("Attempt 记 stopped", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let task_row = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        task_row.latest_report().map(|report| report.state),
        Some(ReportState::Waiting),
        "没报：waiting 合成后记 stopped"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
}

/// 改小席位不掐断在跑的任务，只影响之后的准入：两个在跑的照常报完，
/// 改小后派的第三个按新上限排队。
#[tokio::test]
async fn lowering_seats_does_not_stop_running_tasks_and_gates_the_next_launch() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    router.set_thin_delay(Duration::from_millis(2_500));
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
    wait_until("两个任务都开跑", || {
        store
            .task(&first.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
            && store
                .task(&second.id)
                .unwrap()
                .unwrap()
                .latest_attempt()
                .is_some()
            && attempt_in_flight(&store, &first.id)
            && attempt_in_flight(&store, &second.id)
    })
    .await;

    host.set_exec_seats(1).expect("改小席位");
    assert!(
        attempt_in_flight(&store, &first.id) && attempt_in_flight(&store, &second.id),
        "改小席位不掐断在跑的两个任务"
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
        "改小后新任务按 1 个席位准入"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, third.id.as_str())
            .unwrap(),
        "第三个任务在排队"
    );

    for _ in 0..3 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    wait_until("在跑的两个都报 ready", || {
        reported_ready(&store, &first.id) && reported_ready(&store, &second.id)
    })
    .await;
    wait_until("最后一个任务开跑", || {
        store
            .task(&third.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some()
    })
    .await;
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
    assert_eq!(attempt_end(&store, &first.id), Some(AttemptEnd::Reported));
    assert_eq!(attempt_end(&store, &second.id), Some(AttemptEnd::Reported));
}

/// 席位 1 时 person 叫醒能用多出来的 1 个席位；第二个停任务按 Later 留在库里。
#[tokio::test]
async fn a_person_wake_uses_the_extra_seat_and_the_second_stopped_task_waits() {
    let server = MockServer::start().await;
    let router = Router::new();
    mount(&server, &router).await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open_host(&server, data.path(), cwd.path()).await;
    let store = tasks_store(data.path());
    for title in ["甲", "乙", "丙"] {
        for script in ready_scripts() {
            router.push_thin_for_title(title, script);
        }
    }
    // 派发先压住：三次受理的 Kind 轮次会互相挤脚本，脚本都排好了再一起开跑。
    host.task_runtime().expect("runtime").set_pump_paused(true);
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
    for _ in 0..3 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.task_runtime().expect("runtime").set_pump_paused(false);
    wait_until("三个任务都报 ready", || {
        reported_ready(&store, &first.id)
            && reported_ready(&store, &second.id)
            && reported_ready(&store, &third.id)
    })
    .await;

    // 报告那一轮之后执行者还要写最后一段回答；那段没跑完，会话写者不放开，叫醒的投递会一直 Later。
    for task in [&first, &second, &third] {
        let attempt = store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .cloned()
            .expect("报告过的 Attempt");
        wait_until("报告那一轮收尾", || {
            session_turn_ended(data.path(), &attempt)
        })
        .await;
    }

    host.set_exec_seats(1).expect("席位改成 1");
    // 甲、乙被叫醒后的这两轮要一直挂在跑：席位在 turn 结束就放（turn_ended），
    // 丙的那句必须落进「两个人都在跑」的窗口。泵串行开两个会话本身可能要 1–2s，
    // 用固定 1.5s 延迟时丙会合法地占上空出来的席位（并行跑偶发红）；这里挂 30s，
    // 收尾直接立刻停掉这两轮，不等延迟。
    router.set_thin_delay(Duration::from_secs(30));
    router.push_thin_for(first.id.as_str(), crate::common::text_sse("接着做甲"));
    router.push_thin_for(second.id.as_str(), crate::common::text_sse("接着做乙"));

    let said = say_in_task_dialog(&host, &first, "甲接着做").await;
    assert_eq!(said.new_attempt, Some(2));
    wait_until("甲被叫醒", || attempt_in_flight(&store, &first.id)).await;

    let said = say_in_task_dialog(&host, &second, "乙接着做").await;
    assert_eq!(said.new_attempt, Some(2));
    wait_until("乙用多出来的席位起来", || {
        attempt_in_flight(&store, &second.id)
    })
    .await;

    let third_before = store.task(&third.id).unwrap().unwrap();
    let said = say_in_task_dialog(&host, &third, "丙接着做").await;
    assert_eq!(said.new_attempt, Some(2), "回执只是预览，不代表真开");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let third_after = store.task(&third.id).unwrap().unwrap();
    assert_eq!(
        attempt_count(&third_after),
        attempt_count(&third_before),
        "席位满的丙不开新 Attempt"
    );
    assert!(
        third_after
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_some()),
        "丙还停在上一轮结束的状态"
    );
    assert!(
        store
            .effect_pending(EffectKind::Delivery, third.id.as_str())
            .unwrap(),
        "丙的 delivery 留在库里等席位腾出来"
    );
    assert_eq!(
        store.effect_health(EffectKind::Delivery).unwrap().attempts,
        0,
        "满席位是 Later，不算效果失败"
    );

    // 收尾：立刻停掉甲、乙，把挂着的模型请求掐掉，不等 30s 的延迟。
    for _ in 0..2 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    for task in [&first, &second] {
        host.dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停止");
    }
    wait_until("甲、乙停下", || {
        attempt_end(&store, &first.id) == Some(AttemptEnd::Stopped)
            && attempt_end(&store, &second.id) == Some(AttemptEnd::Stopped)
    })
    .await;
}

/// 立即停之后在任务对话里说话：新 Attempt 接在原会话上，请求带着停之前的历史。
#[tokio::test]
async fn saying_after_a_stop_resumes_the_same_conversation() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;
    // 第一轮一直挂着，好在跑着的时候立即停。
    fixture.router.set_thin_delay(Duration::from_millis(800));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;
    fixture.router.push_kind(crate::common::text_sse("收到"));
    let stop = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stop.stopped, "{stop:?}");
    fixture.router.clear_thin_delay();
    wait_until("第一轮停住", || {
        attempt_end(&store, &task.id) == Some(AttemptEnd::Stopped)
    })
    .await;
    let stopped = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("停住的 Attempt");
    let conversation = stopped.conversation.clone().expect("会话 id");

    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("接着做"));
    let said = say_in_task_dialog(&fixture.host, &task, "停后再说一句").await;
    assert_eq!(said.new_attempt, Some(stopped.id.0 + 1));
    wait_until("停后说话开了新 Attempt", || {
        attempt_end(&store, &task.id).is_none()
    })
    .await;
    let resumed = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("新 Attempt");
    assert_eq!(resumed.id.0, stopped.id.0 + 1);
    assert_eq!(
        resumed.conversation.as_ref(),
        Some(&conversation),
        "停后接着做还是原会话"
    );
    assert_eq!(resumed.trigger, tasks::AttemptTrigger::UserMessage);
    assert!(resumed.ended_at.is_none());

    wait_until("停后的请求带上新句子", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("停后再说一句"))
    })
    .await;
    let last = fixture
        .router
        .bodies(true)
        .last()
        .cloned()
        .expect("停后的请求");
    assert!(last.contains("找答案"), "停后的请求带着原来的历史");
}

/// 重启后 Kind 收到恰好一条汇报、出一张每任务一项的接续卡；确认一项只接这一项。
#[tokio::test]
async fn a_restart_reports_interrupted_tasks_and_resumes_the_confirmed_one() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
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
    crash_while_running(&router, data.path(), host, &[first.clone(), second.clone()]).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[first, second]);
    let conversations: Vec<_> = crashed
        .iter()
        .map(|task| task.latest_attempt().unwrap().conversation.clone())
        .collect();
    let task_refs: Vec<&Task> = crashed.iter().collect();

    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲乙都被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;

    // 启动扫描在效果循环之前收尾：两个 Attempt 都记成 interrupted。
    for task in &crashed {
        let stored = store.task(&task.id).unwrap().unwrap();
        assert_eq!(
            stored.latest_attempt().unwrap().end,
            Some(AttemptEnd::Interrupted),
            "{}",
            task.title
        );
        assert_eq!(stored.stage(false), Stage::NeedsConditions);
    }

    wait_until("Kind 收到重启汇报", || {
        !restart_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;
    wait_until("接续卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    let events = crate::common::read_root_events(data.path());
    let reports = restart_messages(&events);
    assert_eq!(reports.len(), 1, "同一次启动只送一条 restart：{reports:?}");
    assert_eq!(reports[0].1["kind"], "tasks_interrupted");
    let listed = reports[0].1["tasks"].as_array().expect("tasks 数组");
    assert_eq!(listed.len(), 2, "{}", reports[0].1);
    for task in &crashed {
        let entry = listed
            .iter()
            .find(|entry| entry["id"] == task.id.as_str())
            .expect("任务在汇报里");
        assert_eq!(entry["title"], task.title);
        assert_eq!(entry["attempts"], 1);
    }
    let options = card_options(&events, "call-restart-card");
    assert_eq!(options.len(), 2, "{options:?}");
    let fixed = serde_json::json!([{ "name": "状态", "value": "被打断了 → 新的 Attempt，接着做" }]);
    for task in &crashed {
        let option = options
            .iter()
            .find(|option| option["id"] == task.id.as_str())
            .expect("每个任务一项");
        assert_eq!(option["label"], task.title);
        assert_eq!(option["facts"], fixed);
    }

    // 确认甲：新 Attempt、trigger resume、会话不变；没勾的仍是「被打断了」。
    let first_id = crashed[0].id.as_str().to_owned();
    router.push_thin_for(&first_id, crate::common::text_sse("接着做"));
    router.push_kind(crate::common::text_sse("收到"));
    let decision = confirm_card_option(&host, "call-restart-card", &first_id).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    wait_until("甲开了第二次 Attempt", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&crashed[0].id).unwrap().unwrap();
    let second_attempt = resumed.latest_attempt().unwrap().clone();
    assert_eq!(second_attempt.trigger, tasks::AttemptTrigger::Resume);
    assert_eq!(
        second_attempt.conversation, conversations[0],
        "会话 id 不变"
    );
    assert!(second_attempt.ended_at.is_none());
    let untouched = store.task(&crashed[1].id).unwrap().unwrap();
    assert_eq!(
        untouched.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted)
    );
    assert_eq!(attempt_count(&untouched), 1);

    // 第一条输入是恢复说明，原文照 spec；请求真的把它送到 ThinAgent。
    wait_until("恢复说明进了任务会话", || {
        recovery_notes(&thin_events(data.path(), &second_attempt)).len() == 1
    })
    .await;
    let notes = recovery_notes(&thin_events(data.path(), &second_attempt));
    assert_eq!(
        notes[0].0,
        format!("resume:{first_id}:{}", second_attempt.id),
        "恢复说明的判重键"
    );
    assert_eq!(notes[0].1, RECOVERY_NOTE);
    wait_until("接续请求带上恢复说明", || {
        router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains(RECOVERY_NOTE))
    })
    .await;

    // 同一张卡再点一次不重复执行。
    let again = confirm_card_option(&host, "call-restart-card", &first_id).await;
    assert_eq!(again, decision);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        attempt_count(&store.task(&crashed[0].id).unwrap().unwrap()),
        2
    );
    drop(host);
}

/// 点「暂不」：任务继续停着，之后不再有新的 restart 汇报。
#[tokio::test]
async fn dismissing_the_restart_card_leaves_the_task_interrupted_without_another_restart() {
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

    let decision = dismiss_card(&host, "call-restart-card").await;
    assert_eq!(decision["phase"], "dismissed", "{decision}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        restart_messages(&crate::common::read_root_events(data.path())).len(),
        1,
        "暂不之后不再有新的 restart"
    );
    let stored = store.task(&crashed[0].id).unwrap().unwrap();
    assert_eq!(
        stored.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted)
    );
    assert_eq!(attempt_count(&stored), 1, "暂不不开新 Attempt");
    drop(host);
}

/// 被打断后先在任务对话里说一句：新 Attempt 的 trigger 是 resume，第一条消息
/// 恢复说明在前、用户的话在后，判重键是 resume:<taskId>:<attemptId>，页头不再带停止标记。
#[tokio::test]
async fn saying_after_an_interruption_resumes_with_the_recovery_note_before_the_user_words() {
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
    let first = crashed[0]
        .latest_attempt()
        .cloned()
        .expect("第一次 Attempt");
    let conversation = first.conversation.clone().expect("会话 id");
    let task_refs: Vec<&Task> = crashed.iter().collect();
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    let interrupted = store.task(&crashed[0].id).unwrap().unwrap();
    assert_eq!(
        interrupted.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted),
        "启动扫描先把上次的 Attempt 收尾"
    );
    wait_until("Kind 收到重启汇报", || {
        !restart_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;

    router.push_thin_for(crashed[0].id.as_str(), crate::common::text_sse("接着做"));
    let said = say_in_task_dialog(&host, &crashed[0], "把剩下两个线索查完").await;
    assert_eq!(said.new_attempt, Some(first.id.0 + 1));
    wait_until("说话开了新 Attempt", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&crashed[0].id).unwrap().unwrap();
    let second = resumed.latest_attempt().cloned().expect("第二次 Attempt");
    assert_eq!(second.trigger, tasks::AttemptTrigger::Resume);
    assert_eq!(
        second.conversation.as_ref(),
        Some(&conversation),
        "会话 id 不变"
    );
    assert!(second.ended_at.is_none());

    wait_until("恢复说明进了任务会话", || {
        recovery_notes(&thin_events(data.path(), &second)).len() == 1
    })
    .await;
    let notes = recovery_notes(&thin_events(data.path(), &second));
    assert_eq!(
        notes[0].0,
        format!("resume:{}:{}", crashed[0].id, second.id),
        "恢复说明的判重键"
    );
    let note_at = notes[0]
        .1
        .find(RECOVERY_NOTE)
        .expect("恢复说明在同一条消息里");
    let words_at = notes[0]
        .1
        .find("把剩下两个线索查完")
        .expect("用户的话在同一条消息里");
    assert!(note_at < words_at, "恢复说明在用户的话前面：{}", notes[0].1);

    wait_until("下一次请求带着恢复说明和用户的话", || {
        router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains(RECOVERY_NOTE) && body.contains("把剩下两个线索查完"))
    })
    .await;
    let body = router.bodies(true).last().cloned().expect("接续请求");
    let note_at = body.find(RECOVERY_NOTE).expect("恢复说明在请求里");
    let words_at = body.find("把剩下两个线索查完").expect("用户的话在请求里");
    assert!(note_at < words_at, "请求体里恢复说明在用户的话前面");

    // 新 Attempt 已经开跑：页头不再带上一 Attempt 的停止标记，阶段也不是被打断了。
    let read = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: crashed[0].id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read");
    let Response::TaskRead(read) = read else {
        panic!("TaskRead 的响应形状不对");
    };
    assert!(!read.task.stopped, "不残留停止标记：{:?}", read.task);
    assert_eq!(read.task.attempts, 2);
    assert_ne!(
        resumed.stage(false),
        Stage::NeedsConditions,
        "新 Attempt 开跑后不再是被打断了"
    );
    drop(host);
}

/// 打断后的说话接续占 wake 席位，不额外多占：席位满时那句留在库里，腾出来再接。
#[tokio::test]
async fn saying_after_an_interruption_uses_a_wake_seat_and_waits_when_all_are_taken() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
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
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&first)).await;

    let store = tasks_store(data.path());
    let crashed = reload_tasks(&store, &[first]);
    // 重启前把席位改成 1：唤醒接续占满以后，说话只能等。
    let mut saved = service::settings::load(data.path());
    saved.exec.seats = 1;
    service::settings::save(data.path(), &saved).expect("保存席位");
    router.clear_thin_delay();
    let task_refs: Vec<&Task> = crashed.iter().collect();
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
        Some(AttemptEnd::Interrupted)
    );
    wait_until("Kind 收到重启汇报", || {
        !restart_messages(&crate::common::read_root_events(data.path())).is_empty()
    })
    .await;
    wait_until("接续卡出来，这一轮收尾", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    // 新派一个任务占掉唯一的席位，这一轮挂住不结束。
    router.set_thin_delay(Duration::from_secs(30));
    router.push_thin_for_title("乙", crate::common::text_sse("还在做"));
    let filler = accept_task(
        &host,
        &router,
        data.path(),
        "kind-b",
        "call-accept-b",
        3,
        "乙",
    )
    .await;
    wait_until("乙占住席位", || attempt_in_flight(&store, &filler.id)).await;

    // 甲说话：wake 不超席位，Attempt 不增加，delivery 留在库里等。
    let said = say_in_task_dialog(&host, &crashed[0], "甲先把没跑完的补上").await;
    assert_eq!(said.new_attempt, Some(2), "回执只是预览，不代表真开");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let waiting = store.task(&crashed[0].id).unwrap().unwrap();
    assert_eq!(attempt_count(&waiting), 1, "席位满时打断接续不开 Attempt");
    assert!(
        store
            .effect_pending(EffectKind::Delivery, crashed[0].id.as_str())
            .unwrap(),
        "甲的 delivery 留在库里等席位"
    );
    assert_eq!(
        store.effect_health(EffectKind::Delivery).unwrap().attempts,
        0,
        "Later 不算失败"
    );

    // 乙停下放出席位：甲的那句被重新领取，按 resume 接上。
    router.push_kind(crate::common::text_sse("收到"));
    router.push_thin_for(crashed[0].id.as_str(), crate::common::text_sse("甲接着做"));
    let response = host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: filler.id.as_str().to_owned(),
            mode: StopMode::Now,
        }))
        .await
        .expect("停止");
    let Response::TaskStop(stop) = response else {
        panic!("TaskStop 的响应形状不对");
    };
    assert!(stop.stopped, "{stop:?}");
    wait_until("甲接上", || {
        store
            .task(&crashed[0].id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&crashed[0].id).unwrap().unwrap();
    let attempt = resumed.latest_attempt().cloned().expect("甲的新 Attempt");
    assert_eq!(attempt.trigger, tasks::AttemptTrigger::Resume);
    assert!(attempt.ended_at.is_none());
    drop(host);
}

/// 页头事实：启动扫描打断后一直「被打断了」；给 Kind 的 restart 还在队里时
/// restartPending 为真，送到后转假（横幅消失，任务不自动跑）。
#[tokio::test]
async fn task_read_reports_interrupted_and_restart_pending_until_kind_is_told() {
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
    let mut saved = service::settings::load(data.path());
    saved.exec.paused = true;
    service::settings::save(data.path(), &saved).expect("保存暂停");
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;

    // 扫描照做，restart 不送：页头同时是「被打断了」和「待 Kind 核实」。
    wait_until("restart 留在库里", || {
        store
            .effect_health(EffectKind::Restart)
            .expect("效果健康")
            .waiting
            == 1
    })
    .await;
    let before = task_read_header(&host, &crashed[0].id).await;
    assert!(before.interrupted, "扫描后页头要固定「被打断了」");
    assert!(
        before.restart_pending,
        "Kind 还没收到 restart：页头要「待 Kind 核实，尚未接续」"
    );
    assert_eq!(before.attempts, 1, "没自动重跑");

    host.dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
        currency: None,
        paused: Some(false),
        seats: None,
        bash: None,
    }))
    .await
    .expect("开闸");
    wait_until("Kind 收到重启汇报", || {
        restart_messages(&crate::common::read_root_events(data.path())).len() == 1
    })
    .await;

    let after = task_read_header(&host, &crashed[0].id).await;
    assert!(after.interrupted, "restart 送到后任务仍是「被打断了」");
    assert!(!after.restart_pending, "restart 送到后横幅消失");
    assert_eq!(after.attempts, 1, "送汇报不自动开新的 Attempt");
    drop(host);
}

/// 第二次启动只报这次扫描打断的任务：上次已暂不的甲不再上卡，仍停在「被打断了」。
#[tokio::test]
async fn a_second_restart_reports_only_the_tasks_its_scan_interrupted() {
    let (server, router, data, cwd) = restart_setup().await;
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
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
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&first)).await;

    let store = tasks_store(data.path());
    let interrupted = reload_tasks(&store, &[first]);
    let first_id = interrupted[0].id.clone();
    let task_refs: Vec<&Task> = interrupted.iter().collect();
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-a", &task_refs, "甲被打断了。");
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    wait_until("第一次重启的卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-a",
        )
        .is_empty()
    })
    .await;
    // 用户点「暂不」：甲继续停着，Kind 不再追问。
    dismiss_card(&host, "call-restart-a").await;

    // 第二次启动的服务里乙跑着，进程停掉时它还没交结果。
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
    crash_while_running(&router, data.path(), host, std::slice::from_ref(&second)).await;

    let interrupted = reload_tasks(&store, &[second]);
    let second_id = interrupted[0].id.clone();
    let task_refs: Vec<&Task> = interrupted.iter().collect();
    push_restart_turn(&router, "call-restart-b", &task_refs, "乙被打断了。");
    let host = reopen_actions_host(&server, data.path(), cwd.path()).await;

    wait_until("第二次重启汇报", || {
        restart_messages(&crate::common::read_root_events(data.path())).len() == 2
    })
    .await;
    wait_until("第二次重启的卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-b",
        )
        .is_empty()
    })
    .await;
    let events = crate::common::read_root_events(data.path());
    let reports = restart_messages(&events);
    let latest = &reports[1].1;
    let listed: Vec<&str> = latest["tasks"]
        .as_array()
        .expect("tasks 数组")
        .iter()
        .map(|entry| entry["id"].as_str().expect("任务 id"))
        .collect();
    assert_eq!(
        listed,
        vec![second_id.as_str()],
        "只报这次扫描打断的乙：{latest}"
    );

    // 甲没有被再问一次，也没有被悄悄再接续：仍是一次 interrupted 的 Attempt。
    let stored_first = store.task(&first_id).unwrap().unwrap();
    assert_eq!(
        stored_first.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted)
    );
    assert_eq!(attempt_count(&stored_first), 1);
    assert_eq!(stored_first.stage(false), Stage::NeedsConditions);

    let options = card_options(&events, "call-restart-b");
    assert_eq!(options.len(), 1, "{options:?}");
    assert_eq!(options[0]["id"], second_id.as_str());
    drop(host);
}

/// 立即停：连这个任务的后台命令一起停（进程收到 SIGTERM 并退出）。
///
/// 后台命令把收到 TERM 写成 cwd 里的文件；没被杀掉时它会自己睡完 60 秒，
/// 所以红的时候这里等不到文件，也不会留下一个永远不退的进程。
#[tokio::test]
async fn immediately_stopping_a_task_kills_its_background_jobs() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    // 第一轮回复慢一点：任务要在后台命令起来、第二轮还在途的时候被停。
    fixture.router.set_thin_delay(Duration::from_millis(2_000));
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-bash",
            "bash",
            &serde_json::json!({
                "description": "等停止信号",
                "command": "trap 'echo stopped > killed.txt' TERM; sleep 60",
                "run_in_background": true
            })
            .to_string(),
        ),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("还在等"));

    let task = fixture.accept_args(false, write_task_args(), 1).await;
    let store = tasks_store(fixture.data());
    wait_until("后台任务登记", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| {
                let Some(conversation) = attempt.conversation else {
                    return false;
                };
                let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
                    return false;
                };
                session::SessionStore::open_read_only(fixture.data(), &id).is_ok_and(|session| {
                    session.events().iter().any(|event| {
                        matches!(&event.kind, session::EventKind::JobRegistered { .. })
                    })
                })
            })
    })
    .await;

    let stop = stop_task(&fixture, &task, StopMode::Now).await;
    assert!(stop.stopped, "{stop:?}");
    wait_until("后台命令收到 SIGTERM", || {
        fixture.cwd.path().join("killed.txt").exists()
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("killed.txt")).unwrap(),
        "stopped\n",
        "后台命令收到 TERM 后落了文件"
    );
}

/// 确认卡接续被打断的工作树任务：新 Attempt 还在同一棵树，不再 add。
#[tokio::test]
async fn confirming_an_interrupted_worktree_task_resumes_in_the_same_worktree() {
    let (server, router, data, cwd) = restart_setup().await;
    init_git_repo(cwd.path());
    let host = open_actions_host(&server, data.path(), cwd.path()).await;
    host.task_runtime().expect("runtime").set_pump_paused(true);
    kind_turn(
        &host,
        &router,
        data.path(),
        "kind-a",
        "派一个任务",
        1,
        vec![
            crate::common::tool_use_sse(
                0,
                "call-accept-a",
                "task_accept",
                &worktree_task_args().to_string(),
            ),
            crate::common::text_sse("已派出"),
        ],
    )
    .await;
    let store = tasks_store(data.path());
    let task = one_task(&store);

    router.push_thin_for_title("改一下判重", crate::common::text_sse("还在做"));
    router.set_thin_delay(Duration::from_secs(10));
    host.task_runtime().expect("runtime").set_pump_paused(false);
    wait_until("工作树任务在跑", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.conversation.is_some() && attempt.ended_at.is_none())
    })
    .await;
    let worktree_root = std::fs::canonicalize(data.path().join("worktrees").join(task.id.as_str()))
        .expect("工作树目录");
    let worktree_cwd = std::fs::canonicalize(worktree_root.join("sub")).expect("工作树 cwd");
    let listed_before = listed_worktrees(cwd.path());
    assert_eq!(listed_before.len(), 2, "{listed_before:?}");

    host.task_runtime().expect("runtime").crash();
    drop(host);
    router.clear_thin_delay();
    push_restart_turn(&router, "call-restart-card", &[&task], "被打断了。");
    let host = reopen_actions_host(&server, data.path(), cwd.path()).await;
    wait_until("接续卡出来", || {
        !card_options(
            &crate::common::read_root_events(data.path()),
            "call-restart-card",
        )
        .is_empty()
    })
    .await;

    router.push_thin_for(task.id.as_str(), crate::common::text_sse("接着做"));
    router.push_kind(crate::common::text_sse("收到"));
    let decision = confirm_card_option(&host, "call-restart-card", task.id.as_str()).await;
    assert_eq!(decision["phase"], "confirmed", "{decision}");
    wait_until("接续开了新的 Attempt", || {
        store
            .task(&task.id)
            .ok()
            .flatten()
            .is_some_and(|task| attempt_count(&task) == 2)
    })
    .await;
    let resumed = store.task(&task.id).unwrap().unwrap();
    let second = resumed.latest_attempt().unwrap().clone();
    assert_eq!(second.trigger, tasks::AttemptTrigger::Resume);
    wait_until("接续请求带上恢复说明", || {
        router
            .bodies(true)
            .iter()
            .any(|body| body.contains(RECOVERY_NOTE))
    })
    .await;
    let body = router
        .bodies(true)
        .into_iter()
        .find(|body| body.contains(RECOVERY_NOTE))
        .expect("接续请求");
    assert_eq!(
        request_working_directory(&body),
        worktree_cwd.display().to_string(),
        "接续的新 Attempt 的 cwd 仍在工作树里"
    );
    assert_eq!(
        listed_worktrees(cwd.path()),
        listed_before,
        "不再 git worktree add"
    );
    drop(host);
}

/// A 跑着时重启——A 被打断；B 和总任务不出被打断，B 仍是待补足条件。
#[tokio::test]
async fn graph_restart_interrupts_running_child_but_not_root_or_waiting_sibling() {
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
    // A 的 ThinAgent 压后 10 秒，进程退出前它会一直挂着。
    router.push_thin_for(children[0].id.as_str(), crate::common::text_sse("还在做"));
    router.set_thin_delay(Duration::from_secs(10));
    runtime.set_pump_paused(false);
    wait_until("A 在跑", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| attempt.conversation.is_some() && attempt.ended_at.is_none())
    })
    .await;
    runtime.crash();
    drop(host);
    for _ in 0..6 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    let mut reopened = false;
    for _ in 0..250 {
        let host = open_graph_host(&server, data.path(), cwd.path()).await;
        if matches!(host.info().root, service::RootState::Ok) {
            reopened = true;
            let store = tasks_store(data.path());
            wait_until("启动扫描收尾", || {
                store
                    .task(&children[0].id)
                    .ok()
                    .flatten()
                    .and_then(|task| task.latest_attempt().cloned())
                    .is_some_and(|attempt| attempt.end.is_some())
            })
            .await;
            let store = tasks_store(data.path());
            let a = store.task(&children[0].id).expect("读任务").expect("A");
            assert_eq!(
                a.latest_attempt().and_then(|attempt| attempt.end),
                Some(tasks::AttemptEnd::Interrupted),
                "A 被打断了"
            );
            let b = store.task(&children[1].id).expect("读任务").expect("B");
            assert!(b.latest_attempt().is_none(), "B 没开跑，不出被打断");
            assert!(!b.is_interrupted());
            let total = store.task(&root.id).expect("读任务").expect("总任务");
            assert!(total.latest_attempt().is_none(), "总任务没有 Attempt");
            assert!(!total.is_interrupted());
            assert_eq!(
                store.stage(&children[1].id).expect("阶段"),
                Stage::NeedsConditions
            );
            assert_eq!(store.stage(&root.id).expect("阶段"), Stage::NeedsConditions);
            drop(host);
            break;
        }
        drop(host);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reopened, "重开根宿主 250 次都没到 RootState::Ok");
}

/// 门报 blocked 后，Kind 用 task_continue AfterReport 让门重跑。
#[tokio::test]
async fn task_continue_on_blocked_gate_reruns_with_after_report_cause() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    router.enable_gate_kind();
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再核再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建门图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 4)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let (guarded, gate, _downstream) = gate_children(&store, &root);
    router.push_thin_for(
        guarded.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b-report",
            "task_report",
            r#"{"state":"ready","summary":"第一次结果：3 行"}"#,
        ),
    );
    router.push_thin_for(guarded.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        gate.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-gate-blocked",
            "task_report",
            r#"{"state":"blocked","summary":"缺原始链接，核不了","question":"要查什么？"}"#,
        ),
    );
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("好"));
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-gate-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),
        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");
    wait_until("门报 blocked", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Blocked)
    })
    .await;
    // 门这一轮先收尾（会话写锁放开）再接着做。
    wait_until("门第一轮结束", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_attempt().cloned())
            .is_some_and(|attempt| session_turn_ended(data.path(), &attempt))
    })
    .await;
    // Kind 用 task_continue 让门重跑（原因：最近报告 blocked）。
    router.set_thin_delay(Duration::from_millis(30));
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("重新核过了"));
    // 会话写锁可能还在收尾：像 Kind 一样稍后重试（工具回的是「稍后再试一次」）。
    let mut response = None;
    for _ in 0..100 {
        match host
            .dispatch(Call::TaskContinue(service::TaskContinueRequest {
                task_id: gate.id.as_str().to_owned(),
                note: None,
            }))
            .await
        {
            Ok(response_now) => {
                response = Some(response_now);
                break;
            }
            Err(error) if error.to_string().contains("稍后再试") => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("门重跑失败：{error}"),
        }
    }
    let response = response.expect("门重跑一直接不上");
    let Response::TaskContinue(response) = response else {
        panic!("task-continue 响应");
    };
    wait_until("门第二次 Attempt 开跑", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.latest_attempt()
                    .is_some_and(|attempt| attempt.id.0 == response.attempt_id)
            })
    })
    .await;
    let gate_now = tasks_store(data.path()).task(&gate.id).unwrap().unwrap();
    let attempt = gate_now.latest_attempt().unwrap();
    match &attempt.trigger {
        tasks::AttemptTrigger::KindContinue { cause, .. } => assert!(
            matches!(cause, tasks::ContinueCause::AfterReport { .. }),
            "cause 是 {cause:?}"
        ),
        other => panic!("trigger 是 {other:?}"),
    }
}

#[tokio::test]
async fn update_gate_commands_act_stops_a_running_gate_then_reruns_the_new_commands() {
    use service::ActionRegistry;
    use tasks::{GateCommand, NewGraph, NewSubtask, TaskPolicy};

    let fixture = Fixture::new_with_actions().await;
    let runtime = fixture.host.task_runtime().expect("任务执行器");
    runtime.set_pump_paused(true);
    let store = tasks_store(fixture.data());
    let draft = |title: &str| NewTask {
        title: title.to_owned(),
        requirement: "把事做完".to_owned(),
        dod: "有结果".to_owned(),
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
    };
    let root = store
        .create_graph(
            NewGraph {
                root: draft("总任务"),
                subtasks: vec![
                    NewSubtask {
                        task: draft("被把关"),
                        policy: TaskPolicy::default(),
                        after: Vec::new(),
                        milestone: false,
                        program_gate: Vec::new(),
                    },
                    NewSubtask {
                        task: draft("程序门"),
                        policy: TaskPolicy::default(),
                        after: vec![0],
                        milestone: true,
                        program_gate: vec![
                            GateCommand {
                                command: "trap '' TERM; sleep 30".to_owned(),
                                timeout_ms: None,
                            },
                            GateCommand {
                                command: "true".to_owned(),
                                timeout_ms: None,
                            },
                        ],
                    },
                ],
            },
            1_700_000_000_000,
        )
        .unwrap();
    store.confirm_graph(&root.id, 1_700_000_000_001).unwrap();
    let children = store.children(&root.id).unwrap();
    let guarded = children[0].clone();
    let gate = children[1].clone();
    let guarded_attempt = store.begin_attempt(&guarded.id, 1_700_000_000_002).unwrap();
    store
        .record_report(
            &guarded.id,
            guarded_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "改好了".to_owned(),
                question: None,
            },
            1_700_000_000_003,
        )
        .unwrap();
    // 把被把关任务那条 launch 效果收掉，放泵去跑门的 launch。
    while let Some(claim) = store
        .claim_effect(EffectKind::Launch, 1_700_000_000_010)
        .unwrap()
    {
        let task_id = match &claim.payload {
            tasks::EffectPayload::Launch { task_id } => task_id.clone(),
            _ => unreachable!(),
        };
        if task_id == gate.id {
            break;
        }
        store.complete_effect(&claim).unwrap();
    }
    runtime.set_pump_paused(false);
    let mut running = false;
    for _ in 0..300 {
        let current = store.task(&gate.id).unwrap().unwrap();
        if current
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none())
        {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(running, "门应在跑 sleep 3，方便测先停");

    let actions = ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = actions.get("update_gate_commands").expect("动作已登记");
    let outcome = action
        .act(
            &serde_json::json!({
                "gate_task_id": gate.id.as_str(),
                "commands": [{"command": "true"}, {"command": "printf v2"}],
            }),
            &fixture.host,
        )
        .await
        .expect("改命令要成功");
    assert!(outcome.summary.contains("第 2 版"), "{}", outcome.summary);

    let mut updated = None;
    for _ in 0..400 {
        let current = store.task(&gate.id).unwrap().unwrap();
        if current.commands_version() == 2
            && current
                .reports
                .iter()
                .filter(|report| report.origin == ReportOrigin::Program)
                .count()
                >= 2
        {
            updated = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let updated = updated.expect("第 2 版应跑出新报告");
    assert_eq!(
        updated
            .gate_commands()
            .iter()
            .map(|item| item.command.as_str())
            .collect::<Vec<_>>(),
        ["true", "printf v2"]
    );
    let programs: Vec<_> = updated
        .reports
        .iter()
        .filter(|report| report.origin == ReportOrigin::Program)
        .collect();
    assert_eq!(programs.len(), 2);
    assert!(
        programs[0].summary.contains("命令已改"),
        "{}",
        programs[0].summary
    );
    assert_eq!(
        updated.verdicts[0].conclusion,
        tasks::GateConclusion::Undetermined
    );
    assert_eq!(updated.verdicts[0].commands_version, 1);
    assert_eq!(updated.verdicts[1].conclusion, tasks::GateConclusion::Pass);
    assert_eq!(updated.verdicts[1].commands_version, 2);
    assert!(
        updated.verdicts[1].candidate.report_id != updated.verdicts[0].candidate.report_id
            || updated.verdicts[1].candidate.requirement_version
                == updated.verdicts[0].candidate.requirement_version
    );
}
