//! T015.1/T015.2 接缝 1（tools 公开边界）：新建、重写、局部改、四个错误码、meta 形状。

use serde_json::{Value, json};
use tools::{CanvasTool, Tool, ToolCall, ToolContext, ToolResult, ToolSet};

/// 一段同一会话里的画布调用：上下文和工具复用，状态跨调用（create 之后能按 id 改）。
struct Conversation {
    _dir: tempfile::TempDir,
    context: ToolContext,
    tool: CanvasTool,
}

impl Conversation {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext::new(dir.path());
        Self {
            _dir: dir,
            context,
            tool: CanvasTool::new(),
        }
    }

    async fn call(&self, arguments: Value) -> ToolResult {
        self.tool
            .execute(
                ToolCall {
                    id: "call_1".to_owned(),
                    name: "canvas".to_owned(),
                    arguments: arguments.to_string(),
                },
                &self.context,
            )
            .await
    }

    /// 先画一块画布，返回 (id, 第 1 版结果)。
    async fn start(&self, html: &str) -> (String, ToolResult) {
        let result = self
            .call(json!({"title": "会走的小猫", "note": "第一版", "html": html}))
            .await;
        assert!(!result.is_error, "新建画布不该失败：{}", result.content);
        let id = canvas_of(&result)["id"]
            .as_str()
            .expect("meta.canvas.id 是字符串")
            .to_owned();
        (id, result)
    }
}

fn canvas_of(result: &ToolResult) -> &Value {
    &result.meta.as_ref().expect("meta 里要有 canvas")["canvas"]
}

fn edit(id: &str, old: &str, new: &str) -> Value {
    json!({
        "id": id,
        "title": "会走的小猫",
        "note": "第二版",
        "edits": [{"old": old, "new": new}],
    })
}

fn assert_code(result: &ToolResult, code: &str) {
    assert!(result.is_error, "这次调用应当失败：{}", result.content);
    assert_eq!(
        result.error_code.as_deref(),
        Some(code),
        "错误码不对：{}",
        result.content
    );
}

/// 测试清单 1（故事 1）：新建画布 → meta 里有这一版的全文，模型只看到一句「已显示」。
#[tokio::test]
async fn new_canvas_puts_the_whole_html_in_meta_and_one_sentence_to_the_model() {
    let html = "<div id=\"hello\">你好，画布</div><script>1 + 1;</script>";
    let conversation = Conversation::new();
    let result = conversation
        .call(json!({
            "title": "会走的小猫",
            "note": "一只点了会走的小猫",
            "html": html,
        }))
        .await;

    assert!(!result.is_error, "新建画布不该失败：{}", result.content);
    let canvas = canvas_of(&result);
    assert_eq!(canvas["n"], json!(1));
    assert_eq!(canvas["title"], json!("会走的小猫"));
    assert_eq!(canvas["note"], json!("一只点了会走的小猫"));
    assert_eq!(canvas["html"], json!(html));
    let id = canvas["id"].as_str().expect("meta.canvas.id 是字符串");
    assert!(id.starts_with("cv_"), "画布 id 前缀：{id}");

    // 给模型的只有这一句，HTML 和 note 都不进模型的消息。
    assert_eq!(
        result.content,
        format!("画布「会走的小猫」第 1 版已显示（{id}）")
    );
    assert!(!result.content.contains("<div"), "模型的消息里没有 HTML");
    assert!(
        !result.content.contains("点了会走"),
        "模型的消息里没有 note"
    );
}

/// 测试清单 2（故事 6）：带 id + html 是整份重写，出下一版。
#[tokio::test]
async fn rewrite_with_id_shows_the_next_version_in_meta() {
    let conversation = Conversation::new();
    let (id, _) = conversation.start("<div id=\"v\">1</div>").await;
    let result = conversation
        .call(json!({
            "id": id,
            "title": "会走的小猫",
            "note": "整份重写",
            "html": "<div id=\"v\">2</div>",
        }))
        .await;

    assert!(!result.is_error, "重写不该失败：{}", result.content);
    let canvas = canvas_of(&result);
    assert_eq!(canvas["id"], json!(id), "重写不换 id");
    assert_eq!(canvas["n"], json!(2), "重写出第 2 版");
    assert_eq!(canvas["note"], json!("整份重写"));
    assert_eq!(canvas["html"], json!("<div id=\"v\">2</div>"));
    assert_eq!(
        result.content,
        format!("画布「会走的小猫」第 2 版已显示（{id}）")
    );
}

/// 测试清单 2（故事 6）：局部改每个 old 恰好一次；meta 是改完的全文。
#[tokio::test]
async fn edits_replace_one_occurrence_each_and_show_a_new_version() {
    let conversation = Conversation::new();
    let (id, _) = conversation
        .start("<div id=\"v\">one</div><div id=\"w\">two</div>")
        .await;
    let result = conversation
        .call(json!({
            "id": id,
            "title": "会走的小猫",
            "note": "第二版：两处替换",
            "edits": [
                {"old": "one", "new": "ONE"},
                {"old": "two", "new": "TWO"},
            ],
        }))
        .await;

    assert!(!result.is_error, "局部改不该失败：{}", result.content);
    let canvas = canvas_of(&result);
    assert_eq!(canvas["n"], json!(2));
    assert_eq!(
        canvas["html"],
        json!("<div id=\"v\">ONE</div><div id=\"w\">TWO</div>")
    );
    assert_eq!(
        result.content,
        format!("画布「会走的小猫」第 2 版已显示（{id}）")
    );
}

/// 测试清单 2（故事 6）：old 出现 0 次报 canvas/edit-not-found，整次不改（失败不占版本）。
#[tokio::test]
async fn edit_not_found_reports_the_code_and_leaves_the_canvas_unchanged() {
    let conversation = Conversation::new();
    let (id, _) = conversation.start("<div id=\"v\">1</div>").await;
    let failed = conversation.call(edit(&id, "not in the page", "x")).await;
    assert_code(&failed, "canvas/edit-not-found");
    assert!(
        failed.content.contains("0 times") || failed.content.contains("0 次"),
        "要说清是 0 次：{}",
        failed.content
    );

    // 整次不改：失败没占版本，接着用原文改出来的是第 2 版。
    let next = conversation
        .call(edit(&id, "<div id=\"v\">1</div>", "<div id=\"v\">2</div>"))
        .await;
    assert!(!next.is_error, "后面的成功改不该被前一次失败影响");
    assert_eq!(canvas_of(&next)["n"], json!(2));
    assert_eq!(canvas_of(&next)["html"], json!("<div id=\"v\">2</div>"));
}

/// 测试清单 2（故事 6）：old 出现多次报 canvas/edit-ambiguous，整次不改。
#[tokio::test]
async fn edit_ambiguous_reports_the_code_and_leaves_the_canvas_unchanged() {
    let conversation = Conversation::new();
    let (id, _) = conversation.start("<span>x</span><span>x</span>").await;
    let failed = conversation.call(edit(&id, "x", "y")).await;
    assert_code(&failed, "canvas/edit-ambiguous");

    // 整次不改：失败没占版本；重写出来是第 2 版，不是第 3 版。
    let rewrite = conversation
        .call(json!({
            "id": id,
            "title": "会走的小猫",
            "note": "重写",
            "html": "<p>r</p>",
        }))
        .await;
    assert!(!rewrite.is_error, "重写不该失败：{}", rewrite.content);
    assert_eq!(canvas_of(&rewrite)["n"], json!(2));
}

/// 测试清单 3：不存在的 id 报 canvas/unknown-id（重写和局部改都一样）。
#[tokio::test]
async fn unknown_id_reports_the_code_for_rewrite_and_edits() {
    let conversation = Conversation::new();
    let missing = "cv_404";
    let rewrite = conversation
        .call(json!({
            "id": missing,
            "title": "会走的小猫",
            "note": "重写",
            "html": "<p>x</p>",
        }))
        .await;
    assert_code(&rewrite, "canvas/unknown-id");
    let edit = conversation
        .call(json!({
            "id": missing,
            "title": "会走的小猫",
            "note": "局部改",
            "edits": [{"old": "a", "new": "b"}],
        }))
        .await;
    assert_code(&edit, "canvas/unknown-id");
}

/// 测试清单 3：html 和 edits 都给 → canvas/bad-args。
#[tokio::test]
async fn html_and_edits_together_are_bad_args() {
    let conversation = Conversation::new();
    let (id, _) = conversation.start("<p>x</p>").await;
    let result = conversation
        .call(json!({
            "id": id,
            "title": "会走的小猫",
            "note": "两个都给",
            "html": "<p>y</p>",
            "edits": [{"old": "x", "new": "z"}],
        }))
        .await;
    assert_code(&result, "canvas/bad-args");
}

/// 测试清单 3：html 和 edits 都不给 → canvas/bad-args。
#[tokio::test]
async fn html_and_edits_together_missing_are_bad_args() {
    let conversation = Conversation::new();
    let (id, _) = conversation.start("<p>x</p>").await;
    let result = conversation
        .call(json!({
            "id": id,
            "title": "会走的小猫",
            "note": "两个都不给",
        }))
        .await;
    assert_code(&result, "canvas/bad-args");
    // 新建也一样：不给正文就没有画布。
    let fresh = conversation
        .call(json!({"title": "会走的小猫", "note": "没有正文"}))
        .await;
    assert_code(&fresh, "canvas/bad-args");
}

/// 测试清单 3：没有 id 的 edits 没有可改的画布 → canvas/bad-args。
#[tokio::test]
async fn edits_without_an_id_are_bad_args() {
    let conversation = Conversation::new();
    let result = conversation
        .call(json!({
            "title": "会走的小猫",
            "note": "没有 id",
            "edits": [{"old": "a", "new": "b"}],
        }))
        .await;
    assert_code(&result, "canvas/bad-args");
}

/// 票面验收：工具说明照抄 Claude Code 的「When to use」「What an artifact is not」，
/// 例子换成 AIOS 的，并写明能加载的地址和 aios.send。
#[test]
fn description_has_when_to_use_what_an_artifact_is_not_hosts_and_aios_send() {
    let tool = CanvasTool::new();
    let description = tool.description();
    for heading in ["When to use", "What an artifact is not"] {
        assert!(
            description.contains(heading),
            "工具说明缺 {heading}：{description}"
        );
    }
    for host in [
        "cdnjs.cloudflare.com",
        "cdn.jsdelivr.net/npm/",
        "unpkg.com",
        "cdn.tailwindcss.com",
        "code.jquery.com",
        "fonts.googleapis.com",
        "fonts.gstatic.com",
    ] {
        assert!(description.contains(host), "工具说明缺地址 {host}");
    }
    assert!(
        description.contains("aios.send"),
        "工具说明要写明画布只能 aios.send 一句话"
    );
}

/// spec 实现决定：canvas 是内置工具、默认注册。
#[test]
fn canvas_is_registered_by_default() {
    let set = ToolSet::builtin();
    assert!(set.get("canvas").is_some(), "默认工具集缺 canvas");
    assert!(
        set.definitions()
            .iter()
            .any(|schema| schema.name == "canvas"),
        "请求的 tools 列表里要有 canvas"
    );
}
