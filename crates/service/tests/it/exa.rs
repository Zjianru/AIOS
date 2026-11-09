//! 设置页「联网搜索」：Exa 凭证的四个操作、验证请求，以及自带 web skill/工具随 key 开出。
//!
//! 验证请求打到本地假 Exa（wiremock）：只看 Keyring 里的 key 是否被发出去、结果是否写回；
//! 不读进程环境（端到端另有一条 EXA_API_KEY 的命令证明）。

use std::collections::BTreeSet;
use std::path::Path;

use service::{Call, ExaKeyRequest, ExaKeySaveRequest, Response, RootHost, RootSessionOptions};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{SequenceResponder, open, text_sse, tool_use_sse, wait_turns};

fn sse(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(body)
}

const EXA_ONE: &str = "exa-secret-one-0123456789";
const EXA_TWO: &str = "exa-secret-two-0123456789";

fn keyring(data_dir: &Path) -> credentials::Keyring {
    credentials::Keyring::open(data_dir, Some(&data_dir.join("identity.txt"))).expect("Keyring")
}

async fn dispatch_exa(host: &RootHost, call: Call) -> credentials::ToolKeyView {
    match host.dispatch(call).await.expect("Exa 操作") {
        Response::ExaKey(view)
        | Response::ExaKeySave(view)
        | Response::ExaKeyCheck(view)
        | Response::ExaKeyDelete(view) => view,
        other => panic!("期望 Exa 凭证视图：{other:?}"),
    }
}

fn assert_no_secret(data_dir: &Path, secret: &str) {
    fn walk(dir: &Path, secret: &str) {
        for entry in std::fs::read_dir(dir).expect("读目录") {
            let entry = entry.expect("目录项");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, secret);
                continue;
            }
            let bytes = std::fs::read(&path).expect("读文件");
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()),
                "{} 里出现了 key 正文",
                path.display()
            );
        }
    }
    walk(data_dir, secret);
}

fn exa_options(data_dir: &Path, exa_base_url: String) -> RootSessionOptions {
    let mut options = crate::common::options(data_dir);
    options.exa_base_url = Some(exa_base_url);
    options
}

/// 填、验证、换、删：状态和设置页读到的一致；验证真的带着 Keyring 里那把 key 去问 Exa。
#[tokio::test]
async fn exa_key_round_trip_matches_the_settings_page() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SequenceResponder::new(vec![
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"results": []})),
            ResponseTemplate::new(401).set_body_json(serde_json::json!({"error": "bad key"})),
        ]))
        .mount(&server)
        .await;
    let host = open(exa_options(&data, server.uri())).await;

    let empty = dispatch_exa(&host, Call::ExaKey(ExaKeyRequest {})).await;
    assert!(!empty.key.saved);
    assert_eq!(empty.check, credentials::CheckState::Never);

    let saved = dispatch_exa(
        &host,
        Call::ExaKeySave(ExaKeySaveRequest {
            key: EXA_ONE.to_owned(),
        }),
    )
    .await;
    assert!(saved.key.saved, "存进 Keyring");
    let saved_at = saved.key.saved_at.expect("保存时间");
    assert_eq!(saved.check, credentials::CheckState::Never);
    assert_eq!(
        keyring(&data).exa_key_view().expect("重新打开读到"),
        saved,
        "设置页重新打开读到的和保存时一致"
    );

    let checked = dispatch_exa(&host, Call::ExaKeyCheck(ExaKeyRequest {})).await;
    assert!(
        matches!(checked.check, credentials::CheckState::Ok { .. }),
        "验证通过：{:?}",
        checked.check
    );
    let requests = server.received_requests().await.expect("假 Exa 的请求");
    assert_eq!(requests.len(), 1, "验证发一次最小搜索");
    let authorization = requests[0]
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    assert_eq!(
        authorization,
        Some(format!("Bearer {EXA_ONE}")),
        "授权头来自 Keyring 里的 key"
    );
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("请求体");
    assert_eq!(body["type"], "auto");
    assert_eq!(body["numResults"], 1);
    let keyring_view = keyring(&data).view();
    let issue = keyring_view
        .issues
        .iter()
        .find(|issue| issue.connection == "Exa")
        .expect("签发记录里有 Exa");
    assert_eq!(issue.purpose, credentials::EXA_CHECK_PURPOSE);
    assert_eq!(keyring_view.issued_today, 1);

    let replaced = dispatch_exa(
        &host,
        Call::ExaKeySave(ExaKeySaveRequest {
            key: EXA_TWO.to_owned(),
        }),
    )
    .await;
    assert!(replaced.key.saved);
    assert!(replaced.key.saved_at.expect("时间") >= saved_at);
    assert_eq!(
        replaced.check,
        credentials::CheckState::Never,
        "换 key 重置验证"
    );

    let failed = dispatch_exa(&host, Call::ExaKeyCheck(ExaKeyRequest {})).await;
    match failed.check {
        credentials::CheckState::Fail { reason, .. } => {
            assert_eq!(reason, "Exa 拒绝了这把 key（401）");
        }
        other => panic!("401 要记失败：{other:?}"),
    }
    let requests = server.received_requests().await.expect("假 Exa 的请求");
    assert_eq!(requests.len(), 2);
    let authorization = requests[1]
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    assert_eq!(
        authorization,
        Some(format!("Bearer {EXA_TWO}")),
        "换过以后发新的那把"
    );

    let deleted = dispatch_exa(&host, Call::ExaKeyDelete(ExaKeyRequest {})).await;
    assert!(!deleted.key.saved);
    assert!(!keyring(&data).exa_key_saved(), "删掉以后工具条件读不到");

    assert_no_secret(&data, EXA_ONE);
    assert_no_secret(&data, EXA_TWO);
}

/// 环境变量里有 EXA_API_KEY 也不算数：工具凭证只从 Keyring 读（和模型密钥同一条规矩）。
///
/// 门禁用 `EXA_API_KEY=env-should-not-be-read` 跑这条（测试不改进程环境，同慢测试的写法）；
/// 不带这个变量时直接返回，等于没跑——证据见 handback 的命令。
#[tokio::test]
async fn an_exa_key_in_the_environment_is_not_used() {
    let Some(sentinel) = std::env::var_os("EXA_API_KEY") else {
        return;
    };
    assert_eq!(sentinel.to_string_lossy(), "env-should-not-be-read");
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let model = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(text_sse("（回放）环境变量里的 key 不算数。")))
        .mount(&model)
        .await;
    let mut options = crate::common::options(&data);
    options.model.base_url = model.uri();
    let host = open(options).await;

    let view = dispatch_exa(&host, Call::ExaKey(ExaKeyRequest {})).await;
    assert!(!view.key.saved, "设置页还是「还没有 key」");
    assert!(!keyring(&data).exa_key_saved());

    host.dispatch(Call::Send(service::SendRequest {
        request_id: "env".to_owned(),
        content: "帮我搜一下今天的新闻".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("发消息");
    wait_turns(&host, &data, 1).await;

    let requests = model.received_requests().await.expect("模型请求");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("请求体");
    let names: Vec<&str> = body["tools"]
        .as_array()
        .expect("tools 数组")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(!names.contains(&"web_search") && !names.contains(&"web_fetch"));
    let text = body.to_string();
    assert!(text.contains("(unavailable)"), "目录里仍写清没有 key");
}

/// 没 key：目录里没有 web、写着为什么、工具表里没有网页工具；存了 key 并加载 web：
/// 下一步的工具表里有 web_search/web_fetch，参数是 1–4 条查询 / 一个 URL。
#[tokio::test]
async fn web_skill_and_tools_follow_the_exa_key_in_the_root_session() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let model = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SequenceResponder::new(vec![
            sse(text_sse(
                "（第一轮）我现在不能联网，去设置页填 Exa 的 key。",
            )),
            sse(tool_use_sse(1, "call_web", "skill", r#"{"name":"web"}"#)),
            sse(text_sse("（第二轮）联网工具已经开出来了。")),
        ]))
        .mount(&model)
        .await;
    let mut options = crate::common::options(&data);
    options.model.base_url = model.uri();
    let host = open(options).await;

    host.dispatch(Call::Send(service::SendRequest {
        request_id: "t1".to_owned(),
        content: "帮我搜一下今天的新闻".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("发第一轮");
    wait_turns(&host, &data, 1).await;

    let responses = dispatch_exa(
        &host,
        Call::ExaKeySave(ExaKeySaveRequest {
            key: EXA_ONE.to_owned(),
        }),
    )
    .await;
    assert!(responses.key.saved);

    host.dispatch(Call::Send(service::SendRequest {
        request_id: "t2".to_owned(),
        content: "现在可以搜了吧？".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }))
    .await
    .expect("发第二轮");
    wait_turns(&host, &data, 2).await;

    let bodies: Vec<serde_json::Value> = model
        .received_requests()
        .await
        .expect("模型请求")
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("请求体"))
        .collect();
    assert_eq!(bodies.len(), 3, "第一轮一步 + 第二轮两步");
    let names = |body: &serde_json::Value| -> BTreeSet<String> {
        body["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .map(|tool| tool["name"].as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(
        !names(&bodies[0]).contains("web_search") && !names(&bodies[0]).contains("web_fetch"),
        "没 key 时网页工具不进表"
    );
    let first = bodies[0].to_string();
    assert!(
        first.contains("(unavailable)") && first.contains("设置 › 模型与连接 › 联网搜索"),
        "没 key 时目录里要写清为什么、去哪里填：{first}"
    );
    assert!(
        bodies[1].to_string().contains("- \u{60}web\u{60}:"),
        "存了 key 以后目录里有 web：{}",
        bodies[1]
    );
    assert!(
        !names(&bodies[1]).contains("web_search"),
        "加载发生在这一步里，本步工具表还是旧的"
    );
    let second_step = &bodies[2];
    let tools = names(second_step);
    assert!(tools.contains("web_search"), "加载 web 以后开出 web_search");
    assert!(tools.contains("web_fetch"), "加载 web 以后开出 web_fetch");
    let search = second_step["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "web_search"))
        .expect("web_search 定义");
    assert_eq!(
        search["input_schema"]["properties"]["queries"]["items"]["type"],
        "string"
    );
    assert_eq!(
        search["input_schema"]["properties"]["queries"]["maxItems"],
        4
    );
    let fetch = second_step["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "web_fetch"))
        .expect("web_fetch 定义");
    assert_eq!(fetch["input_schema"]["required"][0], "url");
}
