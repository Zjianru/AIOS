//! T015.1 接缝 1（tools 公开边界）：canvas 新建、meta 形状、给模型的一句。

use serde_json::{Value, json};
use tools::{CanvasTool, Tool, ToolCall, ToolContext, ToolResult, ToolSet};

async fn call(arguments: Value) -> ToolResult {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let tool = CanvasTool::new();
    tool.execute(
        ToolCall {
            id: "call_1".to_owned(),
            name: "canvas".to_owned(),
            arguments: arguments.to_string(),
        },
        &context,
    )
    .await
}

/// 测试清单 1（故事 1）：新建画布 → meta 里有这一版的全文，模型只看到一句「已显示」。
#[tokio::test]
async fn new_canvas_puts_the_whole_html_in_meta_and_one_sentence_to_the_model() {
    let html = "<div id=\"hello\">你好，画布</div><script>1 + 1;</script>";
    let result = call(json!({
        "title": "会走的小猫",
        "note": "一只点了会走的小猫",
        "html": html,
    }))
    .await;

    assert!(!result.is_error, "新建画布不该失败：{}", result.content);
    let meta = result.meta.expect("meta 要带画布全文");
    let canvas = &meta["canvas"];
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
    assert!(!result.content.contains("点了会走"), "模型的消息里没有 note");
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
