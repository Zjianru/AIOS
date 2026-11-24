//! Kind 的 project_create：用户直接说建项目时当场建，回执；重名不建。

use std::sync::Arc;

use super::super::*;
use super::*;

pub(crate) struct ProjectCreateTool {
    store: Option<Arc<tasks::ProjectStore>>,
    runtime: Option<Arc<crate::executor::TaskRuntime>>,
    parameters: Value,
}

impl ProjectCreateTool {
    pub(crate) fn new(
        store: Option<Arc<tasks::ProjectStore>>,
        runtime: Option<Arc<crate::executor::TaskRuntime>>,
    ) -> Self {
        Self {
            store,
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Project name, as the user said it."},
                    "goal": {"type": "string", "description": "One sentence: what this project should become."},
                    "dir": {"type": "string", "description": "Optional default working directory; a ~ path or an absolute path. It does not need to exist yet."}
                },
                "required": ["name", "goal"]
            }),
        }
    }
}

impl Tool for ProjectCreateTool {
    fn name(&self) -> &str {
        "project_create"
    }

    fn description(&self) -> &str {
        "Create a project directly when the user explicitly asks to create one (建个项目/新建项目/创建项目 with a name). Call this immediately in the same turn, before a greeting or any clarifying question, when the user already gave name and goal; do not propose a confirmation card first. Returns the project id, name and default directory. If the name already exists, returns an error and creates nothing. Do not use this for a project you merely inferred; propose create_project on a confirmation card instead."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let Some(name) = string_arg(&args, "name") else {
                return ToolResult::error(call.id, ToolFailure::plain("name 不能为空"));
            };
            let Some(goal) = string_arg(&args, "goal") else {
                return ToolResult::error(call.id, ToolFailure::plain("goal 不能为空"));
            };
            let dir = match args.get("dir") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) => Some(value.as_str()),
                Some(_) => {
                    return ToolResult::error(call.id, ToolFailure::plain("dir 必须是字符串"));
                }
            };
            let Some(store) = self.store.as_ref() else {
                return ToolResult::error(call.id, ToolFailure::plain("项目库没开"));
            };
            match crate::projects::new_project(store, &name, &goal, dir) {
                Ok(project) => {
                    if let Some(runtime) = &self.runtime {
                        runtime.notify_projects_changed();
                    }
                    let dir = project
                        .dir
                        .as_ref()
                        .map(|dir| dir.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "无".to_owned());
                    ToolResult::success(
                        call.id,
                        format!(
                            "已建：项目 {}「{}」。默认目录{}，派任务时预填。",
                            project.id, project.name, dir
                        ),
                    )
                }
                Err(error) => ToolResult::error(call.id, ToolFailure::plain(error.to_string())),
            }
        })
    }
}
