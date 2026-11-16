//! 假模型的 SSE 脚本：ready/报告/暂停卡/skill/重启卡与写任务工具流。

use super::*;

pub(super) fn report_sse(
    index: usize,
    call: &str,
    state: &str,
    summary: &str,
    question: Option<&str>,
) -> String {
    let mut args = serde_json::json!({"state": state, "summary": summary});
    if let Some(question) = question {
        args["question"] = serde_json::json!(question);
    }
    crate::common::tool_use_sse(index, call, "task_report", &args.to_string())
}

pub(super) fn skill_sse(call_id: &str, name: &str) -> String {
    crate::common::tool_use_sse(
        0,
        call_id,
        "skill",
        &serde_json::json!({ "name": name }).to_string(),
    )
}

/// Kind 的 set_paused 确认卡：详情只有一个布尔，事实行由服务端填。
pub(super) fn pause_card_sse(call_id: &str, paused: bool) -> String {
    let args = serde_json::json!({
        "title": "全局暂停",
        "brief": if paused {
            "打开以后新任务先排队，在跑的这一轮跑完。"
        } else {
            "关掉以后排队的任务按原顺序开跑。"
        },
        "options": [{
            "id": "one",
            "action": "set_paused",
            "label": if paused { "暂停新任务" } else { "继续开跑" },
            "details": { "paused": paused },
        }],
    })
    .to_string();
    crate::common::tool_use_sse(1, call_id, "propose_action", &args)
}

/// 任务上报 ready 的两步回复（工具调用 + 最终回答）。
pub(super) fn ready_scripts() -> [String; 2] {
    [
        crate::common::tool_use_sse(
            2,
            "call-report",
            "task_report",
            &serde_json::json!({"state": "ready", "summary": "查好了"}).to_string(),
        ),
        crate::common::text_sse("done"),
    ]
}

/// Kind 重启汇报这一轮的回复：加载 aios-tasks、出一张每任务一项的接续卡、说一句。
pub(super) fn push_restart_turn(router: &Router, call_id: &str, tasks: &[&Task], reply: &str) {
    let options: Vec<Value> = tasks
        .iter()
        .map(|task| {
            serde_json::json!({
                "id": task.id.as_str(),
                "action": "resume_task",
                "label": task.title,
                "details": { "task_id": task.id.as_str() },
            })
        })
        .collect();
    let args = serde_json::json!({
        "title": "接着做被打断的任务",
        "brief": "每个任务一项：勾一个就接着做，不勾就继续停着。",
        "options": options,
    })
    .to_string();
    router.push_kind(skill_sse("call-load-tasks", "aios-tasks"));
    router.push_kind(crate::common::tool_use_sse(
        1,
        call_id,
        "propose_action",
        &args,
    ));
    router.push_kind(crate::common::text_sse(reply));
}

pub(super) fn write_file_sse(call_id: &str, file_path: &str, content: &str) -> String {
    crate::common::tool_use_sse(
        1,
        call_id,
        "write",
        &serde_json::json!({"file_path": file_path, "content": content}).to_string(),
    )
}

pub(super) fn read_file_sse(call_id: &str, file_path: &str) -> String {
    crate::common::tool_use_sse(
        1,
        call_id,
        "read",
        &serde_json::json!({"file_path": file_path}).to_string(),
    )
}

pub(super) fn edit_file_sse(
    call_id: &str,
    file_path: &str,
    old_string: &str,
    new_string: &str,
) -> String {
    crate::common::tool_use_sse(
        1,
        call_id,
        "edit",
        &serde_json::json!({
            "file_path": file_path,
            "old_string": old_string,
            "new_string": new_string
        })
        .to_string(),
    )
}
