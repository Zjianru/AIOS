//! service 集成测试共用工具：测试凭证、根会话 options、顺序回放。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{RootHost, RootSessionOptions};
use wiremock::{Request, Respond, ResponseTemplate};

/// 测试凭证来源：签发固定 test-key；屏蔽判断走产品同一张表。
pub struct TestCredential {
    data_dir: std::path::PathBuf,
    connection: String,
}

impl TestCredential {
    pub fn new(data_dir: &Path, connection: &str) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            connection: connection.to_owned(),
        }
    }
}

impl model::CredentialSource for TestCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok("test-key".to_owned())
    }

    fn admit(&self, model: &str) -> Result<(), model::ModelError> {
        service::disabled::admit(&self.data_dir, &self.connection, model)
    }
}

pub fn test_credential(data_dir: &Path) -> Arc<dyn model::CredentialSource> {
    Arc::new(TestCredential::new(data_dir, "deepseek"))
}

/// 根会话的默认测试配置；调用方按需覆盖字段。
pub fn options(data_dir: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: data_dir.to_path_buf(),
        model: DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::new("high"),
            tool_update: None,
        },
        config: AgentConfig::default(),
        credentials: Some(test_credential(data_dir)),
        identity: Some(data_dir.join("identity.txt")),
        model_factory: None,
        append_hook_factory: None,
        keyring_changed: None,
        actions: Arc::new(service::ActionRegistry::empty()),
        exa_base_url: None,
        web_listen: None,
        task_runtime: None,
    }
}

pub async fn open(options: RootSessionOptions) -> RootHost {
    RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await
}

/// 一次工具调用的 SSE（参数走 input_json_delta，和真模型一样）。
pub fn tool_use_sse(index: usize, call_id: &str, name: &str, arguments: &str) -> String {
    let arguments = serde_json::json!(arguments).to_string();
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_{index}\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"deepseek-flash\",\"content\":[],\"usage\":{{\"input_tokens\":10,\"output_tokens\":1}}}}}}\n\nevent: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"{call_id}\",\"name\":\"{name}\",\"input\":{{}}}}}}\n\nevent: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{arguments}}}}}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":2}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    )
}

/// 一句最终回答的 SSE（没有工具调用）。
pub fn text_sse(text: &str) -> String {
    let text = serde_json::json!(text).to_string();
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_text\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"deepseek-flash\",\"content\":[],\"usage\":{{\"input_tokens\":50,\"output_tokens\":1}}}}}}\n\nevent: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\nevent: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{text}}}}}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":3}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    )
}

/// 只读打开根会话日志（决定之类的事件在空闲时也会追加，用这个读回）。
pub fn read_root_events(data_dir: &Path) -> Vec<session::Event> {
    #[derive(serde::Deserialize)]
    struct Record {
        #[serde(rename = "sessionId")]
        session_id: String,
    }
    let bytes = std::fs::read(data_dir.join(service::ROOT_STATE_FILE)).expect("根状态文件");
    let record: Record = serde_json::from_slice(&bytes).expect("根状态 JSON");
    let id = session::SessionId::parse(&record.session_id).expect("会话 id");
    session::SessionStore::open_read_only(data_dir, &id)
        .expect("只读打开根会话")
        .events()
        .to_vec()
}

/// 等这一条根会话日志里至少收尾了 turns 轮，返回当时的全部事件。
pub async fn wait_turns(host: &RootHost, data_dir: &Path, turns: usize) -> Vec<session::Event> {
    for _ in 0..600 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let status = host
            .dispatch(service::Call::Status(service::StatusRequest {}))
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
        let ended = events
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::TurnEnd { .. }))
            .count();
        if ended >= turns {
            return events;
        }
    }
    panic!("等不到第 {turns} 轮收尾");
}

/// 按脚本顺序回放（第 N 次请求拿第 N 个响应；耗尽回 500，不静默复用）。
pub struct SequenceResponder {
    responses: Mutex<VecDeque<ResponseTemplate>>,
}

impl SequenceResponder {
    pub fn new(responses: Vec<ResponseTemplate>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

impl Respond for SequenceResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| ResponseTemplate::new(500))
    }
}
