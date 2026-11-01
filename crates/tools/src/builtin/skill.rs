//! 内置 skill 工具：按名字加载 skill 全文（照 DSH skill/tool-skill）。

use serde_json::{Value, json};

use crate::skills::{is_skill_name, load_skill, render_skill_content};
use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// skill 加载工具。
pub struct SkillTool {
    parameters: Value,
}

impl SkillTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "The exact skill name from the available skills list."}
                },
                "required": ["name"]
            }),
        }
    }
}

impl Default for SkillTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "Load the full instructions for a skill. Call it before acting on a task that names or clearly matches a skill in the session skill catalog."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            match load(&call.arguments, context) {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn load(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = args.as_object() else {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    };
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("invalid name: expected a non-empty string"))?;
    if !is_skill_name(name) {
        return Err(ToolFailure::plain(format!("invalid skill name \"{name}\"")));
    }
    let Some(data_dir) = context.session().data_dir() else {
        return Err(ToolFailure::plain(
            "skill loading requires a session data directory",
        ));
    };
    let roots = context.session().skill_roots();
    let skill = load_skill(&context.cwd, data_dir, &roots, name)?;
    Ok(render_skill_content(&skill))
}
