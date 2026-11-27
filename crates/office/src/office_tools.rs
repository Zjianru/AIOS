//! Office 插件的两个工具：office_read（Kind/ThinAgent 读）与 office_create（ThinAgent 新建 xlsx）。
//!
//! 工具只做参数解析、路径规则与结果形状；真正的解析/生成在 crate 其它模块。

use std::sync::LazyLock;

use serde_json::{Value, json};
use tools::{
    PluginId, Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult, resolve_read_path,
};

use crate::{CellInput, OfficeFormat, SheetPlan, WorkbookPlan, create_xlsx, read_window};

static READ_PARAMETERS: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Office 文件路径（绝对路径；相对路径按会话工作目录）" },
            "offset": { "type": "integer", "minimum": 0, "description": "从第几个单元开始再读（默认 0）" },
            "limit": { "type": "integer", "minimum": 1, "description": "这次最多读多少个单元（默认 300）" }
        },
        "required": ["path"],
        "additionalProperties": false
    })
});

static CREATE_PARAMETERS: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "要写的 .xlsx 路径（写任务里必须在写入范围内）" },
            "format": { "type": "string", "enum": ["xlsx"], "description": "第一刀只支持 xlsx" },
            "sheets": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "rows": {
                            "type": "array",
                            "items": {
                                "type": "array",
                                "items": {
                                    "anyOf": [
                                        { "type": "string" },
                                        { "type": "number" },
                                        { "type": "boolean" },
                                        { "type": "null" }
                                    ]
                                }
                            }
                        }
                    },
                    "required": ["name", "rows"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["path", "sheets"],
        "additionalProperties": false
    })
});

pub struct OfficeReadTool;

impl Tool for OfficeReadTool {
    fn name(&self) -> &str {
        "office_read"
    }

    fn description(&self) -> &str {
        "读 xlsx/docx/pptx 的结构与文字（xlsx: sheet/单元格/公式；docx: 段落号/表格；pptx: slide/shape 锚点）。只给结构与文字，不承诺版式；超出窗口用 offset/limit 再读。"
    }

    fn parameters(&self) -> &Value {
        &READ_PARAMETERS
    }

    fn plugin(&self) -> Option<PluginId> {
        Some(PluginId::Office)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args: Value = match serde_json::from_str(&call.arguments) {
                Ok(value) => value,
                Err(error) => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("参数不是合法 JSON：{error}")),
                    );
                }
            };
            let Some(path) = args.get("path").and_then(Value::as_str) else {
                return ToolResult::error(call.id, ToolFailure::plain("缺少 path"));
            };
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(crate::DEFAULT_WINDOW as u64) as usize;
            let resolved = match resolve_read_path(context, path) {
                Ok(path) => path,
                Err(error) => return ToolResult::error(call.id, error),
            };
            let Some(format) = OfficeFormat::from_path(&resolved) else {
                return ToolResult::error(
                    call.id,
                    ToolFailure::coded(
                        "OFFICE_UNSUPPORTED",
                        format!("不支持的 Office 格式：{}", resolved.display()),
                    ),
                );
            };
            match read_window(&resolved, offset, limit) {
                Ok(document) => ToolResult::success(
                    call.id,
                    serde_json::to_string(&document).unwrap_or_else(|_| "{}".to_owned()),
                ),
                Err(error) => ToolResult::error(
                    call.id,
                    ToolFailure::coded("OFFICE_READ", error.to_string()),
                ),
            }
        })
    }
}

pub struct OfficeCreateTool;

impl Tool for OfficeCreateTool {
    fn name(&self) -> &str {
        "office_create"
    }

    fn description(&self) -> &str {
        "新建 xlsx（第一刀只支持 xlsx；写任务里使用）。返回已生成路径与「未做视觉自检」。docx/pptx 不生成。"
    }

    fn parameters(&self) -> &Value {
        &CREATE_PARAMETERS
    }

    fn plugin(&self) -> Option<PluginId> {
        Some(PluginId::Office)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args: Value = match serde_json::from_str(&call.arguments) {
                Ok(value) => value,
                Err(error) => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("参数不是合法 JSON：{error}")),
                    );
                }
            };
            if args
                .get("format")
                .and_then(Value::as_str)
                .is_some_and(|format| format != "xlsx")
            {
                return ToolResult::error(
                    call.id,
                    ToolFailure::coded(
                        "OFFICE_ONLY_XLSX",
                        "本版只支持新建 xlsx；docx/pptx 生成不在第一刀".to_owned(),
                    ),
                );
            }
            let Some(path) = args.get("path").and_then(Value::as_str) else {
                return ToolResult::error(call.id, ToolFailure::plain("缺少 path"));
            };
            if !path.to_ascii_lowercase().ends_with(".xlsx") {
                return ToolResult::error(
                    call.id,
                    ToolFailure::coded("OFFICE_ONLY_XLSX", "生成目标必须是 .xlsx".to_owned()),
                );
            }
            let Some(sheets) = args.get("sheets").and_then(Value::as_array) else {
                return ToolResult::error(call.id, ToolFailure::plain("缺少 sheets"));
            };
            let plan = match workbook_plan(sheets) {
                Ok(plan) => plan,
                Err(message) => {
                    return ToolResult::error(call.id, ToolFailure::plain(message));
                }
            };
            let target = if std::path::Path::new(path).is_absolute() {
                std::path::PathBuf::from(path)
            } else {
                context.cwd.join(path)
            };
            if let Err(error) = tools::check_write(context, &target) {
                return ToolResult::error(call.id, error);
            }
            if let Some(parent) = target.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                return ToolResult::error(
                    call.id,
                    ToolFailure::coded("OFFICE_WRITE", format!("建目录失败：{error}")),
                );
            }
            let bytes = match create_xlsx(&plan) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::coded("OFFICE_CREATE", error.to_string()),
                    );
                }
            };
            if let Err(error) = std::fs::write(&target, &bytes) {
                return ToolResult::error(
                    call.id,
                    ToolFailure::coded("OFFICE_WRITE", format!("写文件失败：{error}")),
                );
            }
            let path_text = target.display().to_string();
            ToolResult::success_with_meta(
                call.id,
                format!("已生成 {path_text}（未做视觉自检）"),
                json!({ "diffs": [{ "path": path_text }] }),
            )
        })
    }
}

fn workbook_plan(sheets: &[Value]) -> Result<WorkbookPlan, String> {
    let mut plans = Vec::new();
    for sheet in sheets {
        let name = sheet
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "sheet 缺少 name".to_owned())?;
        let rows = sheet
            .get("rows")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("sheet {name} 缺少 rows"))?;
        let mut plan_rows = Vec::new();
        for row in rows {
            let cells = row
                .as_array()
                .ok_or_else(|| format!("sheet {name} 的 rows 必须是数组"))?;
            let mut plan_cells = Vec::new();
            for cell in cells {
                plan_cells.push(cell_input(cell));
            }
            plan_rows.push(plan_cells);
        }
        plans.push(SheetPlan {
            name: name.to_owned(),
            rows: plan_rows,
        });
    }
    Ok(WorkbookPlan::new(plans))
}

fn cell_input(value: &Value) -> Option<CellInput> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(CellInput::Text {
            value: value.clone(),
        }),
        Value::Number(value) => value.as_f64().map(|value| CellInput::Number { value }),
        Value::Bool(value) => Some(CellInput::Bool { value: *value }),
        _ => Some(CellInput::Text {
            value: value.to_string(),
        }),
    }
}
