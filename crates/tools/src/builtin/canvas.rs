//! 内置 canvas 工具：Kind 在对话里画一块网页（D58–D60；T015.2 给同一个画布出新一版）。
//!
//! 形状照 Claude Code 的 artifact 工具（WORK/T015/research/src/claude-code-artifacts.md）：
//! 工具自己算出这一版的全文，放进 tool/result.meta.canvas；模型只看到一句「已显示」。
//! 画布当前每一版的全文从会话日志折叠（Agent 建/续接时灌进工具会话状态），所以重开会话也能接着改。

use serde_json::{Value, json};
use session::CanvasState;

use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 参数里的一个局部替换。
struct Edit {
    old: String,
    new: String,
}

/// 一次调用要做的事（参数校验完之后）。
enum Request {
    New {
        title: String,
        note: String,
        html: String,
    },
    Rewrite {
        id: String,
        title: String,
        note: String,
        html: String,
    },
    Edit {
        id: String,
        title: String,
        note: String,
        edits: Vec<Edit>,
    },
}

/// 画布工具：不带 id 新建；带 id 整份重写或局部改，出新一版。
pub struct CanvasTool {
    parameters: Value,
}

impl CanvasTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Existing canvas id (from an earlier result). Omit to start a new canvas."
                    },
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
                        "description": "The whole page content (a body fragment); inline its CSS and JavaScript. Use this to create a canvas or rewrite an existing one."
                    },
                    "edits": {
                        "type": "array",
                        "description": "Local replacements for an existing canvas. Each old must appear exactly once; if any edit does not match, nothing changes.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old": {
                                    "type": "string",
                                    "description": "Exact text that must appear exactly once in the current page."
                                },
                                "new": {
                                    "type": "string",
                                    "description": "Replacement text."
                                }
                            },
                            "required": ["old", "new"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["title", "note"],
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
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { show(&call, context) })
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
title is the header title, note says what this version shows or changes. To create a canvas, omit id and pass html. To revise a canvas, pass its id (the result of an earlier call tells you the id and version) and exactly one of: html (rewrite the whole page) or edits (a list of {old, new}). Each old must appear exactly once and edits apply in order; if any edit does not match, nothing changes and the whole call fails. Every call produces the next version of that canvas."
}

/// 执行一次调用：定下这一版全文，落进 meta，给模型只回一句。
fn show(call: &ToolCall, context: &ToolContext) -> ToolResult {
    let request = match parse(&call.arguments) {
        Ok(request) => request,
        Err(failure) => return ToolResult::error(call.id.clone(), failure),
    };
    match request {
        Request::New { title, note, html } => {
            let id = next_id(&context.session().canvases());
            publish(
                call,
                context,
                CanvasState {
                    id,
                    n: 1,
                    title,
                    note,
                    html,
                },
            )
        }
        Request::Rewrite {
            id,
            title,
            note,
            html,
        } => {
            let Some(current) = existing(context, &id) else {
                return ToolResult::error(call.id.clone(), unknown_id(&id));
            };
            publish(
                call,
                context,
                CanvasState {
                    id,
                    n: current.n + 1,
                    title,
                    note,
                    html,
                },
            )
        }
        Request::Edit {
            id,
            title,
            note,
            edits,
        } => {
            let Some(current) = existing(context, &id) else {
                return ToolResult::error(call.id.clone(), unknown_id(&id));
            };
            let html = match apply_edits(&current.html, &edits) {
                Ok(html) => html,
                Err(failure) => return ToolResult::error(call.id.clone(), failure),
            };
            publish(
                call,
                context,
                CanvasState {
                    id,
                    n: current.n + 1,
                    title,
                    note,
                    html,
                },
            )
        }
    }
}

/// 这个会话里现有的画布（按 id 找）。
fn existing(context: &ToolContext, id: &str) -> Option<CanvasState> {
    context
        .session()
        .canvases()
        .into_iter()
        .find(|canvas| canvas.id == id)
}

/// 把这一版写进工具会话状态（下一版据此算全文和版本号），并回给模型一句。
fn publish(call: &ToolCall, context: &ToolContext, canvas: CanvasState) -> ToolResult {
    let content = format!(
        "画布「{}」第 {} 版已显示（{}）",
        canvas.title, canvas.n, canvas.id
    );
    let meta = json!({
        "canvas": {
            "id": &canvas.id,
            "n": canvas.n,
            "title": &canvas.title,
            "note": &canvas.note,
            "html": &canvas.html,
        }
    });
    context.session().put_canvas(canvas);
    ToolResult::success_with_meta(call.id.clone(), content, meta)
}

/// 会话里下一个画布的 id：cv_1、cv_2…（同一会话内唯一；日志重放时 id 可复现）。
fn next_id(canvases: &[CanvasState]) -> String {
    let mut number = canvases.len() + 1;
    loop {
        let id = format!("cv_{number}");
        if !canvases.iter().any(|canvas| canvas.id == id) {
            return id;
        }
        number += 1;
    }
}

/// 按顺序做局部替换：每个 old 必须恰好出现一次；有一处不成立，整次不改（返回错误）。
fn apply_edits(html: &str, edits: &[Edit]) -> Result<String, ToolFailure> {
    let mut text = html.to_owned();
    for (index, edit) in edits.iter().enumerate() {
        let position = index + 1;
        match text.matches(&edit.old).count() {
            1 => text = text.replacen(&edit.old, &edit.new, 1),
            0 => {
                return Err(ToolFailure::coded(
                    "canvas/edit-not-found",
                    format!(
                        "edit {position} did not match: {} appears 0 times in the current canvas; nothing was changed",
                        quote(&edit.old)
                    ),
                ));
            }
            count => {
                return Err(ToolFailure::coded(
                    "canvas/edit-ambiguous",
                    format!(
                        "edit {position} is ambiguous: {} appears {count} times; nothing was changed (pass more surrounding text)",
                        quote(&edit.old)
                    ),
                ));
            }
        }
    }
    Ok(text)
}

/// 失败消息里引用一段旧文本（太长就截断；换行保留原样）。
fn quote(text: &str) -> String {
    let mut head: String = text.chars().take(40).collect();
    if text.chars().count() > 40 {
        head.push('…');
    }
    format!("'{head}'")
}

/// 不存在的画布 id。
fn unknown_id(id: &str) -> ToolFailure {
    ToolFailure::coded(
        "canvas/unknown-id",
        format!(
            "no canvas with id '{id}' in this conversation; pass an id from an earlier canvas result, or omit 'id' to start a new canvas"
        ),
    )
}

/// 参数形状错误（html/edits 二选一、字段类型、edits 内容）。
fn bad_args(message: impl Into<String>) -> ToolFailure {
    ToolFailure::coded("canvas/bad-args", message)
}

/// 解析参数：定下这是新建、整份重写还是局部改。
fn parse(arguments: &str) -> Result<Request, ToolFailure> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|error| bad_args(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = value.as_object() else {
        return Err(bad_args("arguments must be a JSON object"));
    };
    const FIELDS: [&str; 5] = ["id", "title", "note", "html", "edits"];
    if let Some(key) = object.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(bad_args(format!("unknown argument '{key}'")));
    }
    let title = required_text(object.get("title"), "title")?;
    let note = required_text(object.get("note"), "note")?;
    let id = optional_text(object.get("id"), "id")?;
    let html = match object.get("html") {
        None | Some(Value::Null) => None,
        Some(Value::String(html)) => Some(html.clone()),
        Some(_) => return Err(bad_args("'html' must be a string with the page body")),
    };
    let edits = match object.get("edits") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => Some(parse_edits(items)?),
        Some(_) => return Err(bad_args("'edits' must be an array of {old, new} objects")),
    };
    match (html, edits) {
        (Some(html), None) => Ok(match id {
            Some(id) => Request::Rewrite {
                id,
                title,
                note,
                html,
            },
            None => Request::New { title, note, html },
        }),
        (None, Some(edits)) => match id {
            Some(id) => Ok(Request::Edit {
                id,
                title,
                note,
                edits,
            }),
            None => Err(bad_args(
                "'edits' changes an existing canvas: pass 'id', or use 'html' to create one",
            )),
        },
        (Some(_), Some(_)) => Err(bad_args("pass exactly one of 'html' and 'edits'")),
        (None, None) => Err(bad_args(
            "pass 'html' (the whole page) or 'edits' (a list of {old, new} replacements)",
        )),
    }
}

/// 一条局部替换：old 非空、new 是字符串；不认识的键直接报错（多半是拼错）。
fn parse_edits(items: &[Value]) -> Result<Vec<Edit>, ToolFailure> {
    if items.is_empty() {
        return Err(bad_args("'edits' must contain at least one replacement"));
    }
    let mut edits = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let position = index + 1;
        let Some(object) = item.as_object() else {
            return Err(bad_args(format!(
                "edit {position} must be an object with 'old' and 'new'"
            )));
        };
        if let Some(key) = object.keys().find(|key| *key != "old" && *key != "new") {
            return Err(bad_args(format!(
                "edit {position} has an unknown key '{key}'"
            )));
        }
        let old = match object.get("old") {
            Some(Value::String(old)) if !old.is_empty() => old.clone(),
            _ => {
                return Err(bad_args(format!(
                    "edit {position}: 'old' must be a non-empty string"
                )));
            }
        };
        let new = match object.get("new") {
            Some(Value::String(new)) => new.clone(),
            _ => return Err(bad_args(format!("edit {position}: 'new' must be a string"))),
        };
        edits.push(Edit { old, new });
    }
    Ok(edits)
}

/// title / note 必须是非空字符串（空白也算空）。
fn required_text(value: Option<&Value>, name: &str) -> Result<String, ToolFailure> {
    match value {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
        _ => Err(bad_args(format!("'{name}' must be a non-empty string"))),
    }
}

/// 可选的 id：给了就必须是非空字符串；null 当没给。
fn optional_text(value: Option<&Value>, name: &str) -> Result<Option<String>, ToolFailure> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.clone())),
        _ => Err(bad_args(format!("'{name}' must be a non-empty string"))),
    }
}
