//! 任务对话与抄送：说法、等待、粘贴/附件、重复投递。

use super::*;

/// 在跑的时候说一句：Steer 进下一步，Attempt 数不变。
#[tokio::test]
async fn saying_while_running_steers_the_next_step_without_a_new_attempt() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;
    fixture.router.set_thin_delay(Duration::from_millis(1_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("第一步收尾"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;
    let attempts_before = attempt_count(&store.task(&task.id).unwrap().unwrap());

    let said = say_in_task_dialog(&fixture.host, &task, "插一句").await;
    assert_eq!(said.new_attempt, None, "在跑时说话不预览新 Attempt");

    wait_until("下一步请求带上这句话", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("插一句"))
    })
    .await;
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        attempt_count(&after),
        attempts_before,
        "在跑时说话不开新 Attempt"
    );
    assert!(
        after
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none()),
        "还是同一个 Attempt 在跑"
    );
}

/// 全局暂停时说的这句压着不送（不算失败），开闸后按原输入送进原会话。
#[tokio::test]
async fn a_paused_task_dialog_holds_the_input_until_the_gate_opens() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(false).await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "第一轮的答案", None));
    fixture
        .router
        .push_thin(crate::common::text_sse("第一轮收尾"));
    for _ in 0..2 {
        fixture.router.push_kind(crate::common::text_sse("收到"));
    }
    let store = tasks_store(fixture.data());
    wait_until("第一轮报 ready", || reported_ready(&store, &task.id)).await;
    let attempts_before = attempt_count(&store.task(&task.id).unwrap().unwrap());

    fixture
        .host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            paused: Some(true),
            ..Default::default()
        }))
        .await
        .expect("暂停");
    let said = say_in_task_dialog(&fixture.host, &task, "暂停时说的").await;
    assert_eq!(said.new_attempt, Some(attempts_before as u64 + 1));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let held = store.task(&task.id).unwrap().unwrap();
    assert_eq!(attempt_count(&held), attempts_before, "暂停时不接着开");
    assert!(
        store
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap(),
        "delivery 留在库里等开闸"
    );
    assert!(
        !fixture
            .router
            .bodies(true)
            .iter()
            .any(|body| body.contains("暂停时说的")),
        "暂停时这句话没送进任务会话"
    );

    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("开闸后接着做"));
    fixture
        .host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            paused: Some(false),
            ..Default::default()
        }))
        .await
        .expect("开闸");
    wait_until("开闸后开新 Attempt", || {
        attempt_count(&store.task(&task.id).unwrap().unwrap()) > attempts_before
    })
    .await;
    wait_until("开闸后请求带上这句话", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("暂停时说的"))
    })
    .await;
}

/// 抄送正文照 spec：{kind: task_cc, task:{id,title}, text<=600} + chars；
/// 用 Inject 进 Kind 日志但不叫醒它；全局暂停也照送。
#[tokio::test]
async fn a_task_cc_carries_the_task_and_truncated_text_without_waking_kind() {
    let fixture = Fixture::new().await;
    let task = first_report_fixture(&fixture, 2).await;
    fixture
        .host
        .dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
            paused: Some(true),
            ..Default::default()
        }))
        .await
        .expect("暂停");
    let kind_before = fixture.router.bodies(false).len();

    // 650 字：正文截到 600，chars 报原始字数；头尾不同的字便于确认截的是头。
    let long = format!("前{}后", "中".repeat(648));
    assert_eq!(long.chars().count(), 650);
    let said = say_in_task_dialog(&fixture.host, &task, &long).await;
    let request_id = format!("task-cc:{}", said.input_id);
    wait_until("Kind 日志里有 task-cc", || {
        cc_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(id, _)| id == &request_id)
    })
    .await;

    let events = crate::common::read_root_events(fixture.data());
    let body = cc_messages(&events)
        .into_iter()
        .find(|(id, _)| id == &request_id)
        .map(|(_, body)| body)
        .expect("task-cc 正文");
    assert_eq!(body["kind"], "task_cc");
    assert_eq!(body["task"]["id"], task.id.as_str());
    assert_eq!(body["task"]["title"], "查一下");
    assert_eq!(
        body["text"].as_str().unwrap(),
        format!("前{}", "中".repeat(599)),
        "正文最多 600 字"
    );
    assert_eq!(body["chars"], 650, "超出的字数单独报");
    assert!(
        tasks_store(fixture.data())
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap(),
        "全局暂停时 delivery 压着"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fixture.router.bodies(false).len(),
        kind_before,
        "Inject 不唤醒 Kind：没有新请求"
    );
    assert!(
        !crate::common::read_root_events(fixture.data())
            .iter()
            .any(|event| matches!(
                &event.kind,
                session::EventKind::UserMessage { message, .. }
                    if matches!(
                        &message.source,
                        model::MessageSource::User { rpc_id: Some(id) } if id == &request_id
                    )
            )),
        "没被叫醒，消息还压在 Kind 的下一步队列里"
    );

    // 另一条普通消息叫醒 Kind：请求里要带着这条抄送。
    fixture
        .router
        .push_kind(crate::common::text_sse("看到抄送了"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-2", "叫醒一下")))
        .await
        .expect("叫醒 Kind");
    wait_until("叫醒后的请求带着抄送", || {
        fixture
            .router
            .bodies(false)
            .last()
            .is_some_and(|body| body.contains("task_cc"))
    })
    .await;
    wait_until(
        "抄送被领取后变成 Kind 日志里的用户消息",
        || {
            crate::common::read_root_events(fixture.data())
                .iter()
                .any(|event| {
                    matches!(
                        &event.kind,
                        session::EventKind::UserMessage { message, .. }
                            if matches!(
                                &message.source,
                                model::MessageSource::User { rpc_id: Some(id) } if id == &request_id
                            )
                    )
                })
        },
    )
    .await;
}

/// 发起的 Kind 会话不是当前根会话：notice 按 Later 留在库里，不投送、不算失败。
#[tokio::test]
async fn a_task_cc_stays_later_when_its_kind_conversation_is_not_the_root() {
    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let task = store
        .create_task(
            tasks::NewTask {
                project_id: None,
                cwd_source: None,
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
            },
            1_700_000_000_000,
        )
        .unwrap();

    let said = say_in_task_dialog(&fixture.host, &task, "给另一个 Kind 的抄送").await;
    let key = format!("cc:{}", said.input_id);
    wait_until("notice 留在库里", || {
        store.effect_pending(EffectKind::Notice, &key).unwrap()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        cc_messages(&crate::common::read_root_events(fixture.data())).is_empty(),
        "发起的 Kind 不是当前根会话：不投送"
    );
    let health = store.effect_health(EffectKind::Notice).unwrap();
    assert_eq!(health.state, EffectState::Ready);
    assert_eq!(health.attempts, 0, "Later 不算失败");
}

/// 任务对话的附件：校验同 Kind（坏 id 拒收），投递到 ThinAgent 时文字块和文件块都在。
#[tokio::test]
async fn a_task_say_attachment_is_validated_and_lands_in_the_thin_request() {
    let fixture = Fixture::new().await;
    let task = first_report_fixture(&fixture, 3).await;
    let store = tasks_store(fixture.data());

    let missing = SendAttachment {
        id: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned(),
        name: "missing.txt".to_owned(),
        bytes: 1,
    };
    let error = fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "看附件".to_owned(),
            pastes: Vec::new(),
            attachments: vec![missing],
        }))
        .await
        .expect_err("不存在的附件要拒收");
    assert_eq!(error.code().as_str(), "attachment/invalid");
    assert_eq!(
        store.task(&task.id).unwrap().unwrap().inputs.len(),
        0,
        "拒收不改任务行"
    );

    let put = fixture
        .host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "notes.txt".to_owned(),
            data: "aGVsbG8=".to_owned(),
        }))
        .await
        .expect("存附件");
    let put = serde_json::to_value(&put).unwrap();
    let attachment_id = put["id"].as_str().unwrap().to_owned();
    let bytes = put["bytes"].as_u64().unwrap();

    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收到附件"));
    fixture.router.push_kind(crate::common::text_sse("收到"));
    let response = fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: "看附件".to_owned(),
            pastes: Vec::new(),
            attachments: vec![SendAttachment {
                id: attachment_id.clone(),
                name: "notes.txt".to_owned(),
                bytes,
            }],
        }))
        .await
        .expect("带附件说一句");
    let Response::TaskSay(said) = response else {
        panic!("TaskSay 的响应形状不对");
    };

    wait_until("ThinAgent 请求带着附件", || {
        fixture.router.bodies(true).iter().any(|body| {
            body.contains("看附件")
                && body.contains("notes.txt")
                && body.contains("verbatim read-only copy saved at")
        })
    })
    .await;

    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("接着做的 Attempt");
    let request_id = format!("task-say:{}", said.input_id);
    let message = thin_events(fixture.data(), &attempt)
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => match &message.source {
                model::MessageSource::User {
                    rpc_id: Some(rpc_id),
                } if rpc_id == &request_id => Some(message.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("任务会话里有这条输入");
    assert!(
        message
            .content
            .iter()
            .any(|block| block.as_text() == Some("看附件"))
    );
    let (id, name, file_bytes, path) = message
        .content
        .iter()
        .find_map(|block| match block {
            model::ContentBlock::File {
                attachment_id,
                name,
                bytes,
                path,
            } => Some((attachment_id.clone(), name.clone(), *bytes, path.clone())),
            _ => None,
        })
        .expect("输入里带着附件块");
    assert_eq!(id, attachment_id);
    assert_eq!(name, "notes.txt");
    assert_eq!(file_bytes, bytes);
    assert!(
        path.as_deref()
            .is_some_and(|path| path.contains("attachments/v1/files")),
        "附件走只读副本路径：{path:?}"
    );
}

/// 只有粘贴、没有文字也算一句：ThinAgent 的请求带着粘贴原文，任务会话里是 paste 块。
#[tokio::test]
async fn a_paste_only_task_say_lands_as_a_paste_block() {
    let fixture = Fixture::new().await;
    let task = first_report_fixture(&fixture, 2).await;
    let store = tasks_store(fixture.data());
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("收到粘贴"));
    fixture.router.push_kind(crate::common::text_sse("收到"));

    let response = fixture
        .host
        .dispatch(Call::TaskSay(TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: String::new(),
            pastes: vec!["只有粘贴的一段".to_owned()],
            attachments: Vec::new(),
        }))
        .await
        .expect("只有粘贴也算一句");
    let Response::TaskSay(said) = response else {
        panic!("TaskSay 的响应形状不对");
    };

    wait_until("ThinAgent 请求带着粘贴", || {
        fixture
            .router
            .bodies(true)
            .iter()
            .any(|body| body.contains("只有粘贴的一段"))
    })
    .await;

    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("接着做的 Attempt");
    let request_id = format!("task-say:{}", said.input_id);
    let message = thin_events(fixture.data(), &attempt)
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => match &message.source {
                model::MessageSource::User {
                    rpc_id: Some(rpc_id),
                } if rpc_id == &request_id => Some(message.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("任务会话里有这条输入");
    let paste = message
        .content
        .iter()
        .find_map(|block| match block {
            model::ContentBlock::Text {
                text, paste: true, ..
            } => Some(text.clone()),
            _ => None,
        })
        .expect("输入里是粘贴块，不是普通文字");
    assert_eq!(paste, "只有粘贴的一段");
    assert!(
        message
            .content
            .iter()
            .all(|block| !matches!(block, model::ContentBlock::Text { paste: false, .. })),
        "没有文字块：这句话只有粘贴"
    );
}

/// 同一条输入的 delivery 再跑一次（重试/重领）：request id 判重，会话里只留一条。
#[tokio::test]
async fn a_delivery_retry_does_not_repeat_the_same_input() {
    let fixture = Fixture::new().await;
    let task = fixture.accept(true).await;
    fixture.router.set_thin_delay(Duration::from_millis(2_000));
    fixture
        .router
        .push_thin(crate::common::text_sse("第一步收尾"));
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    let store = tasks_store(fixture.data());
    wait_until("第一轮开跑", || attempt_in_flight(&store, &task.id)).await;

    let said = say_in_task_dialog(&fixture.host, &task, "只该出现一次的话").await;
    wait_until("下一步请求带上这句话", || {
        fixture
            .router
            .bodies(true)
            .last()
            .is_some_and(|body| body.contains("只该出现一次的话"))
    })
    .await;

    let runtime = fixture.host.task_runtime().expect("runtime");
    runtime
        .store()
        .enqueue_effect(
            tasks::EffectPayload::Delivery {
                task_id: task.id.clone(),
            },
            0,
        )
        .unwrap();
    runtime.set_pump_paused(false);
    wait_until("重跑的 delivery 收掉", || {
        !store
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap()
    })
    .await;

    let attempt = store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_attempt()
        .cloned()
        .expect("Attempt");
    let request_id = format!("task-say:{}", said.input_id);
    let events = thin_events(fixture.data(), &attempt);
    // 一条消息在日志里会出现两次（inbox/spliced 插入 + 领取后的 user/message）；
    // 按稳定 message id 去重，重复投递才是两条。
    let mut repeated = HashSet::new();
    for event in &events {
        let messages: Vec<&model::Message> = match &event.kind {
            session::EventKind::UserMessage { message, .. } => vec![message],
            session::EventKind::InboxSpliced { inserted, .. } => inserted.iter().collect(),
            _ => continue,
        };
        for message in messages {
            if message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect::<String>()
                == "只该出现一次的话"
            {
                repeated.insert(message.id.clone());
            }
        }
    }
    assert_eq!(repeated.len(), 1, "同一条输入只进会话一次");
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            session::EventKind::UserMessage { message, .. }
                if matches!(&message.source, model::MessageSource::User { rpc_id: Some(id) } if id == &request_id)
        )),
        "输入用 task-say:<inputId> 做判重键"
    );
}

// ===== ：重启汇报与接续卡 =====

/// 工作树任务报 ready 之后在任务对话里说话：新 Attempt 还在同一个工作树，
/// 不再 git worktree add，也不回原目录写。
#[tokio::test]
async fn saying_again_keeps_the_worktree_task_in_the_same_worktree() {
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
        report_sse(2, "call-report-1", "ready", "第一轮", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));
    let task = fixture.accept_args(false, worktree_task_args(), 2).await;
    let store = tasks_store(fixture.data());
    wait_until("worktree 第一轮 ready", || {
        reported_ready(&store, &task.id)
    })
    .await;
    let before = store.task(&task.id).unwrap().unwrap();
    let first_attempt = before.latest_attempt().cloned().expect("第一个 Attempt");
    let (first_write, first_error) =
        tool_result(&thin_events(fixture.data(), &first_attempt), "call-write-1");
    assert_eq!(first_error, None, "第一个 Attempt 写失败：{first_write}");
    let first = before.latest_attempt().cloned().expect("第一个 Attempt");
    let worktree_cwd = worktree_cwd(&fixture, &task);
    let listed_before = listed_worktrees(fixture.cwd.path());
    let branches_before = git(
        fixture.cwd.path(),
        &["branch", "--list", &format!("aios/{}", task.id)],
    );
    assert_eq!(listed_before.len(), 2, "{listed_before:?}");
    // 第一轮的收尾请求可能还没跑完；等它结束再排第二轮的脚本，免得被收尾请求领走。
    wait_until("第一轮收尾", || {
        session_turn_ended(fixture.data(), &first)
    })
    .await;

    // 停了再说话：第二个 Attempt 继续改同一个文件。
    fixture.router.push_thin_for(
        task.id.as_str(),
        crate::common::tool_use_sse(
            3,
            "call-read-2",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for(
        task.id.as_str(),
        crate::common::tool_use_sse(
            4,
            "call-write-2",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "second\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for(
        task.id.as_str(),
        report_sse(4, "call-report-2", "ready", "第二轮", None),
    );
    fixture
        .router
        .push_thin_for(task.id.as_str(), crate::common::text_sse("done"));
    let said = say_in_task_dialog(&fixture.host, &task, "再改一下").await;
    assert_eq!(said.new_attempt, Some(first.id.0 + 1));
    wait_until("第二个 Attempt 报 ready", || {
        store.task(&task.id).ok().flatten().is_some_and(|task| {
            attempt_count(&task) == 2
                && task.latest_report().map(|report| report.state) == Some(ReportState::Ready)
        })
    })
    .await;

    let after = store.task(&task.id).unwrap().unwrap();
    let second = after.latest_attempt().expect("第二个 Attempt");
    assert_eq!(second.trigger, tasks::AttemptTrigger::UserMessage);
    assert_eq!(
        second.conversation.as_ref(),
        first.conversation.as_ref(),
        "还在原任务会话里"
    );
    wait_until("接着做的请求", || {
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
        "新 Attempt 的 cwd 仍在工作树里"
    );
    let second_events = thin_events(fixture.data(), second);
    let (second_write, second_error) = tool_result(&second_events, "call-write-2");
    assert_eq!(second_error, None, "第二个 Attempt 写失败：{second_write}");
    assert_eq!(
        std::fs::read_to_string(worktree_cwd.join("app.py")).unwrap(),
        "second\n",
        "第二个 Attempt 写进同一个工作树"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("sub").join("app.py")).unwrap(),
        "old\n",
        "原目录文件不改"
    );
    assert_eq!(
        listed_worktrees(fixture.cwd.path()),
        listed_before,
        "不再 git worktree add"
    );
    assert_eq!(
        git(
            fixture.cwd.path(),
            &["branch", "--list", &format!("aios/{}", task.id)]
        ),
        branches_before,
        "工作树分支没有多出来"
    );
}

#[tokio::test]
async fn task_delivery_reads_the_same_record_and_refuses_unknown_tasks() {
    use service::{Response, TaskDeliveryRequest};

    let fixture = Fixture::new().await;
    let store = tasks_store(fixture.data());
    let task = store
        .create_task(
            NewTask {
                project_id: None,
                cwd_source: None,
                title: "查一下".to_owned(),
                requirement: "找答案".to_owned(),
                dod: "有答案".to_owned(),
                cwd: fixture.cwd.path().to_path_buf(),
                conversation: ConversationId::new("kind-delivery"),
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
    let empty = fixture
        .host
        .dispatch(Call::TaskDelivery(TaskDeliveryRequest {
            task_id: task.id.as_str().to_owned(),
        }))
        .await
        .expect("查询成功");
    let Response::TaskDelivery(empty) = empty else {
        panic!("响应类型不对");
    };
    assert!(empty.delivery.is_none(), "还没交过结果");

    let attempt = store.begin_attempt(&task.id, 1_700_000_000_001).unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "查完了，结论是 3 行".to_owned(),
                question: None,
            },
            1_700_000_000_002,
        )
        .unwrap();
    let response = fixture
        .host
        .dispatch(Call::TaskDelivery(TaskDeliveryRequest {
            task_id: task.id.as_str().to_owned(),
        }))
        .await
        .expect("查询成功");
    let Response::TaskDelivery(response) = response else {
        panic!("响应类型不对");
    };
    let delivery = response.delivery.expect("ready 后有交付记录");
    assert_eq!(delivery.attempt, 1);
    assert!(delivery.text().contains("查完了，结论是 3 行"));
    assert!(delivery.text().contains("未证明：没安排门任务"));
    assert_eq!(delivery.word.text, "等你接受");

    let error = fixture
        .host
        .dispatch(Call::TaskDelivery(TaskDeliveryRequest {
            task_id: "not-a-task".to_owned(),
        }))
        .await
        .expect_err("不存在的任务要拒");
    assert!(
        error.to_string().contains("找不到") || error.to_string().contains("任务"),
        "{error}"
    );
}
