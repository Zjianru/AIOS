//! 做梦的 service 端到端：触发条件、许可卡、忙/暂停、同时只一个、只写笔记、撤销。

use std::path::Path;
use std::time::Duration;

use memory::MemoryStore;
use memory::types::{DreamSettingsPatch, WriteContext, WriteOp};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_owned())
}

fn dream_store(data: &Path) -> MemoryStore {
    MemoryStore::open(data.join("memory.sqlite")).unwrap()
}

/// 种子设置：许可、间隔、最小事件数、上次成功/尝试时间。
fn seed_dream(
    data: &Path,
    permit: bool,
    min_events: u64,
    last_success_at: i64,
    last_attempt_at: i64,
) {
    dream_store(data)
        .update_dream_settings(&DreamSettingsPatch {
            permit: Some(permit),
            min_events: Some(min_events),
            last_success_at: Some(last_success_at),
            last_attempt_at: Some(last_attempt_at),
            ..Default::default()
        })
        .unwrap();
}

async fn open_root(data: &Path, cwd: &Path, server: &MockServer) -> service::RootHost {
    let mut options = crate::common::options(data);
    options.cwd = cwd.to_path_buf();
    options.model.base_url = server.uri();
    options.config = agent::AgentConfig {
        instruction_roots: vec![data.join("agents-home")],
        skill_roots: vec![data.join("skills-home")],
        ..agent::AgentConfig::default()
    };
    service::RootHost::open(options, model::DEFAULT_CONTEXT_WINDOW).await
}

async fn send(root: &service::RootHost, id: &str, text: &str) {
    root.dispatch(service::Call::Send(service::SendRequest {
        request_id: id.to_owned(),
        content: text.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        object_refs: Vec::new(),
        quotes: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
    }))
    .await
    .expect("send");
}

/// 启动那一次做梦检查是异步的：等它收尾再发消息，免得和回合结束的触发抢。
async fn wait_startup_check(root: &service::RootHost) {
    for _ in 0..500 {
        if root.dream_checks_settled() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等启动的做梦检查收尾超时");
}

async fn wait_idle(root: &service::RootHost) {
    for _ in 0..1000 {
        let response = root
            .dispatch(service::Call::Status(service::StatusRequest {}))
            .await
            .expect("status");
        let service::Response::Status(status) = response else {
            panic!("expected Status");
        };
        if !status.running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等根会话空闲超时");
}

async fn wait_card(data: &Path, kind: &str) -> session::Event {
    for _ in 0..1000 {
        if let Some(event) = crate::common::read_root_events(data)
            .into_iter()
            .find(|event| {
                matches!(&event.kind, session::EventKind::MemoryCard { kind: got, .. } if got == kind)
            })
        {
            return event;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等不到 {kind} 卡");
}

async fn wait_runs(data: &Path, at_least: usize) -> Vec<memory::types::DreamRun> {
    for _ in 0..1000 {
        let runs = dream_store(data).dream_runs(20).unwrap();
        if runs.len() >= at_least {
            return runs;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等不到做梦记录");
}

fn apply_args(changes: serde_json::Value) -> String {
    serde_json::json!({ "changes": changes }).to_string()
}

/// 一次做梦的假模型脚本：Kind 回一句，做梦 agent 提交改动，再回一句总结。
async fn mount_dream_run(server: &MockServer, changes: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("好")),
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-apply",
                "memory_dream_apply",
                &apply_args(changes),
            )),
            sse(&crate::common::text_sse("整理完了")),
        ]))
        .mount(server)
        .await;
}

#[tokio::test]
async fn dream_runs_when_due_writes_notes_and_card() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 1, 0, 0);
    mount_dream_run(
        &server,
        serde_json::json!([{
            "action": "new_note",
            "title": "先探编码",
            "body": "sources/ 下的 CSV 先探编码再读",
            "reason": "这两轮都栽在编码上"
        }]),
    )
    .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    // 启动那一次检查可能还在跑：等它退出，避免和回合结束的触发撞上。
    wait_startup_check(&root).await;
    send(&root, "dream-due-user", "今天先这样").await;
    wait_idle(&root).await;
    let card = wait_card(data.path(), "dream_done").await;
    let session::EventKind::MemoryCard {
        reference: Some(run_id),
        ..
    } = &card.kind
    else {
        panic!("dream_done 卡要带 run ref");
    };
    let runs = wait_runs(data.path(), 1).await;
    assert_eq!(runs[0].id, *run_id, "卡上的 ref 指回这次 run");
    assert_eq!(runs[0].outcome, "done");
    let store = dream_store(data.path());
    let notes = store.view().unwrap().notes;
    assert_eq!(notes.len(), 1, "做梦写的笔记落库");
    assert!(notes[0].title.contains("先探编码"));
    let settings = store.dream_settings().unwrap();
    assert!(settings.last_success_at > 0, "成功后记下上次成功时间");
    assert!(settings.last_read_seq > 0, "成功后记下读到的 seq");
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests.len() >= 3,
        "Kind 一轮 + 做梦的两次模型请求，实际 {}",
        requests.len()
    );
    root.shutdown().await;
}

#[tokio::test]
async fn dream_rejects_three_invalid_changes_and_keeps_the_previous_version() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = dream_store(data.path());
    store
        .write(
            WriteOp::Note {
                title: "旧笔记".to_owned(),
                summary: "旧摘要".to_owned(),
                body: "旧正文".to_owned(),
                source_kind: "kind".to_owned(),
                source_ref: "file:dream-old".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap();
    seed_dream(data.path(), true, 1, 0, 0);
    let invalid = apply_args(serde_json::json!([{
        "action": "new_note",
        "title": "旧笔记",
        "body": "重名的新正文",
        "reason": "重名"
    }]));
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("好")),
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-invalid-1",
                "memory_dream_apply",
                &invalid,
            )),
            sse(&crate::common::tool_use_sse(
                1,
                "call-dream-invalid-2",
                "memory_dream_apply",
                &invalid,
            )),
            sse(&crate::common::tool_use_sse(
                2,
                "call-dream-invalid-3",
                "memory_dream_apply",
                &invalid,
            )),
            sse(&crate::common::text_sse("不写了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    send(&root, "dream-invalid-user", "今天先这样").await;
    wait_idle(&root).await;
    let mut runs = wait_runs(data.path(), 1).await;
    for _ in 0..1000 {
        if runs[0].outcome != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        runs = dream_store(data.path()).dream_runs(20).unwrap();
    }
    assert_eq!(runs[0].outcome, "failed", "三次不过保留上一版、run failed");
    assert!(
        runs[0].error.contains("3 次"),
        "失败原因写清 3 次：{}",
        runs[0].error
    );
    let notes = dream_store(data.path()).view().unwrap().notes;
    assert_eq!(notes.len(), 1, "没有写回被校验挡下的笔记");
    assert_eq!(notes[0].title, "旧笔记");
    assert_eq!(notes[0].body, "旧正文");
    root.shutdown().await;
}

#[tokio::test]
async fn dream_without_permit_emits_permit_card_and_does_not_run() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 攒够了（min_events 0 = 启动那次就算够）才出许可卡。
    seed_dream(data.path(), false, 0, 0, 0);
    let root = open_root(data.path(), cwd.path(), &server).await;
    let card = wait_card(data.path(), "permission").await;
    let session::EventKind::MemoryCard {
        reference: Some(reference),
        ..
    } = &card.kind
    else {
        panic!("许可卡要带 ref");
    };
    assert_eq!(reference, "dream_permit");
    let settings = dream_store(data.path()).dream_settings().unwrap();
    assert!(settings.permit_card_at > 0, "许可卡只发一次，记下时间");
    assert!(
        dream_store(data.path()).dream_runs(10).unwrap().is_empty(),
        "没许可不跑"
    );
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "没许可不发模型请求"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn dream_skips_when_not_due_or_paused() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 刚成功过：间隔没到。
    let now = dream_store(data.path()).now();
    seed_dream(data.path(), true, 1, now, now);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![sse(
            &crate::common::text_sse("好"),
        )]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    send(&root, "dream-not-due", "还没到时间").await;
    wait_idle(&root).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        dream_store(data.path()).dream_runs(10).unwrap().is_empty(),
        "间隔没到不跑"
    );
    root.shutdown().await;

    // 全局暂停：自动时机让路。
    let server2 = MockServer::start().await;
    let data2 = tempfile::tempdir().unwrap();
    let cwd2 = tempfile::tempdir().unwrap();
    seed_dream(data2.path(), true, 1, 0, 0);
    let mut settings = service::settings::load(data2.path());
    settings.exec.paused = true;
    service::settings::save(data2.path(), &settings).unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![sse(
            &crate::common::text_sse("好"),
        )]))
        .mount(&server2)
        .await;
    let root2 = open_root(data2.path(), cwd2.path(), &server2).await;
    wait_startup_check(&root).await;
    send(&root2, "dream-paused", "暂停时不做梦").await;
    wait_idle(&root2).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        dream_store(data2.path()).dream_runs(10).unwrap().is_empty(),
        "全局暂停时不做梦"
    );
    root2.shutdown().await;
}

#[tokio::test]
async fn dream_runs_only_one_at_a_time() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 1, 0, 0);
    // 做梦的 apply 请求慢一点，让「正在整理」这段可见。
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("好")),
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-slow",
                "memory_dream_apply",
                &apply_args(serde_json::json!([{
                    "action": "new_note",
                    "title": "慢一点的笔记",
                    "body": "慢慢整理",
                    "reason": "同时只跑一个"
                }])),
            ))
            // 窗口放宽：等 run 行出来再到点「马上整理」之间不能挤满 600ms，否则偶发先跑完。
            .set_delay(Duration::from_millis(1_000)),
            sse(&crate::common::text_sse("整理完了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    send(&root, "dream-once-user", "整理吧").await;
    // 直接等 run 行变成 running（不靠 status/时间窗），再点「马上整理」。
    for _ in 0..500 {
        let runs = dream_store(data.path()).dream_runs(5).unwrap();
        if runs.first().is_some_and(|run| run.outcome == "running") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let runs = dream_store(data.path()).dream_runs(5).unwrap();
    assert_eq!(
        runs.first().map(|run| run.outcome.as_str()),
        Some("running"),
        "run 行该是 running"
    );
    assert_eq!(root.dream_now().unwrap(), "正在整理", "正在跑时不起第二个");
    wait_card(data.path(), "dream_done").await;
    assert_eq!(
        dream_store(data.path()).dream_runs(10).unwrap().len(),
        1,
        "同时只跑一个，只有一条记录"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn dream_cannot_touch_about_you_even_through_the_agent() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 1, 0, 0);
    let store = dream_store(data.path());
    let about = store
        .write(
            WriteOp::Remember {
                title: "回答先给结论".to_owned(),
                summary: String::new(),
                body: "先给结论再给理由".to_owned(),
                source_kind: "user".to_owned(),
                source_ref: "seed".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap()
        .item
        .unwrap();
    drop(store);
    mount_dream_run(
        &server,
        serde_json::json!([{
            "action": "correct",
            "memory_id": about.id,
            "title": "改掉关于你",
            "body": "做梦不能改",
            "reason": "越界"
        }]),
    )
    .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    send(&root, "dream-refuse-user", "整理吧").await;
    wait_idle(&root).await;
    wait_card(data.path(), "dream_done").await;
    let store = dream_store(data.path());
    let view = store.view().unwrap();
    assert_eq!(view.about_you.len(), 1);
    assert_eq!(view.about_you[0].title, "回答先给结论", "关于你没被改");
    assert!(
        store
            .dream_changes(&wait_runs(data.path(), 1).await[0].id)
            .unwrap()
            .is_empty()
    );
    // 做梦会话日志里要有「做梦只能改笔记」的拒绝回执。
    let dream_log = dream_session_log(data.path());
    assert!(
        dream_log.contains("做梦只能改笔记"),
        "做梦会话里要留下拒绝原因"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn undo_removes_notes_written_by_the_dream() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 1, 0, 0);
    mount_dream_run(
        &server,
        serde_json::json!([{
            "action": "new_note",
            "title": "要撤销的笔记",
            "body": "这次做完就撤",
            "reason": "验证撤销"
        }]),
    )
    .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    send(&root, "dream-undo-user", "整理吧").await;
    wait_idle(&root).await;
    wait_card(data.path(), "dream_done").await;
    assert_eq!(dream_store(data.path()).view().unwrap().notes.len(), 1);
    let report = root.dream_undo(None).expect("整次撤销");
    assert!(report.applied >= 1);
    assert!(
        dream_store(data.path()).view().unwrap().notes.is_empty(),
        "撤销后笔记没了"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn dream_now_runs_before_due() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 刚成功过、事件数也不够：自动时机不做；用户明说整理才跑。
    let now = dream_store(data.path()).now();
    seed_dream(data.path(), true, 99, now, now);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-now",
                "memory_dream_apply",
                &apply_args(serde_json::json!([{
                    "action": "new_note",
                    "title": "用户让马上整理",
                    "body": "整理一次",
                    "reason": "用户说整理一下"
                }])),
            )),
            sse(&crate::common::text_sse("整理完了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    assert_eq!(root.dream_now().unwrap(), "开始整理");
    wait_card(data.path(), "dream_done").await;
    assert_eq!(dream_store(data.path()).view().unwrap().notes.len(), 1);
    root.shutdown().await;
}

#[tokio::test]
async fn dream_failure_emits_failed_card() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 1, 0, 0);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("好")),
            ResponseTemplate::new(401).set_body_string("{\"error\":\"bad key\"}"),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    send(&root, "dream-fail-user", "今天先这样").await;
    wait_idle(&root).await;
    let card = wait_card(data.path(), "dream_failed").await;
    let session::EventKind::MemoryCard {
        reference: Some(run_id),
        ..
    } = &card.kind
    else {
        panic!("失败卡要带 run ref");
    };
    let run = dream_store(data.path())
        .dream_run(run_id)
        .unwrap()
        .expect("失败也要有 run 记录");
    assert_eq!(run.outcome, "failed");
    assert!(!run.error.is_empty(), "失败原因要写进记录");
    let response = root
        .dispatch(service::Call::MemoryRead(service::MemoryReadRequest {
            scope: None,
            query: None,
            item_id: None,
            refs: vec![run_id.clone()],
        }))
        .await
        .expect("memory-read");
    let service::Response::MemoryRead(read) = response else {
        panic!("expected MemoryRead");
    };
    let card = read.cards.first().expect("失败卡状态");
    assert_eq!(card.state, "failed");
    assert!(
        card.error.as_deref().is_some_and(|error| !error.is_empty()),
        "失败卡要带一行原因：{:?}",
        card.error
    );
    root.shutdown().await;
}

#[tokio::test]
async fn turn_and_relay_checks_coalesce_into_one_run() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 1, 0, 0);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("收到")),
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-coalesce",
                "memory_dream_apply",
                &apply_args(serde_json::json!([{
                    "action": "new_note",
                    "title": "合并成一次",
                    "body": "同一次整理",
                    "reason": "两个时机只跑一次"
                }])),
            )),
            sse(&crate::common::text_sse("整理完了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    // 中继消息和紧随其后的回合结束是两个时机：只该跑一次。
    root.dispatch(service::Call::Send(service::SendRequest {
        request_id: "task-report:task-1:report-1".to_owned(),
        content: r#"{"kind":"task_report","task":{"id":"task-1","title":"查一下归档"},"state":"ready","summary":"查完了"}"#.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        object_refs: Vec::new(),
        quotes: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
    }))
    .await
    .expect("relay send");
    wait_idle(&root).await;
    wait_card(data.path(), "dream_done").await;
    assert_eq!(
        dream_store(data.path()).dream_runs(10).unwrap().len(),
        1,
        "中继和回合结束两个时机合成一次"
    );
    root.shutdown().await;
}

/// 根会话之外的那条会话日志（做梦 agent 的会话）：删掉它之前，读全部内容。
fn dream_session_log(data: &Path) -> String {
    let root: serde_json::Value =
        serde_json::from_slice(&std::fs::read(data.join("kind-root.json")).unwrap()).unwrap();
    let root_id = root["sessionId"].as_str().unwrap_or_default().to_owned();
    let dir = data.join("sessions");
    let mut out = String::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(&(root_id.clone() + ".jsonl")) {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }
    out
}

#[tokio::test]
async fn dream_reads_only_after_the_last_window() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 起步不自动跑：先攒出一条「旧」对话，再把窗口挪到这条之后。
    seed_dream(data.path(), true, 100, 0, 0);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("好")),
            sse(&crate::common::text_sse("好")),
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-conv",
                "memory_conversation_read",
                "{}",
            )),
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-reports",
                "memory_task_reports",
                r#"{"after_ms":0}"#,
            )),
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-search",
                "memory_search",
                r#"{"query":"咖啡"}"#,
            )),
            sse(&crate::common::text_sse("没什么可整理")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    send(&root, "dream-old-user", "旧的一轮：咖啡要双份").await;
    wait_idle(&root).await;
    let old_seq = crate::common::read_root_events(data.path())
        .last()
        .expect("根会话有事件")
        .seq;
    // 窗口从这条旧位置开始：上次成功时间就是现在，旧对话在窗口外。
    let now = dream_store(data.path()).now();
    dream_store(data.path())
        .update_dream_settings(&DreamSettingsPatch {
            last_read_seq: Some(old_seq),
            last_success_at: Some(now),
            last_attempt_at: Some(0),
            min_events: Some(0),
            permit: Some(true),
            ..Default::default()
        })
        .unwrap();
    send(&root, "dream-new-user", "新的一轮：茶要热的").await;
    wait_idle(&root).await;
    // 回合结束的自动检查还在跑（会等根会话空闲再判条件）：等它收尾再手动叫。
    wait_startup_check(&root).await;
    assert_eq!(root.dream_now().unwrap(), "开始整理");
    wait_runs(data.path(), 1).await;
    for _ in 0..500 {
        if root.dream_checks_settled() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let requests = server.received_requests().await.unwrap();
    let dream_bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .filter(|body| body.contains("dream helper"))
        .collect();
    assert!(
        dream_bodies.len() >= 2,
        "做梦至少读了一次再收尾，实际 {} 个请求",
        dream_bodies.len()
    );
    let joined = dream_bodies.join(
        "
",
    );
    assert!(joined.contains("新的一轮：茶要热的"), "窗口内的新话要读到");
    assert!(
        !joined.contains("咖啡要双份"),
        "窗口外的旧话不能读到：{joined}"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn overlapping_checks_still_settle() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // 起步不自动跑，避免和后面人为造的重叠抢脚本。
    seed_dream(data.path(), true, 100, 0, 0);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-apply",
                "memory_dream_apply",
                &apply_args(serde_json::json!([])),
            ))
            .set_delay(Duration::from_millis(400)),
            sse(&crate::common::text_sse("整理完了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    // 手动改成到期，然后叫一次做梦；在它跑的时候再触发一次检查（重叠）。
    dream_store(data.path())
        .update_dream_settings(&DreamSettingsPatch {
            min_events: Some(0),
            last_success_at: Some(0),
            last_attempt_at: Some(0),
            permit: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(root.dream_now().unwrap(), "开始整理");
    for _ in 0..400 {
        if !dream_store(data.path()).dream_runs(10).unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !dream_store(data.path()).dream_runs(10).unwrap().is_empty(),
        "做梦该已经开始"
    );
    assert!(!root.dream_checks_settled(), "做梦在跑，检查还没收尾");
    root.dream_check();
    for _ in 0..1000 {
        if root.dream_checks_settled() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        root.dream_checks_settled(),
        "重叠触发的检查早退也要记 finished，否则 checks_settled 永远 false"
    );
    let runs = wait_runs(data.path(), 1).await;
    assert_eq!(runs.len(), 1, "同时只跑一个");
    wait_startup_check(&root).await;
    root.shutdown().await;
}

#[tokio::test]
async fn undoing_a_correction_and_merge_marks_the_run_revoked() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 100, 0, 0);
    // 两条笔记：一条被改正、一条被合并，都是「已有笔记」的改动（撤销后 change 行还在）。
    let store = dream_store(data.path());
    let corrected = store
        .write(
            WriteOp::Note {
                title: "先探编码".to_owned(),
                summary: String::new(),
                body: "读之前先探编码".to_owned(),
                source_kind: "kind".to_owned(),
                source_ref: "dream-a".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap()
        .item
        .unwrap();
    let merged = store
        .write(
            WriteOp::Note {
                title: "GBK CSV".to_owned(),
                summary: String::new(),
                body: "sources/ 下有 GBK 的 CSV".to_owned(),
                source_kind: "kind".to_owned(),
                source_ref: "dream-b".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap()
        .item
        .unwrap();
    drop(store);
    let changes = serde_json::json!([
        {
            "action": "correct",
            "memory_id": corrected.id,
            "title": "先探编码",
            "body": "读之前先探编码，GBK 也要先转",
            "reason": "两次都栽在编码上"
        },
        {
            "action": "merge",
            "memory_id": merged.id,
            "title": "GBK CSV",
            "body": "sources/ 下有 GBK 的 CSV，先探编码再读",
            "reason": "并到一条里"
        }
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-dream-apply",
                "memory_dream_apply",
                &apply_args(changes),
            )),
            sse(&crate::common::text_sse("整理完了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    dream_store(data.path())
        .update_dream_settings(&DreamSettingsPatch {
            min_events: Some(0),
            last_success_at: Some(0),
            last_attempt_at: Some(0),
            permit: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(root.dream_now().unwrap(), "开始整理");
    let runs = wait_runs(data.path(), 1).await;
    let run_id = runs[0].id.clone();
    for _ in 0..500 {
        if root.dream_checks_settled() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    wait_startup_check(&root).await;

    let read_page = async |run_id: String| {
        let response = root
            .dispatch(service::Call::MemoryRead(service::MemoryReadRequest {
                scope: None,
                query: None,
                item_id: None,
                refs: vec![run_id],
            }))
            .await
            .expect("memory-read");
        let service::Response::MemoryRead(read) = response else {
            panic!("expected MemoryRead");
        };
        read
    };
    let cards = read_page(run_id.clone()).await;
    let card = cards.cards.first().expect("run 卡状态");
    assert_eq!(card.state, "active", "整理完还没撤销");
    let change = &cards.dream.runs[0].changes[0];
    assert!(change.reversible, "撤销前可撤");
    assert!(!change.undone);

    root.dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
        action: service::MemoryActionKind::UndoDream {
            run_id: Some(run_id.clone()),
            memory_id: None,
        },
    }))
    .await
    .expect("undo dream");
    let cards = read_page(run_id.clone()).await;
    let card = cards.cards.first().expect("run 卡状态");
    assert_eq!(card.state, "revoked", "改正/合并撤销后整次该算已撤销");
    let run = &cards.dream.runs[0];
    assert!(
        run.changes
            .iter()
            .all(|change| change.undone && !change.reversible),
        "每一处都要显示已撤销、不可再撤：{:?}",
        run.changes
    );
    let store = dream_store(data.path());
    assert_eq!(
        store.item(&corrected.id).unwrap().unwrap().body,
        "读之前先探编码",
        "改正撤销回原正文"
    );
    assert_eq!(
        store.item(&merged.id).unwrap().unwrap().body,
        "sources/ 下有 GBK 的 CSV",
        "合并撤销回原正文"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn dream_with_no_changes_is_done_not_revoked() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 100, 0, 0);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-empty",
                "memory_dream_apply",
                &apply_args(serde_json::json!([])),
            )),
            sse(&crate::common::text_sse("这次没什么可整理")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    dream_store(data.path())
        .update_dream_settings(&DreamSettingsPatch {
            min_events: Some(0),
            last_success_at: Some(0),
            last_attempt_at: Some(0),
            permit: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(root.dream_now().unwrap(), "开始整理");
    let runs = wait_runs(data.path(), 1).await;
    let run_id = runs[0].id.clone();
    for _ in 0..500 {
        if root.dream_checks_settled() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let response = root
        .dispatch(service::Call::MemoryRead(service::MemoryReadRequest {
            scope: None,
            query: None,
            item_id: None,
            refs: vec![run_id],
        }))
        .await
        .expect("memory-read");
    let service::Response::MemoryRead(read) = response else {
        panic!("expected MemoryRead");
    };
    assert_eq!(
        read.cards.first().map(|card| card.state.as_str()),
        Some("active"),
        "成功但 0 处改动的 run 是整理完了，不是已撤销"
    );
    assert!(read.dream.runs[0].changes.is_empty());
    root.shutdown().await;
}

#[tokio::test]
async fn memory_changed_is_pushed_when_dream_starts_and_ends() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 100, 0, 0);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-push",
                "memory_dream_apply",
                &apply_args(serde_json::json!([])),
            ))
            .set_delay(Duration::from_millis(200)),
            sse(&crate::common::text_sse("整理完了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    let mut events = root.subscribe_events();
    dream_store(data.path())
        .update_dream_settings(&DreamSettingsPatch {
            min_events: Some(0),
            last_success_at: Some(0),
            last_attempt_at: Some(0),
            permit: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(root.dream_now().unwrap(), "开始整理");
    let mut pushed = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(event)) if event.event_type == "memory/changed" => pushed.push(event),
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {}
        }
        if pushed.len() >= 2 {
            break;
        }
    }
    assert!(
        pushed.len() >= 2,
        "做梦开始和结束都要推 memory/changed，实际 {}",
        pushed.len()
    );
    wait_runs(data.path(), 1).await;
    root.shutdown().await;
}

#[tokio::test]
async fn undoing_a_new_note_run_marks_the_card_revoked() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_dream(data.path(), true, 100, 0, 0);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-new-note",
                "memory_dream_apply",
                &apply_args(serde_json::json!([{
                    "action": "new_note",
                    "title": "先探编码",
                    "body": "读之前先探编码",
                    "reason": "旧路径"
                }])),
            )),
            sse(&crate::common::text_sse("记下了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    dream_store(data.path())
        .update_dream_settings(&DreamSettingsPatch {
            min_events: Some(0),
            last_success_at: Some(0),
            last_attempt_at: Some(0),
            permit: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(root.dream_now().unwrap(), "开始整理");
    let runs = wait_runs(data.path(), 1).await;
    let run_id = runs[0].id.clone();
    for _ in 0..500 {
        if root.dream_checks_settled() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let read_page = async |run_id: String| {
        let response = root
            .dispatch(service::Call::MemoryRead(service::MemoryReadRequest {
                scope: None,
                query: None,
                item_id: None,
                refs: vec![run_id],
            }))
            .await
            .expect("memory-read");
        let service::Response::MemoryRead(read) = response else {
            panic!("expected MemoryRead");
        };
        read
    };
    let cards = read_page(run_id.clone()).await;
    assert_eq!(
        cards.cards.first().map(|card| card.state.as_str()),
        Some("active"),
        "新记一条的 run 还没撤销"
    );
    let note_id = cards.dream.runs[0].changes[0].memory_id.clone();
    root.dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
        action: service::MemoryActionKind::UndoDream {
            run_id: Some(run_id.clone()),
            memory_id: None,
        },
    }))
    .await
    .expect("undo dream");
    let cards = read_page(run_id.clone()).await;
    assert_eq!(
        cards.cards.first().map(|card| card.state.as_str()),
        Some("revoked"),
        "只有新记的 run 全撤后也要显示已撤销（change 行会被删掉）"
    );
    assert!(
        dream_store(data.path()).item(&note_id).unwrap().is_none(),
        "新记的笔记撤销后没了"
    );
    root.shutdown().await;
}
