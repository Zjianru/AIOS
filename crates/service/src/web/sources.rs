//! `web_search_sources`：搜之前让模型看开着的每一家能做什么、多少钱、当下状态。

use serde_json::Value;
use tools::{
    Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolKey, ToolResult,
};

use super::WebSearchConfig;
use super::providers;
use crate::web_search::enabled_for;

pub(crate) struct WebSourcesTool {
    config: WebSearchConfig,
}

impl WebSourcesTool {
    pub(crate) fn new(config: WebSearchConfig) -> Self {
        Self { config }
    }
}

impl Tool for WebSourcesTool {
    fn name(&self) -> &str {
        "web_search_sources"
    }

    fn description(&self) -> &str {
        "List enabled web search sources before searching: name, capabilities, price per search, and status (ready/missing key/missing host/last failure/cooldown)."
    }

    fn parameters(&self) -> &Value {
        static PARAMETERS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        PARAMETERS.get_or_init(|| serde_json::json!({"type":"object","properties":{}}))
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkillWithKey {
            skills: vec!["web".to_owned()],
            key: ToolKey::WebSearch,
        }
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let rows = match enabled_for(&self.config.data_dir, self.config.identity.as_deref()) {
                Ok(rows) => rows,
                Err(error) => {
                    return ToolResult::error(call.id, ToolFailure::plain(error.to_string()));
                }
            };
            if rows.is_empty() {
                return ToolResult::success(
                    call.id,
                    "No web search source is enabled. Open one in Settings > Model & Connections > Web Search.",
                );
            }
            let mut lines = vec!["Enabled web search sources:".to_owned()];
            for row in rows {
                let capabilities = providers::capability_labels(row.spec.id).join("、");
                let status =
                    crate::web::search::provider_status(row.spec.id, row.missing.as_deref());
                lines.push(format!(
                    "- {} ({}): capabilities={}; price={}; status={}",
                    row.spec.name,
                    row.spec.id,
                    capabilities,
                    providers::price_per_search(row.spec.id),
                    status
                ));
            }
            ToolResult::success(call.id, lines.join("\n"))
        })
    }
}
