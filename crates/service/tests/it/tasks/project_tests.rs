//! 项目协议三方法 + task_set_project；创建/更新共用校验。

use super::*;
use service::{
    Call, ProjectCreateRequest, ProjectUpdateRequest, Response, ServiceError, TaskSetProjectRequest,
};

fn draft(title: &str) -> NewTask {
    NewTask {
        title: title.to_owned(),
        requirement: "做".to_owned(),
        dod: "做完".to_owned(),
        cwd: std::env::temp_dir(),
        conversation: ConversationId::new("kind-project"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

#[tokio::test]
async fn project_protocol_create_update_list_and_task_assignment() {
    let fixture = Fixture::new().await;
    let created = fixture
        .host
        .dispatch(Call::ProjectCreate(ProjectCreateRequest {
            name: "  清河县农技站  ".to_owned(),
            goal: "把农技站的事管起来".to_owned(),
            dir: Some("~/projects/qinghe".to_owned()),
        }))
        .await
        .unwrap();
    let Response::ProjectCreate(created) = created else {
        panic!("不是 project_create");
    };
    assert_eq!(created.project.name, "清河县农技站");
    assert!(created.project.goal.contains("农技站"));
    let home = std::env::var("HOME").unwrap();
    assert_eq!(
        created.project.dir.as_deref(),
        Some(format!("{home}/projects/qinghe").as_str()),
        "~ 展开成绝对路径（不要求目录存在）"
    );
    let json = serde_json::to_value(&created).unwrap();
    assert!(json["project"].get("code").is_none());
    assert!(json["project"].get("color").is_none());

    let listed = fixture
        .host
        .dispatch(Call::ProjectList(service::ProjectListRequest {}))
        .await
        .unwrap();
    let Response::ProjectList(listed) = listed else {
        panic!("不是 project_list");
    };
    assert_eq!(listed.projects.len(), 1);
    assert_eq!(listed.projects[0].id, created.project.id);

    // 重名拒绝（创建）；改名排除自己；校验错误是可读原因。
    let duplicate = fixture
        .host
        .dispatch(Call::ProjectCreate(ProjectCreateRequest {
            name: "清河县农技站".to_owned(),
            goal: "另一个".to_owned(),
            dir: None,
        }))
        .await;
    assert!(
        matches!(duplicate, Err(ServiceError::TaskUnavailable(message)) if message.contains("已经存在"))
    );
    let relative = fixture
        .host
        .dispatch(Call::ProjectCreate(ProjectCreateRequest {
            name: "相对目录".to_owned(),
            goal: "".to_owned(),
            dir: Some("projects/relative".to_owned()),
        }))
        .await;
    assert!(relative.is_err(), "非绝对目录要拒绝");

    let renamed = fixture
        .host
        .dispatch(Call::ProjectUpdate(ProjectUpdateRequest {
            id: created.project.id.clone(),
            name: "清河农技站".to_owned(),
            goal: "改后的目标".to_owned(),
            dir: Some("/tmp".to_owned()),
        }))
        .await
        .unwrap();
    let Response::ProjectUpdate(renamed) = renamed else {
        panic!("不是 project_update");
    };
    assert_eq!(renamed.project.name, "清河农技站");
    assert_eq!(renamed.project.dir.as_deref(), Some("/tmp"));

    let store = store_helpers::tasks_store(fixture.data());
    let task = store.create_task(draft("归项目"), 1).unwrap();
    let assigned = fixture
        .host
        .dispatch(Call::TaskSetProject(TaskSetProjectRequest {
            task_id: task.id.as_str().to_owned(),
            project_id: Some(renamed.project.id.clone()),
        }))
        .await
        .unwrap();
    let Response::TaskSetProject(assigned) = assigned else {
        panic!("不是 task_set_project");
    };
    assert_eq!(assigned.changed, vec![task.id.as_str().to_owned()]);
    assert_eq!(
        store.task(&task.id).unwrap().unwrap().project_id.as_deref(),
        Some(renamed.project.id.as_str())
    );

    let cleared = fixture
        .host
        .dispatch(Call::TaskSetProject(TaskSetProjectRequest {
            task_id: task.id.as_str().to_owned(),
            project_id: None,
        }))
        .await
        .unwrap();
    let Response::TaskSetProject(cleared) = cleared else {
        panic!("不是 task_set_project");
    };
    assert_eq!(cleared.project_id, None);
    assert_eq!(store.task(&task.id).unwrap().unwrap().project_id, None);
}
