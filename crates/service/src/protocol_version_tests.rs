use super::*;

#[test]
fn protocol_version_is_32() {
    assert_eq!(
        PROTOCOL_VERSION, 32,
        "监控 Attempt 花费/rings 命令段/日柱分组/排行分组只增字段，升到 32"
    );
}

#[test]
fn project_calls_use_the_specified_wire_methods() {
    let list: Call = serde_json::from_str(r#"{"method":"project_list"}"#).unwrap();
    assert!(matches!(list, Call::ProjectList(_)));
    let create: Call =
        serde_json::from_str(r#"{"method":"project_create","name":"甲","goal":"乙"}"#).unwrap();
    assert!(matches!(create, Call::ProjectCreate(_)));
    let update: Call =
        serde_json::from_str(r#"{"method":"project_update","id":"p1","name":"甲","goal":"乙"}"#)
            .unwrap();
    assert!(matches!(update, Call::ProjectUpdate(_)));
    let set: Call =
        serde_json::from_str(r#"{"method":"task_set_project","taskId":"t1","projectId":null}"#)
            .unwrap();
    assert!(matches!(set, Call::TaskSetProject(_)));
    let response = serde_json::to_value(Response::ProjectList(ProjectListResponse {
        projects: Vec::new(),
    }))
    .unwrap();
    assert_eq!(response["method"], "project_list");
}
