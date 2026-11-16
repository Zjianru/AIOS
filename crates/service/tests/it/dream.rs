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
        canvas: None,
        result_card: None,
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
            .set_delay(Duration::from_millis(600)),
            sse(&crate::common::text_sse("整理完了")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    wait_startup_check(&root).await;
    send(&root, "dream-once-user", "整理吧").await;
    wait_idle(&root).await;
    // 等 run 行出来（此时 running 标记已立），再点一次「马上整理」。
    let runs = wait_runs(data.path(), 1).await;
    assert_eq!(runs.len(), 1);
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
        canvas: None,
        result_card: None,
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
