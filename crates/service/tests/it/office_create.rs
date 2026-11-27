//! office_create 写 library/office/<taskId>/ + 程序读回 + 回执。

use std::sync::Arc;

use tools::{Tool, ToolCall, ToolContext, ToolSessionState};

fn call(args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "call-office".to_owned(),
        name: "office_create".to_owned(),
        arguments: args.to_string(),
    }
}

#[tokio::test]
async fn office_create_writes_library_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let library = data_dir.join("library/office/task-1");
    let context = ToolContext::with_session(
        data_dir,
        Arc::new(ToolSessionState::new(data_dir, "session-1")),
    )
    .with_write_scope(Some(data_dir.join("user-work")))
    .with_extra_write_scope(Some(library.clone()));
    let tool = office::OfficeCreateTool::new(data_dir, "task-1");
    let result = tool
        .execute(
            call(serde_json::json!({
                "name": "水果表.xlsx",
                "format": "xlsx",
                "sheets": [
                    {"name": "汇总", "rows": [["名字","数量"],["苹果",3]]},
                    {"name": "明细", "rows": [["说明"],["第二张表"]]}
                ]
            })),
            &context,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    assert!(
        result.content.contains("未做视觉自检"),
        "{}",
        result.content
    );
    let target = library.join("水果表.xlsx");
    assert!(target.is_file(), "生成物在 library/office/<taskId>/");
    let bytes = std::fs::read(&target).unwrap();
    let document = office::verify_xlsx(&bytes).expect("calamine 独立读回");
    assert_eq!(document.sheet_names, vec!["汇总", "明细"]);
    let diffs = result
        .meta
        .as_ref()
        .and_then(|meta| meta.get("diffs"))
        .and_then(serde_json::Value::as_array)
        .expect("meta.diffs");
    assert_eq!(diffs.len(), 1);
    assert_eq!(diffs[0]["path"], target.display().to_string());
}

#[tokio::test]
async fn office_create_rejects_bad_name_format_and_outside_scope() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let library = data_dir.join("library/office/task-1");
    let context = ToolContext::with_session(
        data_dir,
        Arc::new(ToolSessionState::new(data_dir, "session-1")),
    )
    .with_write_scope(Some(data_dir.join("user-work")))
    .with_extra_write_scope(Some(library));
    let tool = office::OfficeCreateTool::new(data_dir, "task-1");
    for args in [
        serde_json::json!({"name": "../逃出去.xlsx", "sheets": [{"name":"表","rows":[]}]}),
        serde_json::json!({"name": "表.docx", "sheets": [{"name":"表","rows":[]}]}),
        serde_json::json!({"name": "表.xlsx", "format": "docx", "sheets": [{"name":"表","rows":[]}]}),
    ] {
        let result = tool.execute(call(args), &context).await;
        assert!(result.is_error, "非法参数要拒绝：{result:?}");
    }
    assert!(
        !data_dir.join("library/office/task-1/表.xlsx").exists(),
        "非法调用不落文件"
    );
}

#[tokio::test]
async fn office_create_is_plugin_closed_and_writes_nothing_when_plugin_off() {
    use tools::{PluginId, PluginSwitches, ToolSet, execute_batch};

    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let library = data_dir.join("library/office/task-1");
    let switches = PluginSwitches::new();
    let mut set = ToolSet::new(vec![Box::new(office::OfficeCreateTool::new(
        data_dir, "task-1",
    ))]);
    set.set_plugin_switches(switches.clone());
    let context = ToolContext::with_session(
        data_dir,
        Arc::new(ToolSessionState::new(data_dir, "session-1")),
    )
    .with_write_scope(Some(data_dir.join("user-work")))
    .with_extra_write_scope(Some(library.clone()));
    let args = serde_json::json!({
        "name": "水果表.xlsx",
        "sheets": [{"name": "汇总", "rows": [["名字"],["苹果"]]}]
    });
    let outcome = execute_batch(
        &set,
        &[call(args)],
        1,
        &context,
        &model::CancelToken::new(
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(tokio::sync::Notify::new()),
        ),
    )
    .await;
    assert_eq!(
        outcome.results[0].error_code.as_deref(),
        Some("PLUGIN_CLOSED")
    );
    assert!(!library.join("水果表.xlsx").exists(), "插件关时不生成文件");

    switches.enable(PluginId::Office);
    let outcome = execute_batch(
        &set,
        &[call(serde_json::json!({
            "name": "水果表.xlsx",
            "sheets": [{"name": "汇总", "rows": [["名字"],["苹果"]]}]
        }))],
        1,
        &context,
        &model::CancelToken::new(
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(tokio::sync::Notify::new()),
        ),
    )
    .await;
    assert!(!outcome.results[0].is_error, "{:?}", outcome.results[0]);
    assert!(library.join("水果表.xlsx").is_file());
}
