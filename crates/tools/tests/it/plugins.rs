//! 插件开关过滤与「已关」拦截。

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use model::ToolSchema;
use serde_json::json;
use tempfile::tempdir;
use tools::{
    PluginId, PluginSwitches, Tool, ToolCall, ToolContext, ToolFuture, ToolResult, ToolSet,
    execute_batch,
};

struct FakePluginTool {
    ran: Arc<AtomicBool>,
}

impl Tool for FakePluginTool {
    fn name(&self) -> &str {
        "office_read"
    }

    fn description(&self) -> &str {
        "读 Office"
    }

    fn parameters(&self) -> &serde_json::Value {
        static PARAMS: std::sync::LazyLock<serde_json::Value> =
            std::sync::LazyLock::new(|| json!({"type": "object"}));
        &PARAMS
    }

    fn plugin(&self) -> Option<PluginId> {
        Some(PluginId::Office)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        self.ran.store(true, Ordering::SeqCst);
        Box::pin(async move { ToolResult::success(call.id, "读到了") })
    }
}

struct NormalTool;

impl Tool for NormalTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "读文本"
    }

    fn parameters(&self) -> &serde_json::Value {
        static PARAMS: std::sync::LazyLock<serde_json::Value> =
            std::sync::LazyLock::new(|| json!({"type": "object"}));
        &PARAMS
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { ToolResult::success(call.id, "正文") })
    }
}

#[test]
fn plugin_tool_follows_shared_switches() {
    let switches = PluginSwitches::new();
    let mut set = ToolSet::new(vec![
        Box::new(NormalTool),
        Box::new(FakePluginTool {
            ran: Arc::new(AtomicBool::new(false)),
        }),
    ]);
    set.set_plugin_switches(switches.clone());
    let keys = BTreeSet::new();
    let loaded = BTreeSet::new();
    assert_eq!(
        set.enabled(&loaded, &keys),
        BTreeSet::from(["read".to_owned()])
    );
    switches.enable(PluginId::Office);
    assert_eq!(
        set.enabled(&loaded, &keys),
        BTreeSet::from(["read".to_owned(), "office_read".to_owned()])
    );
    switches.disable(PluginId::Office);
    assert!(!set.enabled(&loaded, &keys).contains("office_read"));
}

#[tokio::test]
async fn closed_plugin_tool_returns_plugin_closed_without_executing() {
    let dir = tempdir().unwrap();
    let switches = PluginSwitches::new();
    let ran = Arc::new(AtomicBool::new(false));
    let mut set = ToolSet::new(vec![Box::new(FakePluginTool {
        ran: Arc::clone(&ran),
    })]);
    set.set_plugin_switches(switches.clone());
    let session = Arc::new(tools::ToolSessionState::new(dir.path(), "s"));
    let context = ToolContext::with_session(dir.path(), session);
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "office_read".to_owned(),
        arguments: "{}".to_owned(),
    };
    let outcome = execute_batch(
        &set,
        &[call],
        1,
        &context,
        &model::CancelToken::new(
            Arc::new(AtomicBool::new(false)),
            Arc::new(tokio::sync::Notify::new()),
        ),
    )
    .await;
    assert_eq!(outcome.results.len(), 1);
    assert_eq!(
        outcome.results[0].error_code.as_deref(),
        Some("PLUGIN_CLOSED")
    );
    assert!(
        outcome.results[0]
            .content
            .ends_with("Office 插件已关，去插件页打开")
    );
    assert!(!ran.load(Ordering::SeqCst), "关着时不能执行");

    switches.enable(PluginId::Office);
    let call = ToolCall {
        id: "call-2".to_owned(),
        name: "office_read".to_owned(),
        arguments: "{}".to_owned(),
    };
    let outcome = execute_batch(
        &set,
        &[call],
        1,
        &context,
        &model::CancelToken::new(
            Arc::new(AtomicBool::new(false)),
            Arc::new(tokio::sync::Notify::new()),
        ),
    )
    .await;
    assert_eq!(outcome.results[0].content, "读到了");
    assert!(ran.load(Ordering::SeqCst));
}

#[test]
fn skill_availability_follows_plugin() {
    let switches = PluginSwitches::new();
    let keys = BTreeSet::new();
    let office = tools::bundled_skills()
        .iter()
        .find(|skill| skill.name == "aios-office")
        .copied()
        .expect("要有 office skill");
    assert!(!office.available(&keys, &switches));
    switches.enable(PluginId::Office);
    assert!(office.available(&keys, &switches));
}

#[allow(dead_code)]
fn schema(name: &str) -> ToolSchema {
    ToolSchema {
        name: name.to_owned(),
        description: String::new(),
        parameters: json!({"type": "object"}),
        defer_loading: false,
    }
}
