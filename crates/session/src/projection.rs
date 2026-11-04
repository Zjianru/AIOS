//! 会话投影（T006.12 照 DSH session-projections 的 todos 投影）：UI（T008）消费的内核侧接口。
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
