//! 派任务与驱动：等一轮、kind_turn、受理、停下、说法、确认卡、读任务页头。

use super::*;

pub(super) async fn wait_until(label: &str, mut check: impl FnMut() -> bool) {
    // 续跑的下一张结果卡可能晚几秒才 Steer 进 Kind；给足 18s。
    for _ in 0..900 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等不到：{label}");
}

/// 页面「停下这一轮」的调用；测试只关心响应。
pub(super) async fn interrupt_task(host: &RootHost, task: &Task) -> service::TaskInterruptResponse {
    let response = host
        .dispatch(Call::TaskInterrupt(TaskInterruptRequest {
            task_id: task.id.as_str().to_owned(),
        }))
        .await
        .expect("停下这一轮");
    let Response::TaskInterrupt(interrupt) = response else {
        panic!("TaskInterrupt 的响应形状不对");
    };
    interrupt
}

/// 页面和 Kind 的 task_stop 都走这一个调用；测试只关心响应。
pub(super) async fn stop_task(
    fixture: &Fixture,
    task: &Task,
    mode: StopMode,
) -> service::TaskStopResponse {
    let response = fixture
        .host
        .dispatch(Call::TaskStop(TaskStopRequest {
            task_id: task.id.as_str().to_owned(),
            mode,
        }))
        .await
        .expect("停止");
    let Response::TaskStop(stop) = response else {
        panic!("TaskStop 的响应形状不对");
    };
    stop
}

/// 走一轮 Kind：先把这一轮的回复排好，再发消息，等 turn 收尾。
pub(super) async fn kind_turn(
    host: &RootHost,
    router: &Router,
    data: &Path,
    request_id: &str,
    text: &str,
    turns: usize,
    scripts: Vec<String>,
) {
    for script in scripts {
        router.push_kind(script);
    }
    host.dispatch(Call::Send(send(host, request_id, text)))
        .await
        .expect("发送");
    crate::common::wait_turns(host, data, turns).await;
}

pub(super) async fn confirm_pause_card(host: &RootHost, call_id: &str) -> Value {
    let response = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: call_id.to_owned(),
            verb: CardVerb::Confirm,
            option: Some("one".to_owned()),

            note: None,
            report_id: None,
        }))
        .await
        .expect("确认暂停卡");
    serde_json::to_value(response).expect("确认暂停卡 json")["decision"].clone()
}

/// 受理一个任务并返回它。
pub(super) async fn accept_task(
    host: &RootHost,
    router: &Router,
    data: &Path,
    request_id: &str,
    call_id: &str,
    turns: usize,
    title: &str,
) -> Task {
    let args = serde_json::json!({
        "title": title,
        "quote": "派一个任务",
        "requirement": "找答案",
        "dod": "有答案",
    })
    .to_string();
    kind_turn(
        host,
        router,
        data,
        request_id,
        "派一个任务",
        turns,
        vec![
            crate::common::tool_use_sse(0, call_id, "task_accept", &args),
            crate::common::text_sse("已派出"),
        ],
    )
    .await;
    let store = tasks_store(data);
    store
        .task_ids()
        .expect("任务 id")
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(|task| task.title == title)
        .expect("刚受理的任务")
}

/// 派一个任务并等它出现在任务库；不等 Kind 那一轮收尾。
///
/// ThinAgent 轮次结束会合成等待报告、把 relay Steer 回 Kind，Kind 那一轮可能因此多一步；
/// 只断言任务自己的 Attempt 和效果时不该被这个时序拖住。
pub(super) async fn dispatch_task(
    host: &RootHost,
    router: &Router,
    data: &Path,
    request_id: &str,
    call_id: &str,
    title: &str,
) -> Task {
    let args = serde_json::json!({
        "title": title,
        "quote": "派一个任务",
        "requirement": "找答案",
        "dod": "有答案",
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        0,
        call_id,
        "task_accept",
        &args,
    ));
    router.push_kind(crate::common::text_sse("已派出"));
    host.dispatch(Call::Send(send(host, request_id, "派一个任务")))
        .await
        .expect("发送");
    let store = tasks_store(data);
    wait_until("任务出现在任务库", || {
        store
            .task_ids()
            .map(|ids| {
                ids.iter().any(|id| {
                    store
                        .task(id)
                        .ok()
                        .flatten()
                        .is_some_and(|task| task.title == title)
                })
            })
            .unwrap_or(false)
    })
    .await;
    store
        .task_ids()
        .expect("任务 id")
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(|task| task.title == title)
        .expect("刚受理的任务")
}

/// 在任务对话里说一句（和页面同一个调用）。
pub(super) async fn say_in_task_dialog(
    host: &RootHost,
    task: &Task,
    text: &str,
) -> service::TaskSayResponse {
    let response = host
        .dispatch(Call::TaskSay(service::TaskSayRequest {
            task_id: task.id.as_str().to_owned(),
            text: text.to_owned(),
            pastes: Vec::new(),
            attachments: Vec::new(),
            object_refs: Vec::new(),
        }))
        .await
        .expect("任务对话里说一句");
    let Response::TaskSay(said) = response else {
        panic!("TaskSay 的响应形状不对");
    };
    said
}

/// 受理一个任务并跑完第一轮 ready 报告；说话类测试的起点。
pub(super) async fn first_report_fixture(fixture: &Fixture, kind_spares: usize) -> Task {
    fixture.router.push_thin_for_title(
        "查一下",
        report_sse(2, "call-report", "ready", "第一轮答案", None),
    );
    fixture
        .router
        .push_thin_for_title("查一下", crate::common::text_sse("第一轮收尾"));
    let task = fixture.accept_with_kind_replies(false, kind_spares).await;
    let store = tasks_store(fixture.data());
    wait_until("第一轮报 ready", || reported_ready(&store, &task.id)).await;
    wait_until("Kind 收到第一轮 relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;
    // 给 relay 叫醒的 Kind 轮一点时间收尾，之后说话时 Kind 应当是空闲的。
    tokio::time::sleep(Duration::from_millis(200)).await;
    task
}

/// 点确认卡上的一个选项；返回写下的决定。
pub(super) async fn confirm_card_option(host: &RootHost, call_id: &str, option: &str) -> Value {
    let response = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: call_id.to_owned(),
            verb: CardVerb::Confirm,
            option: Some(option.to_owned()),

            note: None,
            report_id: None,
        }))
        .await
        .expect("确认接续卡");
    serde_json::to_value(response).expect("确认卡 json")["decision"].clone()
}

/// 点卡片上的「暂不」。
pub(super) async fn dismiss_card(host: &RootHost, call_id: &str) -> Value {
    let response = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: call_id.to_owned(),
            verb: CardVerb::Dismiss,
            option: None,

            note: None,
            report_id: None,
        }))
        .await
        .expect("暂不接续");
    serde_json::to_value(response).expect("暂不 json")["decision"].clone()
}

/// TaskRead 的头部事实（只读；测试里读页头用）。
pub(super) async fn task_read_header(
    host: &RootHost,
    task_id: &tasks::TaskId,
) -> service::TaskReadHeader {
    let request = TaskReadRequest {
        task_id: task_id.as_str().to_owned(),
        before_seq: None,
    };
    match host
        .dispatch(Call::TaskRead(request))
        .await
        .expect("task-read")
    {
        Response::TaskRead(read) => read.task,
        _ => panic!("task-read 响应"),
    }
}
