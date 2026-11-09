//! 确认卡：提议、核对、执行、回执，以及下一次请求前的状态通知。
//!
//! 卡片和具体动作无关：这里有一个只在测试里登记的假动作，产品的换模型动作单独测。

use serde_json::{Value, json};
use service::{
    ActionEnv, ActionFuture, ActionKind, ActionOutcome, ActionRegistry, Call, CardActRequest,
    CardFact, CardVerb, DeliveryMode, RootHost, SendRequest,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

use crate::common::{self, read_root_events, text_sse, tool_use_sse, wait_turns};
use crate::scripted_sse::{ScriptedSseServer, SseScript};

/// 接受时工具结果照抄的原文。
const CARD_RESULT: &str = "Card shown below your answer. Do not restate it. Invite the user to confirm or dismiss the card.";

/// 只在测试里登记的一个动作：details 只有 choice，事实带当前状态，执行会记下选了哪个。
struct FakeAction {
    name: String,
    skill: String,
    state: Mutex<String>,
    describe_error: Mutex<Option<String>>,
    act_error: Mutex<Option<String>>,
    /// 只给并发用例：执行前停一下，把「两个请求同时读到没有决定」的窗口拉大。
    act_delay: Mutex<Option<Duration>>,
    acts: AtomicUsize,
    choices: Mutex<Vec<String>>,
}

impl FakeAction {
    fn new() -> Self {
        Self {
            name: "fake_choice".to_owned(),
            skill: "aios-settings".to_owned(),
            state: Mutex::new("旧状态".to_owned()),
            describe_error: Mutex::new(None),
            act_error: Mutex::new(None),
            act_delay: Mutex::new(None),
            acts: AtomicUsize::new(0),
            choices: Mutex::new(Vec::new()),
        }
    }

    fn acts(&self) -> usize {
        self.acts.load(Ordering::SeqCst)
    }

    fn choices(&self) -> Vec<String> {
        self.choices.lock().expect("选择记录锁").clone()
    }
}

impl ActionKind for FakeAction {
    fn name(&self) -> &str {
        &self.name
    }

    fn skill(&self) -> &str {
        &self.skill
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
        let choice = object
            .get("choice")
            .and_then(Value::as_str)
            .filter(|choice| !choice.is_empty())
            .ok_or_else(|| "details.choice 必须是非空字符串".to_owned())?;
        Ok(json!({ "choice": choice }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        if let Some(reason) = self.describe_error.lock().expect("describe 错误锁").clone() {
            return Err(reason);
        }
        let choice = details
            .get("choice")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let state = self.state.lock().expect("假动作状态锁").clone();
        Ok(vec![CardFact {
            name: "选项".to_owned(),
            value: format!("{choice}（{state}）"),
        }])
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        _host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            if let Some(reason) = self.act_error.lock().expect("act 错误锁").clone() {
                return Err(reason);
            }
            let delay = *self.act_delay.lock().expect("act 延迟锁");
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            let choice = details
                .get("choice")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            self.acts.fetch_add(1, Ordering::SeqCst);
            self.choices
                .lock()
                .expect("选择记录锁")
                .push(choice.clone());
            Ok(ActionOutcome {
                summary: format!("执行了 {choice}"),
                data: json!({ "choice": choice }),
            })
        })
    }
}

struct Scenario {
    host: RootHost,
    data_dir: std::path::PathBuf,
    _server: ScriptedSseServer,
    _dir: tempfile::TempDir,
}

impl Scenario {
    fn host(&self) -> &RootHost {
        &self.host
    }
}

fn registry(fake: &Arc<FakeAction>) -> Arc<ActionRegistry> {
    let mut registry = ActionRegistry::empty();
    registry.register(fake.clone());
    Arc::new(registry)
}

async fn scenario(scripts: Vec<SseScript>, actions: Arc<ActionRegistry>) -> Scenario {
    let dir = tempdir().expect("临时目录");
    let data_dir = dir.path().to_path_buf();
    let server = ScriptedSseServer::start(scripts);
    let mut options = common::options(&data_dir);
    options.model.base_url = server.base_url();
    options.config.instruction_roots = vec![data_dir.join("agents-home")];
    options.config.skill_roots = vec![data_dir.join("skills-home")];
    options.actions = actions;
    let host = common::open(options).await;
    Scenario {
        host,
        data_dir,
        _server: server,
        _dir: dir,
    }
}

/// 设置域测试共用的连接：一个 deepseek 连接，可选模型来自目录。
fn seed_settings_connection(data_dir: &std::path::Path) -> std::path::PathBuf {
    let identity = data_dir.join("identity.txt");
    let keyring = credentials::Keyring::open(data_dir, Some(&identity)).expect("Keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "http://127.0.0.1:1".to_owned(),
            api_key: "test-key".to_owned(),
            preset: Some("deepseek".to_owned()),
            // 跟着厂商走：可选模型来自目录。
            models: Some(Vec::new()),
            auth_header: None,
        })
        .expect("保存连接");
    identity
}

/// 设置域的产品装配：数据目录、连接和动作登记表都用生产入口。
async fn settings_scenario(scripts: Vec<SseScript>) -> Scenario {
    let dir = tempdir().expect("临时目录");
    let data_dir = dir.path().to_path_buf();
    let identity = seed_settings_connection(&data_dir);
    let server = ScriptedSseServer::start(scripts);
    let mut options = common::options(&data_dir);
    options.model.base_url = server.base_url();
    options.config.instruction_roots = vec![data_dir.join("agents-home")];
    options.config.skill_roots = vec![data_dir.join("skills-home")];
    options.identity = Some(identity.clone());
    options.actions = Arc::new(ActionRegistry::settings(data_dir.clone(), Some(identity)));
    let host = common::open(options).await;
    Scenario {
        host,
        data_dir,
        _server: server,
        _dir: dir,
    }
}

fn settings_action(data_dir: &std::path::Path, name: &str) -> Arc<dyn ActionKind> {
    ActionRegistry::settings(data_dir.to_path_buf(), Some(data_dir.join("identity.txt")))
        .get(name)
        .expect("动作已登记")
        .clone()
}

fn action_env<'a>(data_dir: &'a std::path::Path, identity: &'a std::path::Path) -> ActionEnv<'a> {
    ActionEnv {
        data_dir,
        identity: Some(identity),
        events: &[],
    }
}

async fn send(host: &RootHost, text: &str, serial: u32) {
    let response = host
        .dispatch(Call::Send(SendRequest {
            request_id: format!("req-{serial}"),
            content: text.to_owned(),
            delivery: DeliveryMode::FollowUp,
            pastes: Vec::new(),
            attachments: Vec::new(),
            canvas: None,
        }))
        .await
        .expect("send");
    let value = serde_json::to_value(response).expect("send json");
    assert_eq!(value["accepted"], true, "发送没有被受理：{value}");
}

fn skill_script() -> SseScript {
    SseScript {
        frames: tool_use_sse(0, "call_skill", "skill", r#"{"name":"aios-settings"}"#),
        hold: false,
    }
}

fn tool_script(index: usize, call_id: &str, name: &str, arguments: &str) -> SseScript {
    SseScript {
        frames: tool_use_sse(index, call_id, name, arguments),
        hold: false,
    }
}

fn answer_script(text: &str) -> SseScript {
    SseScript {
        frames: text_sse(text),
        hold: false,
    }
}

fn message_text(message: &model::Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            model::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// 按工具名取结果（call id 先对名字）。
fn tool_result(events: &[session::Event], name: &str) -> Option<session::Event> {
    let mut names = std::collections::BTreeMap::new();
    for event in events {
        if let session::EventKind::ToolCall { call_id, name, .. } = &event.kind {
            names.insert(call_id.clone(), name.clone());
        }
    }
    events
        .iter()
        .rev()
        .find(|event| {
            let session::EventKind::ToolResult { message, .. } = &event.kind else {
                return false;
            };
            message
                .tool_call_id
                .as_deref()
                .and_then(|id| names.get(id))
                .is_some_and(|tool| tool == name)
        })
        .cloned()
}

fn card_meta(events: &[session::Event]) -> Value {
    let result = tool_result(events, "propose_action").expect("提议工具结果");
    let session::EventKind::ToolResult { meta, .. } = &result.kind else {
        panic!("不是 tool/result");
    };
    meta.as_ref().expect("提议结果带 meta")["card"].clone()
}

fn card_decisions(events: &[session::Event]) -> Vec<session::CardDecision> {
    events
        .iter()
        .filter_map(|event| session::CardDecision::from_event(&event.kind))
        .collect()
}

fn notice_count(events: &[session::Event]) -> usize {
    const PREAMBLE: &str = "Latest status of action cards from user actions and background work. These are status facts, not a request to start or retry work. Use these receipts instead of the earlier proposal when describing progress.";
    events
        .iter()
        .filter(|event| {
            let session::EventKind::SystemMessage { message, .. } = &event.kind else {
                return false;
            };
            message_text(message).starts_with(PREAMBLE)
        })
        .count()
}

fn request_tools(body: &[u8]) -> Vec<String> {
    let value: Value = serde_json::from_slice(body).expect("请求体 JSON");
    value["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn options_json(choices: &[(&str, &str)]) -> String {
    let options: Vec<Value> = choices
        .iter()
        .map(|(id, choice)| {
            json!({
                "id": id,
                "action": "fake_choice",
                "label": format!("选 {choice}"),
                "description": format!("把状态改成 {choice}"),
                "details": { "choice": choice },
            })
        })
        .collect();
    json!({
        "title": "选一个",
        "brief": "选了才会改。",
        "options": options,
    })
    .to_string()
}

fn mask_options_json(action: &str) -> String {
    json!({
        "title": "屏蔽还是解除",
        "brief": "点了才改，设置页里看得到。",
        "options": [{
            "id": "one",
            "action": action,
            "label": "DeepSeek V4 Pro",
            "description": "只动这一个模型的屏蔽状态",
            "details": { "connection": "deepseek", "model": "deepseek-v4-pro" },
        }],
    })
    .to_string()
}

/// 三个选项：确认后只执行选中的那个；接受时的工具结果照抄原文。
#[tokio::test]
async fn three_options_confirm_only_the_chosen_one() {
    let fake = Arc::new(FakeAction::new());
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲"), ("b", "乙"), ("c", "丙")]),
            ),
            answer_script("选一个告诉我。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "帮我换一个", 1).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 1).await;

    // 工具只在本轮加载 skill 以后出现：第一次请求没有它，第二次有。
    let bodies = scenario._server.request_bodies();
    assert!(
        !request_tools(&bodies[0])
            .iter()
            .any(|name| name == "propose_action")
    );
    assert!(
        request_tools(&bodies[1])
            .iter()
            .any(|name| name == "propose_action")
    );

    let result = tool_result(&events, "propose_action").expect("提议工具结果");
    let session::EventKind::ToolResult { message, .. } = &result.kind else {
        panic!("不是 tool/result");
    };
    assert_ne!(message.is_error, Some(true), "提议应该被接受");
    assert_eq!(message_text(message), CARD_RESULT, "接受时工具结果照抄原文");

    let card = card_meta(&events);
    assert_eq!(card["title"], "选一个");
    assert_eq!(card["options"].as_array().expect("选项数组").len(), 3);

    let response = scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("b".to_owned()),
        }))
        .await
        .expect("确认卡");
    let value = serde_json::to_value(response).expect("确认卡 json");
    assert_eq!(value["decision"]["phase"], "confirmed");
    assert_eq!(fake.acts(), 1, "只执行选中的那一项");
    assert_eq!(fake.choices(), vec!["乙".to_owned()]);

    let events = read_root_events(&scenario.data_dir);
    let decisions = card_decisions(&events);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].phase, session::CardDecisionPhase::Confirmed);
    assert_eq!(decisions[0].option.as_deref(), Some("b"));
    assert_eq!(decisions[0].action.as_deref(), Some("fake_choice"));

    // 重复点击不再执行。
    scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("b".to_owned()),
        }))
        .await
        .expect("第二次确认卡");
    assert_eq!(fake.acts(), 1);
    assert_eq!(
        card_decisions(&read_root_events(&scenario.data_dir)).len(),
        1
    );
}

/// 同一张卡的两个确认并发到达：只执行一次，两边都拿到同一条回执。
#[tokio::test]
async fn concurrent_confirm_on_the_same_card_acts_once() {
    let fake = Arc::new(FakeAction::new());
    *fake.act_delay.lock().expect("act 延迟锁") = Some(Duration::from_millis(100));
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            answer_script("选一个。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "改一下", 1).await;
    wait_turns(scenario.host(), &scenario.data_dir, 1).await;

    let request = || CardActRequest {
        call_id: "call_card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("a".to_owned()),
    };
    let (first, second) = tokio::join!(
        scenario.host().dispatch(Call::CardAct(request())),
        scenario.host().dispatch(Call::CardAct(request())),
    );
    for response in [first.expect("第一个确认"), second.expect("第二个确认")] {
        let value = serde_json::to_value(response).expect("确认卡 json");
        assert_eq!(value["decision"]["phase"], "confirmed", "两边都拿到回执");
        assert_eq!(value["decision"]["option"], "a");
    }
    assert_eq!(fake.acts(), 1, "并发确认只执行一次");
    assert_eq!(fake.choices(), vec!["甲".to_owned()]);
    let decisions = card_decisions(&read_root_events(&scenario.data_dir));
    assert_eq!(decisions.len(), 1, "日志里只有一条决定");
}

/// 六种拒绝：没加载对应 skill、同一轮第二次、动作没登记、details 不合法、
/// 选项 id 重复、describe 失败。
#[tokio::test]
async fn refuses_when_the_skill_was_not_loaded_this_turn() {
    let fake = Arc::new(FakeAction::new());
    let scenario = scenario(
        vec![
            skill_script(),
            answer_script("记住了。"),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            answer_script("这次不提议。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "加载设置 skill", 1).await;
    wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    send(scenario.host(), "不用加载，直接提议", 2).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 2).await;
    let result = tool_result(&events, "propose_action").expect("提议工具结果");
    let session::EventKind::ToolResult { message, .. } = &result.kind else {
        panic!("不是 tool/result");
    };
    assert_eq!(message.is_error, Some(true));
    assert!(message_text(message).contains("Load skill aios-settings first"));
}

#[tokio::test]
async fn refuses_a_second_card_in_the_same_turn() {
    let fake = Arc::new(FakeAction::new());
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            tool_script(
                2,
                "call_card_2",
                "propose_action",
                &options_json(&[("b", "乙")]),
            ),
            answer_script("一张卡。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "出卡", 1).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    let mut results = Vec::new();
    for event in &events {
        if let session::EventKind::ToolResult { message, .. } = &event.kind
            && message
                .tool_call_id
                .as_deref()
                .is_some_and(|id| id.starts_with("call_card"))
        {
            results.push((message_text(message), message.is_error == Some(true)));
        }
    }
    assert_eq!(results.len(), 2);
    assert!(!results[0].1, "第一张卡被接受");
    assert!(results[1].1, "同一轮第二次被拒");
    assert!(results[1].0.contains("One card per turn"));
}

#[tokio::test]
async fn refuses_an_unregistered_action() {
    let fake = Arc::new(FakeAction::new());
    let args = json!({
        "title": "选一个",
        "brief": "说明",
        "options": [{ "id": "a", "action": "nope", "label": "甲", "details": { "choice": "甲" } }],
    })
    .to_string();
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(1, "call_card", "propose_action", &args),
            answer_script("不行。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "出卡", 1).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    let result = tool_result(&events, "propose_action").expect("提议工具结果");
    let session::EventKind::ToolResult { message, .. } = &result.kind else {
        panic!("不是 tool/result");
    };
    assert_eq!(message.is_error, Some(true));
    assert!(message_text(message).contains(r#"Unknown action "nope""#));
}

#[tokio::test]
async fn refuses_invalid_details() {
    let fake = Arc::new(FakeAction::new());
    let args = json!({
        "title": "选一个",
        "brief": "说明",
        "options": [{ "id": "a", "action": "fake_choice", "label": "甲", "details": {} }],
    })
    .to_string();
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(1, "call_card", "propose_action", &args),
            answer_script("不行。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "出卡", 1).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    let result = tool_result(&events, "propose_action").expect("提议工具结果");
    let session::EventKind::ToolResult { message, .. } = &result.kind else {
        panic!("不是 tool/result");
    };
    assert_eq!(message.is_error, Some(true));
    assert!(message_text(message).contains("Invalid details"));
}

#[tokio::test]
async fn refuses_duplicate_option_ids() {
    let fake = Arc::new(FakeAction::new());
    let args = json!({
        "title": "选一个",
        "brief": "说明",
        "options": [
            { "id": "same", "action": "fake_choice", "label": "甲", "details": { "choice": "甲" } },
            { "id": "same", "action": "fake_choice", "label": "乙", "details": { "choice": "乙" } },
        ],
    })
    .to_string();
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(1, "call_card", "propose_action", &args),
            answer_script("不行。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "出卡", 1).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    let result = tool_result(&events, "propose_action").expect("提议工具结果");
    let session::EventKind::ToolResult { message, .. } = &result.kind else {
        panic!("不是 tool/result");
    };
    assert_eq!(message.is_error, Some(true));
    assert!(message_text(message).contains("Option ids must be unique"));
}

#[tokio::test]
async fn refuses_when_describe_fails() {
    let fake = Arc::new(FakeAction::new());
    *fake.describe_error.lock().expect("describe 错误锁") = Some("模型不存在".to_owned());
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            answer_script("不行。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "出卡", 1).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    let result = tool_result(&events, "propose_action").expect("提议工具结果");
    let session::EventKind::ToolResult { message, .. } = &result.kind else {
        panic!("不是 tool/result");
    };
    assert_eq!(message.is_error, Some(true));
    assert!(message_text(message).contains("Cannot offer this option: 模型不存在"));
}

/// 换模型：事实是模型和推理强度的之前之后，未变的那一行不显示；确认以后会话和
/// lastSelection 都变了，日志里有 card/decision confirmed。
#[tokio::test]
async fn select_model_changes_session_and_last_selection() {
    let dir = tempdir().expect("临时目录");
    let data_dir = dir.path().to_path_buf();
    let identity = data_dir.join("identity.txt");
    let keyring = credentials::Keyring::open(&data_dir, Some(&identity)).expect("Keyring");
    keyring
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "http://127.0.0.1:1".to_owned(),
            api_key: "test-key".to_owned(),
            preset: Some("deepseek".to_owned()),
            // 跟着厂商走：可选模型来自目录。
            models: Some(Vec::new()),
            auth_header: None,
        })
        .expect("保存连接");
    let actions = Arc::new(ActionRegistry::settings(
        data_dir.clone(),
        Some(identity.clone()),
    ));
    let args = json!({
        "title": "换成哪个模型",
        "brief": "下一轮开始用你选的那个。",
        "options": [
            { "id": "pro", "action": "select_model", "label": "DeepSeek V4 Pro", "details": { "model": "deepseek-v4-pro" } },
            { "id": "pro-max", "action": "select_model", "label": "V4 Pro · 最强思考", "details": { "model": "deepseek-v4-pro", "effort": "max" } },
            { "id": "flash-max", "action": "select_model", "label": "Flash · 最强思考", "details": { "model": "deepseek-flash", "effort": "max" } },
        ],
    })
    .to_string();
    let server = ScriptedSseServer::start(vec![
        skill_script(),
        tool_script(1, "call_card", "propose_action", &args),
        answer_script("选一个。"),
    ]);
    let mut options = common::options(&data_dir);
    options.model.base_url = server.base_url();
    options.config.instruction_roots = vec![data_dir.join("agents-home")];
    options.config.skill_roots = vec![data_dir.join("skills-home")];
    options.identity = Some(identity);
    options.actions = actions;
    let host = common::open(options).await;
    send(&host, "把这条对话换成更好的模型", 1).await;
    let events = wait_turns(&host, &data_dir, 1).await;

    let card = card_meta(&events);
    let choices = card["options"].as_array().expect("选项数组");
    assert_eq!(choices.len(), 3);
    let facts = |index: usize| -> Vec<Value> {
        choices[index]["facts"]
            .as_array()
            .expect("事实数组")
            .clone()
    };
    assert_eq!(facts(0).len(), 1, "模型变了、强度没变：只显示模型一行");
    assert_eq!(facts(0)[0]["name"], "模型");
    assert!(facts(0)[0]["value"].as_str().expect("事实值").contains("→"));
    assert_eq!(facts(1).len(), 2, "模型和强度都变了：两行");
    assert_eq!(facts(2).len(), 1, "只有强度变了：未变的模型行不显示");
    assert_eq!(facts(2)[0]["name"], "推理强度");

    let response = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("pro-max".to_owned()),
        }))
        .await
        .expect("确认卡");
    let value = serde_json::to_value(response).expect("确认卡 json");
    assert_eq!(value["decision"]["phase"], "confirmed");

    let events = read_root_events(&data_dir);
    let selected = events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            session::EventKind::SessionModel {
                connection,
                model,
                effort_wanted,
            } => Some((connection.clone(), model.clone(), effort_wanted.clone())),
            _ => None,
        })
        .expect("session/model");
    assert_eq!(
        selected,
        (
            "deepseek".to_owned(),
            "deepseek-v4-pro".to_owned(),
            "max".to_owned()
        )
    );
    let settings = service::settings::load(&data_dir);
    assert_eq!(
        settings.last_selection,
        Some(session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-v4-pro".to_owned(),
            effort_wanted: "max".to_owned(),
        })
    );
    let decisions = card_decisions(&events);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].phase, session::CardDecisionPhase::Confirmed);
    assert_eq!(decisions[0].action.as_deref(), Some("select_model"));
}

/// 暂不：什么都不执行，记 dismissed；下一次请求有状态通知、开头照抄原文，只报一次。
#[tokio::test]
async fn dismiss_records_and_notifies_the_model_once() {
    let fake = Arc::new(FakeAction::new());
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            answer_script("选一个。"),
            answer_script("那就不改了。"),
            answer_script("还是不改。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "改一下", 1).await;
    wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    let response = scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card".to_owned(),
            verb: CardVerb::Dismiss,
            option: None,
        }))
        .await
        .expect("暂不");
    let value = serde_json::to_value(response).expect("暂不 json");
    assert_eq!(value["decision"]["phase"], "dismissed");
    assert_eq!(fake.acts(), 0, "暂不什么都不执行");

    let events = read_root_events(&scenario.data_dir);
    let decisions = card_decisions(&events);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].phase, session::CardDecisionPhase::Dismissed);

    send(scenario.host(), "说说为什么", 2).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 2).await;
    assert_eq!(notice_count(&events), 1, "下一次请求前报一次");
    let bodies = scenario._server.request_bodies();
    let preamble = "Latest status of action cards from user actions and background work.";
    assert!(
        String::from_utf8_lossy(&bodies[3]).contains(preamble),
        "第二次请求带状态通知"
    );
    let notice = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::SystemMessage { message, .. }
                if message_text(message).contains(preamble) =>
            {
                Some(message_text(message))
            }
            _ => None,
        })
        .expect("状态通知");
    assert!(notice.contains("call_card"));
    assert!(notice.contains("dismissed"));

    send(scenario.host(), "再说一句", 3).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 3).await;
    assert_eq!(notice_count(&events), 1, "报过一次不再报");
}

/// 点确认前 facts 变了：记 refused，不执行。
#[tokio::test]
async fn changed_facts_refuse_without_executing() {
    let fake = Arc::new(FakeAction::new());
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            answer_script("选一个。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "改一下", 1).await;
    wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    *fake.state.lock().expect("假动作状态锁") = "新状态".to_owned();
    let response = scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("a".to_owned()),
        }))
        .await
        .expect("确认卡");
    let value = serde_json::to_value(response).expect("确认卡 json");
    assert_eq!(value["decision"]["phase"], "refused");
    assert_eq!(fake.acts(), 0);
    let decisions = card_decisions(&read_root_events(&scenario.data_dir));
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].phase, session::CardDecisionPhase::Refused);
}

/// 新卡替换没动过的旧卡：点旧卡不生效，点新卡才执行。
#[tokio::test]
async fn a_new_card_replaces_an_untouched_old_card() {
    let fake = Arc::new(FakeAction::new());
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            answer_script("选一个。"),
            skill_script(),
            tool_script(
                2,
                "call_card_2",
                "propose_action",
                &options_json(&[("b", "乙")]),
            ),
            answer_script("这张是新的。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "第一张", 1).await;
    wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    send(scenario.host(), "第二张", 2).await;
    wait_turns(scenario.host(), &scenario.data_dir, 2).await;

    let response = scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("a".to_owned()),
        }))
        .await
        .expect("旧卡");
    let value = serde_json::to_value(response).expect("旧卡 json");
    assert!(value["decision"].is_null(), "被替换的旧卡点了不生效");
    assert_eq!(fake.acts(), 0);
    assert!(card_decisions(&read_root_events(&scenario.data_dir)).is_empty());

    scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card_2".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("b".to_owned()),
        }))
        .await
        .expect("新卡");
    assert_eq!(fake.acts(), 1);
    assert_eq!(fake.choices(), vec!["乙".to_owned()]);
}

/// 不同卡的请求并发到达：各自拿各自的锁，不互相排队。会话里只有最新一张卡可执行，
/// 旧卡已被替换（返回 None、不执行）；新卡执行一次。
#[tokio::test]
async fn concurrent_acts_on_different_cards_do_not_block_each_other() {
    let fake = Arc::new(FakeAction::new());
    *fake.act_delay.lock().expect("act 延迟锁") = Some(Duration::from_millis(100));
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            answer_script("第一张。"),
            skill_script(),
            tool_script(
                2,
                "call_card_2",
                "propose_action",
                &options_json(&[("b", "乙")]),
            ),
            answer_script("第二张。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "第一张", 1).await;
    wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    send(scenario.host(), "第二张", 2).await;
    wait_turns(scenario.host(), &scenario.data_dir, 2).await;

    let live = CardActRequest {
        call_id: "call_card_2".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("b".to_owned()),
    };
    let replaced = CardActRequest {
        call_id: "call_card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("a".to_owned()),
    };
    let (live_result, replaced_result) = tokio::join!(
        scenario.host().dispatch(Call::CardAct(live)),
        scenario.host().dispatch(Call::CardAct(replaced)),
    );
    let live_value = serde_json::to_value(live_result.expect("新卡")).expect("新卡 json");
    assert_eq!(live_value["decision"]["phase"], "confirmed");
    assert_eq!(live_value["decision"]["option"], "b");
    let replaced_value = serde_json::to_value(replaced_result.expect("旧卡")).expect("旧卡 json");
    assert!(
        replaced_value["decision"].is_null(),
        "被替换的旧卡并发过来也不执行"
    );
    assert_eq!(fake.acts(), 1, "只有新卡执行一次");
    assert_eq!(fake.choices(), vec!["乙".to_owned()]);
    let decisions = card_decisions(&read_root_events(&scenario.data_dir));
    assert_eq!(decisions.len(), 1, "日志里只有新卡那条决定");
    assert_eq!(decisions[0].call_id, "call_card_2");
}

/// act 抛错：记 failed 回执，不重复执行。
#[tokio::test]
async fn act_failure_is_recorded_as_failed() {
    let fake = Arc::new(FakeAction::new());
    *fake.act_error.lock().expect("act 错误锁") = Some("磁盘满了".to_owned());
    let scenario = scenario(
        vec![
            skill_script(),
            tool_script(
                1,
                "call_card",
                "propose_action",
                &options_json(&[("a", "甲")]),
            ),
            answer_script("选一个。"),
        ],
        registry(&fake),
    )
    .await;
    send(scenario.host(), "改一下", 1).await;
    wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    let response = scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("a".to_owned()),
        }))
        .await
        .expect("确认卡");
    let value = serde_json::to_value(response).expect("确认卡 json");
    assert_eq!(value["decision"]["phase"], "failed");
    assert_eq!(value["decision"]["data"]["summary"], "磁盘满了");
    assert_eq!(fake.acts(), 0);
    let decisions = card_decisions(&read_root_events(&scenario.data_dir));
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].phase, session::CardDecisionPhase::Failed);
}

/// 屏蔽、解除屏蔽：事实是服务端读出的之前/之后；确认以后禁用表变了，
/// 日志里有两条 card/decision confirmed。
#[tokio::test]
async fn mask_and_unmask_change_disabled_models_and_record_decisions() {
    let scenario = settings_scenario(vec![
        skill_script(),
        tool_script(
            1,
            "call_card",
            "propose_action",
            &mask_options_json("mask_model"),
        ),
        answer_script("点确认才屏蔽。"),
        skill_script(),
        tool_script(
            2,
            "call_card_2",
            "propose_action",
            &mask_options_json("unmask_model"),
        ),
        answer_script("点确认就解除。"),
    ])
    .await;

    send(scenario.host(), "把 V4 Pro 屏蔽掉", 1).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 1).await;
    let card = card_meta(&events);
    let options = card["options"].as_array().expect("选项数组");
    assert_eq!(options.len(), 1, "屏蔽只有一个选项");
    let facts = options[0]["facts"].as_array().expect("事实数组");
    assert_eq!(facts.len(), 1, "只写这个模型的状态");
    assert_eq!(facts[0]["name"], "屏蔽状态");
    assert_eq!(facts[0]["value"], "可用 → 已屏蔽");

    let response = scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("one".to_owned()),
        }))
        .await
        .expect("确认屏蔽");
    let value = serde_json::to_value(response).expect("确认屏蔽 json");
    assert_eq!(value["decision"]["phase"], "confirmed");
    assert_eq!(
        service::settings::load(&scenario.data_dir).disabled_models,
        vec!["deepseek/deepseek-v4-pro".to_owned()],
        "屏蔽以后禁用表里有这一条的精确规则"
    );

    send(scenario.host(), "再把 V4 Pro 解除屏蔽", 2).await;
    let events = wait_turns(scenario.host(), &scenario.data_dir, 2).await;
    let card = card_meta(&events);
    let options = card["options"].as_array().expect("选项数组");
    assert_eq!(options.len(), 1, "解除也只有一个选项");
    let facts = options[0]["facts"].as_array().expect("事实数组");
    assert_eq!(facts.len(), 1, "只写这个模型的状态");
    assert_eq!(facts[0]["name"], "屏蔽状态");
    assert_eq!(facts[0]["value"], "已屏蔽 → 可用");

    scenario
        .host()
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call_card_2".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("one".to_owned()),
        }))
        .await
        .expect("确认解除");
    assert!(
        service::settings::load(&scenario.data_dir)
            .disabled_models
            .is_empty(),
        "解除以后禁用表空了"
    );

    let decisions = card_decisions(&read_root_events(&scenario.data_dir));
    assert_eq!(decisions.len(), 2);
    assert!(
        decisions
            .iter()
            .all(|decision| decision.phase == session::CardDecisionPhase::Confirmed),
        "两条都是 card/decision confirmed"
    );
    assert_eq!(decisions[0].action.as_deref(), Some("mask_model"));
    assert_eq!(decisions[1].action.as_deref(), Some("unmask_model"));
}

/// 三种拒绝：模型不存在、已经屏蔽、解除时没有单独规则。
#[test]
fn mask_refuses_a_model_that_does_not_exist() {
    let dir = tempdir().expect("临时目录");
    let data_dir = dir.path().to_path_buf();
    let identity = seed_settings_connection(&data_dir);
    let action = settings_action(&data_dir, "mask_model");
    let error = action
        .describe(
            &json!({ "connection": "deepseek", "model": "no-such-model" }),
            &action_env(&data_dir, &identity),
        )
        .expect_err("模型不存在时不提议");
    assert!(error.contains("没有模型"), "{error}");
}

#[test]
fn mask_refuses_a_model_that_is_already_masked() {
    let dir = tempdir().expect("临时目录");
    let data_dir = dir.path().to_path_buf();
    let identity = seed_settings_connection(&data_dir);
    let mut settings = service::settings::load(&data_dir);
    settings.disabled_models = vec!["deepseek/deepseek-v4-pro".to_owned()];
    service::settings::save(&data_dir, &settings).expect("保存屏蔽表");
    let action = settings_action(&data_dir, "mask_model");
    let error = action
        .describe(
            &json!({ "connection": "deepseek", "model": "deepseek-v4-pro" }),
            &action_env(&data_dir, &identity),
        )
        .expect_err("已经屏蔽时不提议");
    assert!(error.contains("已经屏蔽"), "{error}");
}

#[test]
fn unmask_refuses_without_an_exact_rule() {
    let dir = tempdir().expect("临时目录");
    let data_dir = dir.path().to_path_buf();
    let identity = seed_settings_connection(&data_dir);
    let mut settings = service::settings::load(&data_dir);
    settings.disabled_models = vec!["deepseek/*".to_owned()];
    service::settings::save(&data_dir, &settings).expect("保存屏蔽表");
    let action = settings_action(&data_dir, "unmask_model");
    let error = action
        .describe(
            &json!({ "connection": "deepseek", "model": "deepseek-v4-pro" }),
            &action_env(&data_dir, &identity),
        )
        .expect_err("只有通配规则时不提议解除单个模型");
    assert!(error.contains("没有单独屏蔽"), "{error}");
}
