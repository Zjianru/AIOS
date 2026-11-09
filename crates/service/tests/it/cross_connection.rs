//! 跨连接换模型和并发的 status 读：两个窗口的同一条根会话交错。
//!
//! 窗口 A：status 读日志时如果把 Agent 克隆带过 fx 的 await，克隆会一直占着会话写锁；
//! 这时跨连接换模型要重开根会话，拿不到写锁就报 writer-held（Busy）。
//! 窗口 B：重开根会话在放掉旧 Agent 和装上新 Agent 之间，inner 短暂为空；这时 status
//! 会报 RootUnavailable(state=Ok)，页面拿到 409。
//!
//! 复现只用测试夹具把时间窗拉长：A 用一个 FIFO 卡住 fx 缓存读（status 的 await 点），
//! B 用装配层的 append_hook_factory 卡住重开时的 Agent::open。都不给产品加注入口。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{Call, CardActRequest, CardVerb, ModelFactory, RootHost, StatusRequest};
use tempfile::tempdir;

use crate::common::{self, wait_turns};
use crate::scripted_sse::{ScriptedSseServer, SseScript};

const CARD_CALL_ID: &str = "call_card";
const CROSS_CONNECTION: &str = "minimax-highspeed";
const SAME_CONNECTION: &str = "pro";

struct Scenario {
    host: Arc<RootHost>,
    data_dir: PathBuf,
    // 脚本在场景结束前不能停：根会话的连接指向它。
    _server: ScriptedSseServer,
    _dir: tempfile::TempDir,
}

/// 两个连接都在 Keyring 里：deepseek 跟着目录走，minimax-cn 显式勾一个模型。
async fn scenario(append_hook_factory: Option<session::AppendHookFactory>) -> Scenario {
    let dir = tempdir().expect("临时目录");
    let data_dir = dir.path().to_path_buf();
    let identity = data_dir.join("identity.txt");
    let server = ScriptedSseServer::start(vec![
        SseScript {
            frames: common::tool_use_sse(0, "call_skill", "skill", r#"{"name":"aios-settings"}"#),
            hold: false,
        },
        SseScript {
            frames: common::tool_use_sse(
                1,
                CARD_CALL_ID,
                "propose_action",
                &card_arguments().to_string(),
            ),
            hold: false,
        },
        SseScript {
            frames: common::text_sse("选一个。"),
            hold: false,
        },
    ]);
    seed_connections(&data_dir, &identity, &server.base_url());
    // status 会拉汇率；预置未过期的缓存，测试不依赖网络（A 用例稍后把它换成 FIFO）。
    crate::settings::seed_fx_cache(&data_dir);
    let mut options = common::options(&data_dir);
    options.identity = Some(identity.clone());
    options.config.instruction_roots = vec![data_dir.join("agents-home")];
    options.config.skill_roots = vec![data_dir.join("skills-home")];
    options.actions = Arc::new(service::ActionRegistry::settings(
        data_dir.clone(),
        Some(identity.clone()),
    ));
    options.model_factory = Some(factory(data_dir.clone(), identity));
    options.append_hook_factory = append_hook_factory;
    let host = Arc::new(common::open(options).await);
    send(&host, "把这条对话换成另一个模型", 1).await;
    wait_turns(&host, &data_dir, 1).await;
    Scenario {
        host,
        data_dir,
        _server: server,
        _dir: dir,
    }
}

fn card_arguments() -> serde_json::Value {
    serde_json::json!({
        "title": "换成哪个模型",
        "brief": "下一轮开始用你选的那个。",
        "options": [
            {
                "id": SAME_CONNECTION,
                "action": "select_model",
                "label": "V4 Pro",
                "details": { "model": "deepseek-v4-pro" },
            },
            {
                "id": CROSS_CONNECTION,
                "action": "select_model",
                "label": "MiniMax-M2.7-highspeed",
                "details": { "connection": "minimax-cn", "model": "MiniMax-M2.7-highspeed" },
            },
        ],
    })
}

fn seed_connections(data_dir: &Path, identity: &Path, endpoint: &str) {
    let keyring = credentials::Keyring::open(data_dir, Some(identity)).expect("Keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: endpoint.to_owned(),
            api_key: "test-key".to_owned(),
            preset: Some("deepseek".to_owned()),
            // 跟着厂商走：可选模型来自目录。
            models: Some(Vec::new()),
            auth_header: None,
        })
        .expect("保存 deepseek 连接");
    keyring
        .save_connection(credentials::NewConnection {
            id: "minimax-cn".to_owned(),
            name: "MiniMax（国内站）".to_owned(),
            model: "MiniMax-M2.7-highspeed".to_owned(),
            endpoint: endpoint.to_owned(),
            api_key: "test-key-minimax".to_owned(),
            preset: Some("minimax-cn".to_owned()),
            models: Some(vec![credentials::ModelRow {
                id: "MiniMax-M2.7-highspeed".to_owned(),
                name: Some("MiniMax-M2.7-highspeed".to_owned()),
                context_window: Some(1_000_000),
                max_tokens: None,
                price_input: None,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
            }]),
            auth_header: None,
        })
        .expect("保存 minimax 连接");
}

/// 和 cli 的装配同口径：会话选择 > deepseek 连接；换连接时整组取连接默认。
fn factory(data_dir: PathBuf, identity: PathBuf) -> ModelFactory {
    Arc::new(move |wanted: Option<&session::ModelSelection>| {
        let keyring = credentials::Keyring::open(data_dir.clone(), Some(&identity))
            .map_err(|error| error.to_string())?;
        let view = keyring.view();
        let connection_id = wanted
            .map(|selection| selection.connection.clone())
            .unwrap_or_else(|| "deepseek".to_owned());
        let connection = view
            .connections
            .iter()
            .find(|connection| connection.id == connection_id)
            .or_else(|| view.connections.first())
            .cloned()
            .ok_or_else(|| "还没有接模型".to_owned())?;
        let same = wanted.is_some_and(|selection| selection.connection == connection.id);
        let model = wanted
            .filter(|_| same)
            .map(|selection| selection.model.clone())
            .unwrap_or_else(|| connection.model.clone());
        let effort = wanted
            .filter(|_| same)
            .map(|selection| selection.effort_wanted.clone())
            .unwrap_or_else(|| "high".to_owned());
        Ok((
            connection.id.clone(),
            DeepSeekConfig {
                provider: model::catalog::provider_id_for_connection(
                    &connection.preset,
                    &connection.endpoint,
                ),
                base_url: connection.endpoint.clone(),
                model,
                max_tokens: 2048,
                context_window: DEFAULT_CONTEXT_WINDOW,
                effort: Effort::new(effort),
                tool_update: None,
            },
            Some(Arc::new(TestCredential) as Arc<dyn model::CredentialSource>),
        ))
    })
}

/// 测试凭证：签发固定串；屏蔽表由各用例自己配，这里不参与。
struct TestCredential;

impl model::CredentialSource for TestCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok("test-key".to_owned())
    }
}

async fn send(host: &RootHost, text: &str, serial: u32) {
    host.dispatch(Call::Send(service::SendRequest {
        request_id: format!("req-{serial}"),
        content: text.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("send");
}

fn confirm(option: &str) -> Call {
    Call::CardAct(CardActRequest {
        call_id: CARD_CALL_ID.to_owned(),
        verb: CardVerb::Confirm,
        option: Some(option.to_owned()),
    })
}

fn stats_of(response: service::Response) -> service::stats::SessionStats {
    let service::Response::Status(status) = response else {
        panic!("status 必须回 StatusResponse");
    };
    status.stats.expect("status 带 stats")
}

fn decision_phase(response: service::Response) -> (String, String) {
    let value = serde_json::to_value(response).expect("响应 JSON");
    (
        value["decision"]["phase"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        value["decision"]["data"]["summary"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

/// 窗口 B：重开根会话的 take→install 之间，status 要等它装完，不能报 root/unavailable。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_waits_while_root_reopens() {
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let hook: session::AppendHookFactory = {
        let calls = Arc::clone(&calls);
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        Arc::new(move |_id: &session::SessionId| -> session::AppendHook {
            // 第一次打开是启动，第二次就是跨连接换模型的重开。
            if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                entered.notify_one();
                let (lock, cvar) = &*release;
                let mut done = lock.lock().expect("release 锁被毒化");
                while !*done {
                    done = cvar.wait(done).expect("release 等待");
                }
            }
            Arc::new(|_events: &[session::Event]| {})
        })
    };
    let scenario = scenario(Some(hook)).await;
    let card = tokio::spawn({
        let host = Arc::clone(&scenario.host);
        async move { host.dispatch(confirm(CROSS_CONNECTION)).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("跨连接换模型要走到重开根会话");

    // 重开已经放掉旧 Agent、还没装上新 Agent：此时 status 不能看见 inner=None。
    let mut status = tokio::spawn({
        let host = Arc::clone(&scenario.host);
        async move { host.dispatch(Call::Status(StatusRequest::default())).await }
    });
    let early = match tokio::time::timeout(std::time::Duration::from_secs(2), &mut status).await {
        Ok(joined) => Some(joined.expect("status 任务")),
        // 还在等锁 = 修复后的行为。
        Err(_) => None,
    };
    // 先放行，失败也要让重开走完，测试进程不挂着退出。
    *release.0.lock().expect("release 锁被毒化") = true;
    release.1.notify_all();
    let card = card.await.expect("card_act 任务");
    let status = match early {
        Some(outcome) => outcome,
        None => status.await.expect("status 任务"),
    };

    let (phase, summary) = decision_phase(card.expect("并发换模型要成功"));
    assert_eq!(phase, "confirmed", "summary={summary}");
    assert!(
        summary.contains("MiniMax-M2.7-highspeed"),
        "回执要写选了哪个模型：{summary}"
    );
    let stats = stats_of(status.expect("重开期间 status 不能报 root/unavailable"));
    assert_eq!(stats.connection, "minimax-cn");
    assert_eq!(stats.model, "MiniMax-M2.7-highspeed");
}

/// 窗口 A：status 读到 fx 的 await 时不能还拿着会话写锁，否则重开根会话报 writer-held。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_does_not_hold_the_session_writer_across_fx() {
    let scenario = scenario(None).await;
    let fx_path = scenario.data_dir.join(service::fx::FX_FILE);
    std::fs::remove_file(&fx_path).expect("去掉预置的汇率缓存");
    make_fifo(&fx_path);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间")
        .as_secs();
    let body = format!("{{\"rate\":7.11,\"fetchedAt\":{now}}}");

    let (reader_ready_tx, reader_ready_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let writer = {
        let fx_path = fx_path.clone();
        std::thread::spawn(move || {
            use std::io::Write;
            // 写端能打开 = status 已经把 fx 的读端打开、正卡在 await 里。
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .open(&fx_path)
                .expect("打开 fx FIFO 写端");
            reader_ready_tx.send(()).expect("报告 fx 读端已就位");
            release_rx.recv().expect("等测试放行");
            file.write_all(body.as_bytes()).expect("写汇率缓存");
        })
    };

    let status = tokio::spawn({
        let host = Arc::clone(&scenario.host);
        async move { host.dispatch(Call::Status(StatusRequest::default())).await }
    });
    tokio::task::spawn_blocking(move || {
        reader_ready_rx.recv_timeout(std::time::Duration::from_secs(5))
    })
    .await
    .expect("等待 fx 读端")
    .expect("status 要进入 fx 的 await");

    let card = scenario.host.dispatch(confirm(CROSS_CONNECTION)).await;
    release_tx.send(()).expect("放行 fx 读");
    writer.join().expect("fx 写线程");
    let status = status.await.expect("status 任务");

    let (phase, summary) = decision_phase(card.expect("status 占着会话读时换模型也要成功"));
    assert_eq!(phase, "confirmed", "summary={summary}");
    let stats = stats_of(status.expect("status 最终要回 Ok"));
    // 这次 status 的日志快照在换模型之前取；这里只要求它没有因为重开拿不到写锁
    // 而报 Busy，最终能回一份完整的 status。
    assert!(!stats.connection.is_empty());
}

/// 同连接换模型不走重开：并发 status 时不回归。
#[tokio::test]
async fn same_connection_select_still_works_next_to_status() {
    let scenario = scenario(None).await;
    let (card, status) = tokio::join!(
        scenario.host.dispatch(confirm(SAME_CONNECTION)),
        scenario
            .host
            .dispatch(Call::Status(StatusRequest::default())),
    );
    let (phase, summary) = decision_phase(card.expect("同连接换模型要成功"));
    assert_eq!(phase, "confirmed", "summary={summary}");
    status.expect("并发 status 要 Ok");
    let events = crate::common::read_root_events(&scenario.data_dir);
    let selected = events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            session::EventKind::SessionModel { model, .. } => Some(model.clone()),
            _ => None,
        })
        .expect("session/model");
    assert_eq!(selected, "deepseek-v4-pro");
}

fn make_fifo(path: &Path) {
    let output = std::process::Command::new("mkfifo")
        .arg(path)
        .output()
        .expect("运行 mkfifo");
    assert!(
        output.status.success(),
        "mkfifo 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
