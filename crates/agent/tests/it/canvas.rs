//! T015.2 接缝 2（agent，假模型回放）：画一版，关掉重开，再用 edits 改，成功（清单 4，故事 14、15）。

use std::path::Path;

use agent::{Agent, Delivery, Deps};
use model::{DeepSeek, UserMessage};
use serde_json::{Value, json};
use session::{EventKind, SessionId, SessionStore};
use tools::ToolSet;

use crate::common::{
    FINAL_TEXT_SSE, ScriptedSseServer, SseScript, isolated_config, model_config, test_credential,
    tool_calls_with_inputs_sse, wait_idle,
};

async fn create_agent(data_dir: &Path, cwd: &Path, base_url: &str) -> (Agent, SessionId) {
    let store = SessionStore::create(data_dir, cwd).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        isolated_config(data_dir),
        Deps {
            model: DeepSeek::with_credentials(model_config(base_url), test_credential()).unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    (agent, id)
}

async fn resume_agent(data_dir: &Path, id: &SessionId, base_url: &str) -> Agent {
    let store = SessionStore::open(data_dir, id).unwrap();
    Agent::resume(
        id.clone(),
        isolated_config(data_dir),
        Deps {
            model: DeepSeek::with_credentials(model_config(base_url), test_credential()).unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap()
}

/// 日志里每个画布版本的 meta.canvas（按出现顺序）。
fn canvases(events: &[session::Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolResult {
                meta: Some(meta), ..
            } => meta.get("canvas").cloned(),
            _ => None,
        })
        .collect()
}

/// 一条 tool/result 给模型的文字（只取 text 块）。
fn tool_text(events: &[session::Event], call_id: &str) -> String {
    for event in events {
        if let EventKind::ToolResult { message, .. } = &event.kind
            && message.tool_call_id.as_deref() == Some(call_id)
        {
            return message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
        }
    }
    String::new()
}

#[tokio::test]
async fn resume_rehydrates_the_canvas_and_the_next_edit_succeeds() {
    // 第 1 轮：画一版（工具调用 + 一句回答）。
    let first = ScriptedSseServer::start(vec![
        SseScript {
            frames: tool_calls_with_inputs_sse(&[(
                "call_create",
                "canvas",
                json!({
                    "title": "每月存钱算一算",
                    "note": "第一版",
                    "html": "<div id=\"v\">1</div>",
                }),
            )]),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &first.base_url()).await;
    agent
        .send(UserMessage::text("画一个存钱的画布"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 5).await;
    drop(agent);

    // 关掉重开之前：日志里只有第 1 版，id 从日志取（真实模型也是从工具结果里拿到它）。
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let created = canvases(store.events());
    drop(store);
    assert_eq!(created.len(), 1, "第 1 轮只该有一个画布结果");
    assert_eq!(created[0]["n"], json!(1));
    let canvas_id = created[0]["id"].as_str().unwrap().to_owned();

    // 重开会话，第 2 轮用 edits 改同一块画布（id 来自日志里的那一版）。
    let second = ScriptedSseServer::start(vec![
        SseScript {
            frames: tool_calls_with_inputs_sse(&[(
                "call_edit",
                "canvas",
                json!({
                    "id": canvas_id,
                    "title": "每月存钱算一算",
                    "note": "第二版：换成折线",
                    "edits": [{"old": "<div id=\"v\">1</div>", "new": "<div id=\"v\">2</div>"}],
                }),
            )]),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let resumed = resume_agent(data_dir.path(), &id, &second.base_url()).await;
    resumed
        .send(UserMessage::text("把柱子换成折线"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 5).await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let events = store.events().to_vec();
    drop(store);
    let done = canvases(&events);
    assert_eq!(done.len(), 2, "重开后这次该出第 2 版");
    assert_eq!(done[1]["id"], json!(canvas_id), "改的是同一个画布");
    assert_eq!(done[1]["n"], json!(2));
    assert_eq!(done[1]["html"], json!("<div id=\"v\">2</div>"));
    assert_eq!(
        tool_text(&events, "call_edit"),
        format!("画布「每月存钱算一算」第 2 版已显示（{canvas_id}）")
    );
}
