//! ask_user_question：模型需要用户决定时提问；执行由 agent loop 接管（T005.10）。

use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolFuture, ToolResult};

/// 模型侧工具定义；真正的问题是 agent loop 发事件、停住等 answer（这里不会被执行）。
#[derive(Debug, Default)]
pub struct AskUserQuestionTool;

impl AskUserQuestionTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "ask_user_question"
    }

    fn description(&self) -> &str {
        "Ask the user a concise question when you need confirmation, a choice, or missing information before proceeding."
    }

    fn parameters(&self) -> &Value {
        static PARAMS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        PARAMS.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "Questions to ask the user before continuing.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "description": "Stable id for this question; echoed in the answer."},
                                "question": {"type": "string", "description": "The specific question to ask the user."},
                                "header": {"type": "string", "description": "Optional short heading."},
                                "options": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {"type": "string"},
                                            "description": {"type": "string"}
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multi_select": {"type": "boolean"}
                            },
                            "required": ["id", "question"]
                        }
                    }
                },
                "required": ["questions"]
            })
        })
    }

    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Exclusive
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            ToolResult {
                call_id: call.id,
                content: "ask_user_question is handled by the agent loop".to_owned(),
                is_error: true,
                error_code: Some("ASK_USER_LOOP_ONLY".to_owned()),
                meta: None,
            }
        })
    }
}
