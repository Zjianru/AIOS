//! 项目协议三方法 + task_set_project；创建/更新共用校验。

use super::*;
use service::{
    Call, ProjectCreateRequest, ProjectUpdateRequest, Response, ServiceError, TaskSetProjectRequest,
};

fn draft(title: &str) -> NewTask {
    NewTask {
        quote: None,
        quote_source: None,
        kind_note: None,
        project_id: None,
        cwd_source: None,
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
        matches!(duplicate, Err(ServiceError::TaskUnavailable(message)) if message.contains("已经有一个叫"))
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

#[tokio::test]
async fn task_accept_uses_the_project_and_its_default_directory() {
    let fixture = Fixture::new().await;
    let project_dir = fixture.cwd.path().to_path_buf();
    let store = tasks::ProjectStore::open(fixture.data().join(tasks::PROJECTS_DB_FILE)).unwrap();
    let project = store
        .create(
            tasks::NewProject {
                name: "带默认目录".to_owned(),
                goal: "做一件事".to_owned(),
                dir: Some(project_dir.clone()),
            },
            1,
        )
        .unwrap();
    let task = fixture
        .accept_args(
            true,
            serde_json::json!({
                "title": "查默认目录",
                "requirement": "看看目录",
                "dod": "有结果",
                "project": project.id,
            }),
            0,
        )
        .await;
    assert_eq!(task.project_id.as_deref(), Some(project.id.as_str()));
    assert_eq!(task.cwd_source.as_deref(), Some("project"));
    assert_eq!(task.cwd, std::fs::canonicalize(project_dir).unwrap());
}

#[tokio::test]
async fn task_accept_reports_a_missing_project_default_directory() {
    let fixture = Fixture::new().await;
    let store = tasks::ProjectStore::open(fixture.data().join(tasks::PROJECTS_DB_FILE)).unwrap();
    let missing = fixture.cwd.path().join("does-not-exist");
    let project = store
        .create(
            tasks::NewProject {
                name: "目录不存在".to_owned(),
                goal: "做一件事".to_owned(),
                dir: Some(missing),
            },
            1,
        )
        .unwrap();
    fixture
        .host
        .task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-missing-dir",
        "task_accept",
        &serde_json::json!({
            "title": "缺目录",
            "quote": "改一下",
            "requirement": "看看目录",
            "dod": "有结果",
            "project": project.id,
        })
        .to_string(),
    ));
    fixture
        .router
        .push_kind(crate::common::text_sse("没派出去"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-missing", "改一下")))
        .await
        .unwrap();
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    let events = crate::common::read_root_events(fixture.data());
    let results: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        results
            .iter()
            .any(|text| text.contains("项目默认目录") && text.contains("不存在")),
        "{results:?}"
    );
    assert!(
        tasks_store(fixture.data()).all_tasks().unwrap().is_empty(),
        "目录不存在时不能落任务"
    );
}

#[tokio::test]
async fn task_graph_propose_carries_the_project_to_every_child() {
    let fixture = Fixture::new().await;
    let project_dir = fixture.cwd.path().to_path_buf();
    let store = tasks::ProjectStore::open(fixture.data().join(tasks::PROJECTS_DB_FILE)).unwrap();
    let project = store
        .create(
            tasks::NewProject {
                name: "图项目".to_owned(),
                goal: "做两步".to_owned(),
                dir: Some(project_dir.clone()),
            },
            1,
        )
        .unwrap();
    fixture
        .host
        .task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-graph-project",
        "task_graph_propose",
        &serde_json::json!({
            "title": "图",
            "quote": "拆两步",
            "requirement": "两步",
            "dod": "都有结果",
            "project": project.id,
            "subtasks": [
                {"title": "一", "requirement": "第一步", "dod": "有结果"},
                {"title": "二", "requirement": "第二步", "dod": "有结果", "after": [0]}
            ]
        })
        .to_string(),
    ));
    fixture.router.push_kind(crate::common::text_sse("已提议"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-graph", "拆两步")))
        .await
        .unwrap();
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    let store = tasks_store(fixture.data());
    let root = store
        .all_tasks()
        .unwrap()
        .into_iter()
        .find(tasks::Task::is_graph_root)
        .expect("图");
    assert_eq!(root.project_id.as_deref(), Some(project.id.as_str()));
    assert_eq!(root.cwd_source.as_deref(), Some("project"));
    let children = store.children(&root.id).unwrap();
    assert_eq!(children.len(), 2);
    for child in children {
        assert_eq!(child.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(child.cwd, std::fs::canonicalize(&project_dir).unwrap());
        assert_eq!(child.versions[0].quote.as_deref(), Some("拆两步"));
        let header = task_read_header(&fixture.host, &child.id).await;
        assert_eq!(header.requirement_versions.len(), 1);
        assert_eq!(
            header.requirement_versions[0].quote.as_deref(),
            Some("拆两步")
        );
        assert_eq!(
            header.requirement_versions[0].inherited_from.as_deref(),
            Some(root.title.as_str())
        );
    }
}

#[tokio::test]
async fn project_create_tool_creates_directly_and_refuses_a_duplicate() {
    let fixture = Fixture::new().await;
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-project",
        "project_create",
        &serde_json::json!({
            "name": "工具建的",
            "goal": "直接建",
            "dir": "~/projects/tool-project"
        })
        .to_string(),
    ));
    fixture.router.push_kind(crate::common::text_sse("建好了"));
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-project-tool",
            "建个项目",
        )))
        .await
        .unwrap();
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    let store = tasks::ProjectStore::open(fixture.data().join(tasks::PROJECTS_DB_FILE)).unwrap();
    let projects = store.list().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "工具建的");
    let events = crate::common::read_root_events(fixture.data());
    let results: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        results.iter().any(|text| text.contains("已建：项目")),
        "{results:?}"
    );
}

#[tokio::test]
async fn project_list_is_in_the_dynamic_fixed_block_and_updates_at_the_tail() {
    let fixture = Fixture::new().await;
    fixture.router.push_kind(crate::common::text_sse("第一轮"));
    fixture.router.push_kind(crate::common::text_sse("第二轮"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-dynamic-1", "你好")))
        .await
        .unwrap();
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    let first_events = crate::common::read_root_events(fixture.data());
    let first_text: Vec<String> = first_events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        first_text
            .iter()
            .any(|text| text.contains("项目清单：还没有项目")),
        "{first_text:?}"
    );

    let project_dir = fixture.cwd.path().to_string_lossy().into_owned();
    let created = fixture
        .host
        .dispatch(Call::ProjectCreate(service::ProjectCreateRequest {
            name: "新项目".to_owned(),
            goal: "后面会出现".to_owned(),
            dir: Some(project_dir.clone()),
        }))
        .await
        .unwrap();
    assert!(matches!(created, Response::ProjectCreate(_)));
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-dynamic-2",
            "再说一句",
        )))
        .await
        .unwrap();
    crate::common::wait_turns(&fixture.host, fixture.data(), 2).await;
    let events = crate::common::read_root_events(fixture.data());
    let texts: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(model::ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    let first_at = texts
        .iter()
        .position(|text| text.contains("项目清单：还没有项目"))
        .expect("初始项目清单");
    let created_at = texts
        .iter()
        .position(|text| text.contains("新项目"))
        .expect("新项目清单");
    assert!(created_at > first_at, "变化的清单补在末尾");
    let first_text_after = &texts[first_at];
    assert!(!first_text_after.contains("新项目"), "旧消息前缀不被改写");
    // 固定块只留 id/名字/一句目标；默认目录只在 aios_query project 里。
    let created_text = &texts[created_at];
    assert!(created_text.contains("新项目"), "{created_text}");
    assert!(created_text.contains("后面会出现"), "{created_text}");
    assert!(
        !created_text.contains(&project_dir),
        "固定块不能带默认目录：{created_text}"
    );
    assert!(
        !created_text.contains("默认目录"),
        "固定块不能带默认目录字样：{created_text}"
    );
}

#[tokio::test]
async fn create_project_action_creates_once_and_reconciles_an_existing_name() {
    use service::ActionEnv;
    let fixture = Fixture::new().await;
    let actions = service::ActionRegistry::settings(fixture.data().to_path_buf(), None);
    let action = actions
        .get("create_project")
        .expect("create_project 已登记");
    let details = action
        .details(&serde_json::json!({
            "name": "卡片建项目",
            "goal": "确认后才建",
            "dir": "~/projects/card"
        }))
        .unwrap();
    let env = ActionEnv {
        data_dir: fixture.data(),
        identity: None,
        events: &[],
        conversation: None,
    };
    let facts = action.describe(&details, &env).unwrap();
    assert!(
        facts
            .iter()
            .any(|fact| fact.name == "名字" && fact.value == "卡片建项目")
    );
    action.reconcile(&details, &facts, &env).unwrap();
    let first = action.act(&details, &fixture.host).await.unwrap();
    assert!(first.summary.contains("已建：项目"), "{}", first.summary);
    let store = tasks::ProjectStore::open(fixture.data().join(tasks::PROJECTS_DB_FILE)).unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
    let second = action.act(&details, &fixture.host).await.unwrap();
    assert!(second.summary.contains("已经建过了"), "{}", second.summary);
    assert_eq!(store.list().unwrap().len(), 1);
}

#[tokio::test]
async fn task_accept_directory_order_is_user_then_project_then_conversation() {
    let fixture = Fixture::new().await;
    let project_dir = tempfile::tempdir().unwrap();
    let user_dir = tempfile::tempdir().unwrap();
    let store = tasks::ProjectStore::open(fixture.data().join(tasks::PROJECTS_DB_FILE)).unwrap();
    let project = store
        .create(
            tasks::NewProject {
                name: "目录顺序".to_owned(),
                goal: "做一件事".to_owned(),
                dir: Some(project_dir.path().to_path_buf()),
            },
            1,
        )
        .unwrap();
    let by_user = fixture
        .accept_args(
            true,
            serde_json::json!({
                "title": "用户点名目录",
                "requirement": "看看目录",
                "dod": "有结果",
                "project": project.id,
                "cwd": user_dir.path().to_string_lossy(),
            }),
            0,
        )
        .await;
    assert_eq!(by_user.cwd, std::fs::canonicalize(user_dir.path()).unwrap());
    assert_eq!(by_user.cwd_source.as_deref(), Some("user"));
    assert_eq!(by_user.project_id.as_deref(), Some(project.id.as_str()));
}

#[tokio::test]
async fn read_only_task_starts_from_home_when_no_path_or_project() {
    let fixture = Fixture::new().await;
    let task = fixture
        .accept_args(
            true,
            serde_json::json!({
                "title": "只读调查",
                "quote": "改一下",
                "requirement": "查一下现状",
                "dod": "有结论"
            }),
            0,
        )
        .await;
    let home = std::fs::canonicalize(std::env::var("HOME").unwrap()).unwrap();
    assert_eq!(task.cwd, home, "只读任务不回落对话目录");
    assert_eq!(task.cwd_source.as_deref(), Some("home"));
    assert_ne!(task.cwd, std::fs::canonicalize(fixture.cwd.path()).unwrap());
}

#[tokio::test]
async fn read_only_task_uses_the_user_path_file_parent() {
    let fixture = Fixture::new().await;
    let pointed = tempfile::tempdir().unwrap();
    let file = pointed.path().join("README.md");
    std::fs::write(&file, "readme").unwrap();
    let task = fixture
        .accept_args(
            true,
            serde_json::json!({
                "title": "读文件",
                "quote": "改一下",
                "requirement": "读 README",
                "dod": "有内容",
                "cwd": file.to_string_lossy()
            }),
            0,
        )
        .await;
    assert_eq!(task.cwd, std::fs::canonicalize(pointed.path()).unwrap());
    assert_eq!(task.cwd_source.as_deref(), Some("user"));
}

#[tokio::test]
async fn read_only_graph_starts_from_home_without_a_project() {
    let fixture = Fixture::new().await;
    fixture
        .host
        .task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);
    fixture.router.push_kind(crate::common::tool_use_sse(
        0,
        "call-graph-home",
        "task_graph_propose",
        &serde_json::json!({
            "title": "只读图",
            "quote": "拆两步",
            "requirement": "两步",
            "dod": "都有结果",
            "subtasks": [
                {"title": "一", "requirement": "第一步", "dod": "有结果"},
                {"title": "二", "requirement": "第二步", "dod": "有结果", "after": [0]}
            ]
        })
        .to_string(),
    ));
    fixture.router.push_kind(crate::common::text_sse("已提议"));
    fixture
        .host
        .dispatch(Call::Send(send(&fixture.host, "kind-graph-home", "拆两步")))
        .await
        .unwrap();
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;
    let store = tasks_store(fixture.data());
    let root = store
        .all_tasks()
        .unwrap()
        .into_iter()
        .find(tasks::Task::is_graph_root)
        .expect("图");
    let home = std::fs::canonicalize(std::env::var("HOME").unwrap()).unwrap();
    assert_eq!(root.cwd, home);
    assert_eq!(root.cwd_source.as_deref(), Some("home"));
    for child in store.children(&root.id).unwrap() {
        assert_eq!(child.cwd, home);
        assert_eq!(child.cwd_source.as_deref(), Some("home"));
    }
}
