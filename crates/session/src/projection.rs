//! 会话投影（T006.12 照 DSH session-projections 的 todos 投影；T015.2 加画布状态折叠）：
//! UI（T008）和画布工具消费的内核侧接口。
//!
//! DSH tool-todo 注册 key=todos、stateVersion=2、summary=false 的投影：fold 取最新一条
//! todo/write 的整表；下一个 turn/start 把它清回 null（turn/end 保留已完成的清单，供 UI 展示）；
//! 首次写之前为 null。本模块只做事件到投影值的纯粹派生，不新增事件、不改会话格式。

use serde::{Deserialize, Serialize};

use crate::{Event, EventKind, TodoItem};

/// 投影 key（DSH sessionProjections.register({ key: 'todos' })）。
pub const TODOS_PROJECTION_KEY: &str = "todos";
/// 投影 stateVersion（DSH tool-todo 的 stateVersion: 2）。
pub const TODOS_PROJECTION_STATE_VERSION: u32 = 2;
/// 是否进入 Session summary（DSH wire.summary: false，排除在摘要之外）。
pub const TODOS_PROJECTION_IN_SUMMARY: bool = false;

/// todos 投影的 wire view（DSH wire.view：整表或 null；序列化即 null / 数组）。
pub type TodosProjectionView = Option<Vec<TodoItem>>;

/// 投影描述（key/stateVersion/summary + 当前 view），给 UI/T008 的传输形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodosProjection {
    pub key: String,
    #[serde(rename = "stateVersion")]
    pub state_version: u32,
    pub summary: bool,
    pub view: TodosProjectionView,
}

/// 一个画布当前的样子（T015.2）：从 tool/result.meta.canvas 折叠出的最新一版。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasState {
    pub id: String,
    /// 第几版，从 1 开始。
    pub n: u64,
    pub title: String,
    pub note: String,
    pub html: String,
}

/// 日志里的一次画布版本事件（T015.3）：版本状态 + 来源事件 seq。
///
/// 剪枝要按事件定位「非最新版」的结果；折叠本身不关心 seq。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasVersion {
    pub seq: u64,
    pub state: CanvasState,
}

/// 按日志顺序列出每个画布版本（T015.3）：只认 `tool/result.meta.canvas` 的原文。
///
/// 压缩剪枝会把旧版结果改写成一句摘要，但原事件还在日志里；带 surfaceOp 的替换事件
/// 一律跳过，所以这里读到的永远是完整版本（折叠照读原事件）。
pub fn canvas_versions(events: &[Event]) -> Vec<CanvasVersion> {
    events
        .iter()
        .filter(|event| event.surface_op.is_none())
        .filter_map(|event| {
            let EventKind::ToolResult {
                meta: Some(meta), ..
            } = &event.kind
            else {
                return None;
            };
            canvas_state(meta).map(|state| CanvasVersion {
                seq: event.seq,
                state,
            })
        })
        .collect()
}

/// 从会话事件折叠每个画布的当前版（T015.2）：工具要据此出新一版，页面据此画版本。
///
/// 同一画布后出现的版本胜出（日志顺序即版本顺序），与 `canvas_versions` 同源。
pub fn canvases(events: &[Event]) -> Vec<CanvasState> {
    let mut out: Vec<CanvasState> = Vec::new();
    for version in canvas_versions(events) {
        match out.iter_mut().find(|item| item.id == version.state.id) {
            Some(item) => *item = version.state,
            None => out.push(version.state),
        }
    }
    out
}

/// `meta.canvas` 的形状（页面那边读同一套字段）。
fn canvas_state(meta: &serde_json::Value) -> Option<CanvasState> {
    let canvas = meta.get("canvas")?;
    let id = canvas.get("id")?.as_str()?;
    let html = canvas.get("html")?.as_str()?;
    let title = canvas.get("title")?.as_str()?;
    let n = canvas
        .get("n")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    let note = canvas
        .get("note")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Some(CanvasState {
        id: id.to_owned(),
        n,
        title: title.to_owned(),
        note: note.to_owned(),
        html: html.to_owned(),
    })
}

/// 从会话事件折叠 todos 投影（照 DSH 的 todo 投影 fold）。
pub fn todos_projection(events: &[Event]) -> TodosProjection {
    let mut view: TodosProjectionView = None;
    for event in events {
        // 照 DSH tool-todo 的 fold：最新整表；下一个 turn/start 清回 null；其它事件保持。
        view = match &event.kind {
            EventKind::TodoWrite { todos } => Some(todos.clone()),
            EventKind::TurnStart { .. } => None,
            _ => view,
        };
    }
    TodosProjection {
        key: TODOS_PROJECTION_KEY.to_owned(),
        state_version: TODOS_PROJECTION_STATE_VERSION,
        summary: TODOS_PROJECTION_IN_SUMMARY,
        view,
    }
}
