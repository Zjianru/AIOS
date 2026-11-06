//! 内置 canvas 工具：Kind 在对话里画一块网页（D58–D60；T015.1 只做新建）。
//!
//! 形状照 Claude Code 的 artifact 工具（WORK/T015/research/src/claude-code-artifacts.md）：
//! 工具自己算出这一版的全文，放进 tool/result.meta.canvas；模型只看到一句「已显示」。
//! 页面从日志的 meta 取 HTML，重开或刷新都从日志重建（日志是唯一事实来源）。

use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 画布工具。T015.1 只做新建（不带 id）；带 id 出新一版和 edits 在 T015.2。
pub struct CanvasTool {
    parameters: Value,
}

impl CanvasTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title shown in the canvas header."
                    },
                    "note": {
                        "type": "string",
                        "description": "What this version shows or changes; shown when the version is collapsed."
                    },
                    "html": {
                        "type": "string",
                        "description": "The whole page content (a body fragment); inline its CSS and JavaScript."
                    }
                },
                "required": ["title", "note", "html"],
                "additionalProperties": false
            }),
        }
    }
}

impl Default for CanvasTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for CanvasTool {
    fn name(&self) -> &str {
        "canvas"
    }

    fn description(&self) -> &str {
        describe()
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { create(&call) })
    }
}

/// 工具说明：照抄 Claude Code artifact 的 When to use / What an artifact is not
/// （claude-code-artifacts.md:18-32），例子换成 AIOS 的，并写明能加载的地址和 aios.send。
fn describe() -> &'static str {
    "Show what you made as a live page inside the conversation instead of describing it in text. The page runs in a sandbox in that turn and follows the AIOS light and dark theme.

When to use
Use the canvas when terminal text is the wrong medium for the result: output that is easier to look at and interact with than to read line by line. Good candidates are a chart, a small calculator with sliders the user can drag, several options laid out side by side, or something that moves. If you would otherwise answer with paragraphs (a savings plan over ten years, a dashboard of numbers the session already pulled), consider a canvas.

Do not use a canvas for an answer one sentence covers, and do not use it to operate AIOS: creating tasks, changing settings, and questions the user has to answer go through cards, never a canvas.

What an artifact is not
A canvas is a capture of work: one self-contained HTML page with no backend. It cannot serve multiple routes, read AIOS data, trigger AIOS actions, keep state across a reload, or start a download. Its only way out is aios.send(text), which puts one line into the user's input box marked as coming from this canvas; the user decides whether to send it.

Loading
Inline your CSS and JavaScript. Scripts and stylesheets may load only from cdnjs.cloudflare.com, cdn.jsdelivr.net/npm/, unpkg.com, cdn.tailwindcss.com and code.jquery.com; fonts only from fonts.googleapis.com and fonts.gstatic.com. Every other external image, script, fetch, XHR and WebSocket is blocked, so embed images as data URIs.

Inside the page
The AIOS design tokens arrive as CSS variables named after the MCP Apps standard: --color-background-primary, --color-background-secondary, --color-background-inverse, --color-text-primary, --color-text-secondary, --color-text-tertiary, --color-text-inverse, --color-text-danger, --color-text-success, --color-border-primary, --color-border-secondary, --font-sans, --font-mono, --font-weight-normal, --font-weight-medium, --font-weight-semibold, --font-text-sm-size, --font-text-md-size, --font-heading-lg-size, --border-radius-sm, --border-radius-md. They follow the user's theme. The body is sized to its content up to 420px; taller content scrolls.

Arguments
title is the header title, note says what this version shows or changes, html is the whole body fragment of the page."
}

/// 新建一块画布：算出这一版的全文放进 meta，给模型只回一句。
fn create(call: &ToolCall) -> ToolResult {
    match parse(&call.arguments) {
        Ok(canvas) => {
            let id = format!("cv_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
            let content = format!("画布「{}」第 1 版已显示（{id}）", canvas.title);
            let meta = json!({
                "canvas": {
                    "id": id,
                    "n": 1,
                    "title": canvas.title,
                    "note": canvas.note,
                    "html": canvas.html,
                }
            });
            ToolResult::success_with_meta(call.id.clone(), content, meta)
        }
        Err(failure) => ToolResult::error(call.id.clone(), failure),
    }
}

/// 一次新建的参数。
struct NewCanvas {
    title: String,
    note: String,
    html: String,
}

fn parse(arguments: &str) -> Result<NewCanvas, ToolFailure> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = value.as_object() else {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    };
    // T015.1 只做新建。带 id / edits 的调用要明确报错，不能悄悄又新建一块。
    for unsupported in ["id", "edits"] {
        if object.contains_key(unsupported) {
            return Err(ToolFailure::plain(format!(
                "invalid '{unsupported}': this build can only show a new canvas"
            )));
        }
    }
    let title = required_text(object.get("title"), "title")?;
    let note = required_text(object.get("note"), "note")?;
    let Some(Value::String(html)) = object.get("html") else {
        return Err(ToolFailure::plain(
            "invalid 'html': expected a string with the page body",
        ));
    };
    Ok(NewCanvas {
        title,
        note,
        html: html.clone(),
    })
}

/// title / note 必须是非空字符串（空白也算空）。
fn required_text(value: Option<&Value>, name: &str) -> Result<String, ToolFailure> {
    match value {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
        _ => Err(ToolFailure::plain(format!(
            "invalid '{name}': expected a non-empty string"
        ))),
    }
}
