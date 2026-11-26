//! T095.4：恢复/总结的服务面（list/apply、区间、过滤与拒绝原因）。

use std::time::Duration;

use service::{
    Call, EventsRequest, Response, RewindAction, RewindApplied, RewindRequest, RewindResponse,
    SendRequest,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{SequenceResponder, open, options, read_root_events, text_sse, wait_turns};

fn sse(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

fn request(id: &str, text: &str) -> SendRequest {
    SendRequest {
        request_id: id.to_owned(),
        content: text.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
    }
}

async fn open_with(server: &MockServer) -> (service::RootHost, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let mut settings = options(data.path());
    settings.model.base_url = server.uri();
    settings.cwd = data.path().to_path_buf();
    let host = open(settings).await;
    (host, data)
}

async fn mount_sequence(server: &MockServer, bodies: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            bodies.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

async fn send(host: &service::RootHost, id: &str, text: &str) {
    host.dispatch(Call::Send(request(id, text)))
        .await
        .expect("send");
}

/// 等宿主真的不在跑（turn/end 落盘后 phase 可能还没切回 Idle）。
async fn wait_idle(host: &service::RootHost) {
    for _ in 0..500 {
        let status = host
            .dispatch(Call::Status(service::StatusRequest {}))
            .await
            .expect("status");
        let Response::Status(status) = status else {
            panic!("status 响应形状不对")
        };
        if !status.running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("宿主一直没空闲");
}

async fn rewind(
    host: &service::RootHost,
    action: RewindAction,
    from_seq: Option<u64>,
) -> Result<RewindResponse, service::ServiceError> {
    match host
        .dispatch(Call::Rewind(RewindRequest { action, from_seq }))
        .await
    {
        Ok(Response::Rewind(response)) => Ok(response),
        Ok(other) => panic!("不是 rewind 响应：{other:?}"),
        Err(error) => Err(error),
    }
}

fn user_seq(events: &[session::Event], text: &str) -> u64 {
    events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. }
                if message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<Vec<_>>()
                    .join("")
                    .contains(text) =>
            {
                Some(event.seq)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("找不到用户消息 {text}"))
}

fn replaced_range(events: &[session::Event]) -> Option<(u64, u64)> {
    events.iter().find_map(|event| match event.surface_op {
        Some(session::SurfaceOp::Replace { start_seq, end_seq }) => Some((start_seq, end_seq)),
        None => None,
    })
}

fn body_user_texts(body: &serde_json::Value) -> Vec<String> {
    let Some(messages) = body["messages"].as_array() else {
        return Vec::new();
    };
    messages
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| {
            message["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|block| block["type"] == "text")
                        .filter_map(|block| block["text"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect()
}

#[tokio::test]
async fn list_is_after_last_checkpoint_and_restore_hides_the_range() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![text_sse("回答一"), text_sse("回答二"), text_sse("回答三")],
    )
    .await;
    let (host, data) = open_with(&server).await;
    send(&host, "req-1", "第一句").await;
    let events = wait_turns(&host, data.path(), 1).await;
    send(&host, "req-2", "第二句").await;
    let events = wait_turns(&host, data.path(), 2).await;
    let first = user_seq(&events, "第一句");
    let second = user_seq(&events, "第二句");
    let before = events.last().map(|event| event.seq).unwrap();

    wait_idle(&host).await;
    let listed = rewind(&host, RewindAction::List, None).await.expect("list");
    assert!(listed.applied.is_none());
    assert_eq!(
        listed
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        vec!["第一句", "第二句"],
        "list 只给最后 checkpoint 之后的用户消息，最新在下",
    );

    wait_idle(&host).await;
    let restored = rewind(&host, RewindAction::Restore, Some(first))
        .await
        .expect("restore");
    let applied = restored.applied.expect("apply 回执");
    assert_eq!(applied.start_seq, first);
    assert_eq!(applied.end_seq, before);
    assert!(applied.event_seq > before);

    let events = read_root_events(data.path());
    assert_eq!(
        session::rewound_ranges(&events),
        vec![(first, before)],
        "rewound_ranges 只取 conversation/rewound 的区间",
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::ConversationRewound { .. })),
        "事件落盘",
    );
    let page = host
        .dispatch(Call::Events(EventsRequest {
            before_seq: None,
            max_messages: Some(500),
            min_messages: None,
            min_turns: None,
        }))
        .await
        .expect("events");
    let Response::Events(page) = page else {
        panic!("events")
    };
    assert!(
        page.events
            .iter()
            .all(|event| !(event.seq >= first && event.seq <= before)),
        "页面分页不返回恢复区间里的历史",
    );
    assert!(
        page.events.iter().any(|event| event.seq > before),
        "恢复事件后的事件还在"
    );

    // 恢复后发新一句：请求里不带被恢复掉的旧话。
    send(&host, "req-3", "第三句").await;
    wait_turns(&host, data.path(), 3).await;
    let requests = server.received_requests().await.unwrap();
    let third: serde_json::Value = serde_json::from_slice(&requests[2].body).unwrap();
    let texts = body_user_texts(&third);
    assert!(texts.iter().any(|text| text.contains("第三句")));
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("第一句") || text.contains("第二句")),
        "恢复掉的段对模型隐藏：{texts:?}",
    );
    let _ = second;
}

#[tokio::test]
async fn summarize_from_writes_one_checkpoint_covering_suffix() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            text_sse("回答一"),
            text_sse("回答二"),
            text_sse("这里是总结正文"),
            text_sse("回答三"),
        ],
    )
    .await;
    let (host, data) = open_with(&server).await;
    send(&host, "req-1", "第一句").await;
    wait_turns(&host, data.path(), 1).await;
    send(&host, "req-2", "第二句").await;
    let events = wait_turns(&host, data.path(), 2).await;
    let first = user_seq(&events, "第一句");
    let before = events.last().map(|event| event.seq).unwrap();

    wait_idle(&host).await;
    let applied: RewindApplied = rewind(&host, RewindAction::SummarizeFrom, Some(first))
        .await
        .expect("summarize_from")
        .applied
        .expect("回执");
    assert!(applied.checkpoint_seq.is_some());
    assert_eq!(applied.start_seq, first);
    assert_eq!(applied.end_seq, before);
    let events = read_root_events(data.path());
    assert!(
        session::rewound_ranges(&events).is_empty(),
        "总结区间不进 rewound_ranges"
    );
    assert_eq!(
        replaced_range(&events),
        Some((first, before)),
        "checkpoint 用同一个 SurfaceOp::Replace 区间",
    );

    send(&host, "req-3", "第三句").await;
    wait_turns(&host, data.path(), 3).await;
    let requests = server.received_requests().await.unwrap();
    let third: serde_json::Value = serde_json::from_slice(&requests[3].body).unwrap();
    let texts = body_user_texts(&third);
    assert!(
        texts.iter().any(|text| text.contains("这里是总结正文")),
        "checkpoint 摘要留在请求里：{texts:?}",
    );
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("第一句") || text.contains("第二句")),
        "被总结的旧话不再整段出现：{texts:?}",
    );
}

#[tokio::test]
async fn summarize_up_to_keeps_selected_message_and_later_history() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            text_sse("回答一"),
            text_sse("回答二"),
            text_sse("前半段总结"),
            text_sse("回答三"),
        ],
    )
    .await;
    let (host, data) = open_with(&server).await;
    send(&host, "req-1", "第一句").await;
    wait_turns(&host, data.path(), 1).await;
    send(&host, "req-2", "第二句").await;
    let events = wait_turns(&host, data.path(), 2).await;
    let second = user_seq(&events, "第二句");

    wait_idle(&host).await;
    let applied = rewind(&host, RewindAction::SummarizeUpTo, Some(second))
        .await
        .expect("summarize_up_to")
        .applied
        .expect("回执");
    assert!(applied.checkpoint_seq.is_some());
    assert!(applied.end_seq < second, "区间到选中消息之前：{applied:?}");
    let events = read_root_events(data.path());
    assert!(session::rewound_ranges(&events).is_empty());

    send(&host, "req-3", "第三句").await;
    wait_turns(&host, data.path(), 3).await;
    let requests = server.received_requests().await.unwrap();
    let third: serde_json::Value = serde_json::from_slice(&requests[3].body).unwrap();
    let texts = body_user_texts(&third);
    assert!(
        texts.iter().any(|text| text.contains("第二句")),
        "选中的那句后面留着：{texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("第一句")),
        "前半段被总结：{texts:?}"
    );
    assert!(texts.iter().any(|text| text.contains("前半段总结")));
}

#[tokio::test]
async fn apply_rejects_non_user_hidden_and_missing_targets() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            text_sse("回答一"),
            text_sse("回答二"),
            text_sse("总结"),
            text_sse("回答三"),
        ],
    )
    .await;
    let (host, data) = open_with(&server).await;
    send(&host, "req-1", "第一句").await;
    wait_turns(&host, data.path(), 1).await;
    send(&host, "req-2", "第二句").await;
    let events = wait_turns(&host, data.path(), 2).await;
    let first = user_seq(&events, "第一句");
    let assistant = events
        .iter()
        .find_map(|event| match event.kind {
            session::EventKind::AssistantMessage { .. } => Some(event.seq),
            _ => None,
        })
        .unwrap();

    wait_idle(&host).await;
    let error = rewind(&host, RewindAction::Restore, Some(assistant))
        .await
        .expect_err("助手消息不能当目标");
    assert!(
        error.to_string().contains("只能从你说过的话开始"),
        "{error}"
    );

    wait_idle(&host).await;
    let error = rewind(&host, RewindAction::Restore, Some(999_999))
        .await
        .expect_err("不存在的目标");
    assert!(
        error.to_string().contains("找不到") || error.to_string().contains("藏起来"),
        "{error}"
    );

    rewind(&host, RewindAction::SummarizeFrom, Some(first))
        .await
        .expect("先总结掉第一段");
    wait_idle(&host).await;
    let error = rewind(&host, RewindAction::Restore, Some(first))
        .await
        .expect_err("已被总结藏掉的目标不能再选");
    assert!(error.to_string().contains("藏起来"), "{error}");
}

#[tokio::test]
async fn apply_rejects_while_a_turn_is_running() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(text_sse("回答一")),
            sse(text_sse("慢回答")).set_delay(Duration::from_secs(5)),
        ]))
        .mount(&server)
        .await;
    let (host, data) = open_with(&server).await;
    send(&host, "req-1", "第一句").await;
    let events = wait_turns(&host, data.path(), 1).await;
    let first = user_seq(&events, "第一句");
    wait_idle(&host).await;
    send(&host, "req-2", "第二句").await;
    for _ in 0..200 {
        let status = host
            .dispatch(Call::Status(service::StatusRequest {}))
            .await
            .expect("status");
        let Response::Status(status) = status else {
            panic!("status")
        };
        if status.running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let error = rewind(&host, RewindAction::Restore, Some(first))
        .await
        .expect_err("跑着时拒绝");
    assert!(error.to_string().contains("正在跑"), "{error}");
}

#[tokio::test]
async fn compact_then_restore_recomputes_derived_and_page_history() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            text_sse("回答一"),
            text_sse("回答二"),
            text_sse("压缩总结正文"),
            text_sse("回答三"),
        ],
    )
    .await;
    let (host, data) = open_with(&server).await;
    send(&host, "req-1", "第一句").await;
    wait_turns(&host, data.path(), 1).await;
    send(&host, "req-2", "第二句").await;
    wait_turns(&host, data.path(), 2).await;
    wait_idle(&host).await;

    // 先压缩：第二句被 checkpoint 替换，菜单只剩 checkpoint 之后的话。
    let compacted = host
        .dispatch(Call::Compact(service::CompactRequest::default()))
        .await
        .expect("compact");
    let Response::Compact(compacted) = compacted else {
        panic!("compact 响应")
    };
    assert!(compacted.compacted.is_some(), "有可压缩历史");
    assert!(
        rewind(&host, RewindAction::List, None)
            .await
            .expect("list")
            .messages
            .is_empty(),
        "压缩后没有 checkpoint 之后的用户消息",
    );

    send(&host, "req-3", "第三句").await;
    let events = wait_turns(&host, data.path(), 3).await;
    wait_idle(&host).await;
    let third = user_seq(&events, "第三句");
    let listed = rewind(&host, RewindAction::List, None).await.expect("list");
    assert_eq!(
        listed
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        vec!["第三句"],
        "菜单只给最后一次压缩之后的话",
    );

    let applied = rewind(&host, RewindAction::Restore, Some(third))
        .await
        .expect("restore")
        .applied
        .expect("回执");
    assert_eq!(applied.start_seq, third);
    let events = read_root_events(data.path());
    assert_eq!(
        session::rewound_ranges(&events).len(),
        1,
        "压缩后恢复再写一条 rewound 区间",
    );
    assert_eq!(session::last_todos(&events), None);

    // 页面历史过滤恢复区间；恢复后发新一句，请求里没有再带第三句。
    let page = host
        .dispatch(Call::Events(EventsRequest {
            before_seq: None,
            max_messages: Some(500),
            min_messages: None,
            min_turns: None,
        }))
        .await
        .expect("events");
    let Response::Events(page) = page else {
        panic!("events")
    };
    assert!(
        page.events
            .iter()
            .all(|event| !(event.seq >= applied.start_seq && event.seq <= applied.end_seq)),
        "页面分页不返回恢复区间里的历史",
    );
}
