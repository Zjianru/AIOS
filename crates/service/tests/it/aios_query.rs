//! T014.2：`aios_query` 的五个主题——结果和服务里对应的读一致，输出里没有 key 正文。

use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use service::{
    Call, ConnectionSaveRequest, DeliveryMode, ModelSettingsRequest, RootHost, SendRequest,
    StatusRequest,
};
use tempfile::tempdir;

use crate::scripted_sse::{ScriptedSseServer, SseScript};

const TEST_KEY: &str = "sk-t014-query-0123456789";

/// 一次 tool_use 的 SSE（参数走 input_json_delta，和真模型一样）。
fn tool_use_sse(index: usize, call_id: &str, name: &str, arguments: &str) -> String {
    let arguments = serde_json::json!(arguments).to_string();
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_{index}\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"deepseek-flash\",\"content\":[],\"usage\":{{\"input_tokens\":10,\"output_tokens\":1}}}}}}\n\nevent: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"{call_id}\",\"name\":\"{name}\",\"input\":{{}}}}}}\n\nevent: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{arguments}}}}}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":2}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    )
}

const FINAL_TEXT_SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_final\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"deepseek-flash\",\"content\":[],\"usage\":{\"input_tokens\":50,\"output_tokens\":1}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"查到了。\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

async fn host(dir: &Path, base_url: &str) -> RootHost {
    let mut options = crate::common::options(dir);
    options.model.base_url = base_url.to_owned();
    // 隔离用户的 ~/.agents：指令和 skill 根都落在数据目录里，技能目录只来自程序自带的那份。
    options.config.instruction_roots = vec![dir.join("agents-home")];
    options.config.skill_roots = vec![dir.join("skills-home")];
    crate::common::open(options).await
}

fn send_request(content: &str) -> SendRequest {
    SendRequest {
        request_id: "req-t014-2".to_owned(),
        content: content.to_owned(),
        delivery: DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
    }
}

/// 等到这一轮收尾，返回当时的会话日志。
async fn wait_turn_events(host: &RootHost, data_dir: &Path) -> Vec<session::Event> {
    for _ in 0..400 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let status = host
            .dispatch(Call::Status(StatusRequest {}))
            .await
            .expect("status");
        let json = serde_json::to_value(&status).expect("status json");
        let Some(session_id) = json["object"]["sessionId"].as_str() else {
            continue;
        };
        let Ok(id) = session::SessionId::parse(session_id) else {
            continue;
        };
        let Ok(store) = session::SessionStore::open_read_only(data_dir, &id) else {
            continue;
        };
        let events = store.events().to_vec();
        if events
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::TurnEnd { .. }))
        {
            return events;
        }
    }
    panic!("等不到 turn/end");
}

/// 按调用顺序取 `aios_query` 的工具结果正文。
fn query_results(events: &[session::Event]) -> Vec<String> {
    let mut names: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for event in events {
        if let session::EventKind::ToolCall { call_id, name, .. } = &event.kind {
            names.insert(call_id.clone(), name.clone());
        }
    }
    let mut results = Vec::new();
    for event in events {
        if let session::EventKind::ToolResult { message, .. } = &event.kind {
            let Some(call_id) = message.tool_call_id.as_deref() else {
                continue;
            };
            if names.get(call_id).map(String::as_str) != Some("aios_query") {
                continue;
            }
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            results.push(text);
        }
    }
    results
}

/// 测试决定 6：五个主题各一条，结果和服务里对应的读一致，输出里没有 key 正文。
#[tokio::test]
async fn aios_query_topics_match_service_reads() {
    let server = ScriptedSseServer::start(vec![
        SseScript {
            frames: tool_use_sse(0, "call_skill", "skill", r#"{"name":"aios-settings"}"#),
            hold: false,
        },
        SseScript {
            frames: tool_use_sse(1, "call_settings", "aios_query", r#"{"topic":"settings"}"#),
            hold: false,
        },
        SseScript {
            frames: tool_use_sse(
                2,
                "call_connections",
                "aios_query",
                r#"{"topic":"connections"}"#,
            ),
            hold: false,
        },
        SseScript {
            frames: tool_use_sse(3, "call_models", "aios_query", r#"{"topic":"models"}"#),
            hold: false,
        },
        SseScript {
            frames: tool_use_sse(4, "call_usage", "aios_query", r#"{"topic":"usage"}"#),
            hold: false,
        },
        SseScript {
            frames: tool_use_sse(5, "call_product", "aios_query", r#"{"topic":"product"}"#),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let data = tempdir().unwrap();
    let host = host(data.path(), &server.base_url()).await;
    let saved = host
        .dispatch(Call::ConnectionSave(ConnectionSaveRequest {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: server.base_url(),
            api_key: TEST_KEY.to_owned(),
            ..Default::default()
        }))
        .await;
    assert!(saved.is_ok(), "存连接应成功：{saved:?}");

    host.dispatch(Call::Send(send_request(
        "现在用的什么模型，这条对话花了多少？",
    )))
    .await
    .expect("send");
    let events = wait_turn_events(&host, data.path()).await;
    let results = query_results(&events);
    assert_eq!(results.len(), 5, "五个主题各一条：{results:?}");
    for text in &results {
        assert!(
            !text.contains(TEST_KEY),
            "查询输出里不能出现 key 正文：{text}"
        );
    }
    let topic = |index: usize| -> Value {
        serde_json::from_str(&results[index])
            .unwrap_or_else(|error| panic!("查询结果是 JSON（{error}）：{results:?}"))
    };

    // 设置：这条对话的模型、强度和币种，和 status 一致。
    let status = host
        .dispatch(Call::Status(StatusRequest {}))
        .await
        .expect("status");
    let status = serde_json::to_value(&status).expect("status json");
    let settings = topic(0);
    assert_eq!(
        settings["session"]["model"], status["stats"]["model"],
        "设置里的模型和 status 一致：{settings}"
    );
    assert_eq!(
        settings["session"]["connection"], status["stats"]["connection"],
        "设置里的连接和 status 一致：{settings}"
    );
    assert_eq!(
        settings["currency"], status["currency"],
        "币种和 status 一致：{settings}"
    );

    // 连接：和模型设置页的 Keyring 视图一致；有 key、没验证过。
    let model_settings = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("model-settings");
    let model_settings = serde_json::to_value(&model_settings).expect("model-settings json");
    let connections = topic(1);
    let rows = connections["connections"].as_array().expect("connections");
    assert_eq!(rows.len(), 1, "只有一个连接：{connections}");
    assert_eq!(
        rows[0]["id"],
        model_settings["settings"]["connections"][0]["id"]
    );
    assert_eq!(
        rows[0]["name"],
        model_settings["settings"]["connections"][0]["name"]
    );
    assert_eq!(
        rows[0]["hasKey"], model_settings["settings"]["connections"][0]["key_saved"],
        "有没有 key 和设置页一致"
    );
    assert_eq!(
        rows[0]["lastCheck"],
        model_settings["settings"]["connections"][0]["check"]
    );
    assert!(rows[0]["hasKey"].as_bool().unwrap_or(false), "存了 key");

    // 模型：屏蔽表和设置页一致，模型行来自连接的可选模型。
    let models = topic(2);
    assert_eq!(
        models["disabled"], model_settings["disabledModels"],
        "屏蔽表和设置页一致：{models}"
    );
    let listed = models["connections"][0]["models"]
        .as_array()
        .expect("模型行");
    assert!(
        listed
            .iter()
            .any(|row| row["id"] == "deepseek-flash" && row["disabled"] == serde_json::json!(false)),
        "连接的可选模型在表里且没被屏蔽：{models}"
    );

    // 用量：查的时候是第 5 步，前 5 次请求各报 input 10 / output 2（回放脚本里的数）。
    // 期望值手写自回放脚本，不从实现算出来。
    let usage = topic(3);
    assert_eq!(usage["turns"], 1, "这一轮：{usage}");
    assert_eq!(usage["steps"], 5, "第 5 步查的，5 条 step/start：{usage}");
    assert_eq!(usage["inputTokens"], 50, "5 × 10：{usage}");
    assert_eq!(usage["outputTokens"], 10, "5 × 2：{usage}");
    assert_eq!(
        usage["connection"], status["stats"]["connection"],
        "用量的连接和 status 一致：{usage}"
    );
    assert_eq!(
        usage["model"], status["stats"]["model"],
        "用量的模型和 status 一致：{usage}"
    );
    assert_eq!(
        usage["costUsd"].is_null(),
        usage["unpriced"].as_bool().unwrap_or(false),
        "没有价格时不给花费数字：{usage}"
    );

    // 产品：说清 AIOS 能做什么、不能做什么。
    let product = topic(4);
    assert_eq!(product["product"], "AIOS");
    assert!(
        product["can"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "要列能做什么：{product}"
    );
    assert!(
        product["cannot"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "要列不能做什么：{product}"
    );
}
