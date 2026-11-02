//! T006.12：todos 会话投影（DSH session-projections 的 todos key，UI/T008 消费）。

use serde_json::json;

use crate::{
    Event, EventKind, TODOS_PROJECTION_IN_SUMMARY, TODOS_PROJECTION_KEY,
    TODOS_PROJECTION_STATE_VERSION, TodoItem, TodoStatus, TodosProjection, todos_projection,
};

fn event(seq: u64, kind: EventKind) -> Event {
    Event {
        seq,
        time: seq,
        kind,
        surface_op: None,
        source_event_seqs: None,
    }
}

fn todo(content: &str, status: TodoStatus) -> TodoItem {
    TodoItem {
        content: content.to_owned(),
        status,
    }
}

#[test]
fn projection_folds_latest_and_clears_on_next_turn_start() {
    let first = vec![todo("plan", TodoStatus::InProgress)];
    let second = vec![todo("write", TodoStatus::Pending)];
    assert_eq!(todos_projection(&[]).view, None, "首次写之前是 null");
    assert_eq!(
        todos_projection(&[event(1, EventKind::TurnStart { turn: 1 })]).view,
        None
    );
    assert_eq!(
        todos_projection(&[
            event(1, EventKind::TurnStart { turn: 1 }),
            event(
                2,
                EventKind::TodoWrite {
                    todos: first.clone()
                }
            ),
        ])
        .view,
        Some(first.clone()),
        "最近一次 todo/write"
    );
    assert_eq!(
        todos_projection(&[
            event(1, EventKind::TurnStart { turn: 1 }),
            event(
                2,
                EventKind::TodoWrite {
                    todos: first.clone()
                }
            ),
            event(
                3,
                EventKind::TodoWrite {
                    todos: second.clone()
                }
            ),
        ])
        .view,
        Some(second),
        "整表最后写胜出"
    );
    assert_eq!(
        todos_projection(&[
            event(1, EventKind::TurnStart { turn: 1 }),
            event(
                2,
                EventKind::TodoWrite {
                    todos: first.clone()
                }
            ),
            event(3, EventKind::TurnStart { turn: 2 }),
        ])
        .view,
        None,
        "下一个 turn/start 清空（DSH fold：turn/start -> null）"
    );
    assert_eq!(
        todos_projection(&[
            event(1, EventKind::TurnStart { turn: 1 }),
            event(
                2,
                EventKind::TodoWrite {
                    todos: first.clone()
                }
            ),
            event(3, EventKind::StepStart { turn: 1, step: 2 }),
        ])
        .view,
        Some(first),
        "不相关事件保持原状态"
    );
}

#[test]
fn projection_metadata_and_wire_shape() {
    assert_eq!(TODOS_PROJECTION_KEY, "todos");
    assert_eq!(
        TODOS_PROJECTION_STATE_VERSION, 2,
        "DSH tool-todo stateVersion: 2"
    );
    assert_eq!(
        TODOS_PROJECTION_IN_SUMMARY, false,
        "DSH wire.summary: false"
    );
    let empty = TodosProjection {
        key: TODOS_PROJECTION_KEY.to_owned(),
        state_version: TODOS_PROJECTION_STATE_VERSION,
        summary: TODOS_PROJECTION_IN_SUMMARY,
        view: None,
    };
    assert_eq!(
        serde_json::to_value(&empty).unwrap(),
        json!({"key": "todos", "stateVersion": 2, "summary": false, "view": null})
    );
    let filled = TodosProjection {
        view: Some(vec![todo("finish", TodoStatus::Completed)]),
        ..empty
    };
    let value = serde_json::to_value(&filled).unwrap();
    assert_eq!(
        value["view"],
        json!([{"content": "finish", "status": "completed"}])
    );
}
