//! T079：项目领域在服务端的校验与协议视图。创建/更新共用 prepare_project。

use std::path::{Path, PathBuf};

use crate::error::ServiceError;
use crate::protocol::{
    ProjectCreateRequest, ProjectCreateResponse, ProjectListResponse, ProjectUpdateRequest,
    ProjectUpdateResponse, ProjectView, TaskSetProjectRequest, TaskSetProjectResponse,
};

pub(crate) fn list(store: &tasks::ProjectStore) -> Result<ProjectListResponse, ServiceError> {
    let projects = store
        .list()
        .map_err(project_error)?
        .iter()
        .map(view)
        .collect();
    Ok(ProjectListResponse { projects })
}

pub(crate) fn create(
    store: &tasks::ProjectStore,
    request: ProjectCreateRequest,
) -> Result<ProjectCreateResponse, ServiceError> {
    let draft = prepare_project(
        store,
        &request.name,
        &request.goal,
        request.dir.as_deref(),
        None,
    )?;
    let project = store.create(draft, now_ms()).map_err(project_error)?;
    Ok(ProjectCreateResponse {
        project: view(&project),
    })
}

pub(crate) fn update(
    store: &tasks::ProjectStore,
    request: ProjectUpdateRequest,
) -> Result<ProjectUpdateResponse, ServiceError> {
    let draft = prepare_project(
        store,
        &request.name,
        &request.goal,
        request.dir.as_deref(),
        Some(&request.id),
    )?;
    let project = store.update(&request.id, draft).map_err(project_error)?;
    Ok(ProjectUpdateResponse {
        project: view(&project),
    })
}

pub(crate) fn set_task_project(
    runtime: &crate::executor::TaskRuntime,
    store: &tasks::ProjectStore,
    request: TaskSetProjectRequest,
) -> Result<TaskSetProjectResponse, ServiceError> {
    if let Some(project_id) = request.project_id.as_deref()
        && store.get(project_id).map_err(project_error)?.is_none()
    {
        return Err(ServiceError::TaskUnavailable(format!(
            "项目不存在：{project_id}"
        )));
    }
    let changed = runtime
        .set_task_project(&request.task_id, request.project_id.as_deref())
        .map_err(|error| ServiceError::TaskUnavailable(error.to_string()))?;
    Ok(TaskSetProjectResponse {
        task_id: request.task_id,
        project_id: request.project_id,
        changed,
    })
}

/// 创建和更新共用的唯一校验：名字非空、查重（更新排除自己）、目录 ~ 展开后必须是绝对路径。
fn prepare_project(
    store: &tasks::ProjectStore,
    name: &str,
    goal: &str,
    dir: Option<&str>,
    exclude: Option<&str>,
) -> Result<tasks::NewProject, ServiceError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ServiceError::TaskUnavailable("项目名字不能为空".to_owned()));
    }
    if store.name_taken(name, exclude).map_err(project_error)? {
        return Err(ServiceError::TaskUnavailable(format!(
            "项目名字「{name}」已经存在"
        )));
    }
    Ok(tasks::NewProject {
        name: name.to_owned(),
        goal: goal.trim().to_owned(),
        dir: expand_dir(dir)?,
    })
}

/// 默认目录只要求 ~ 展开后是绝对路径；不要求目录已存在。
fn expand_dir(raw: Option<&str>) -> Result<Option<PathBuf>, ServiceError> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = if raw == "~" {
        home
    } else if let Some(rest) = raw.strip_prefix("~/") {
        Path::new(&home).join(rest).to_string_lossy().to_string()
    } else {
        raw.to_owned()
    };
    let path = PathBuf::from(expanded);
    if !path.is_absolute() {
        return Err(ServiceError::TaskUnavailable(format!(
            "默认目录要是绝对路径（可以用 ~）：{raw}"
        )));
    }
    Ok(Some(path))
}

fn view(project: &tasks::Project) -> ProjectView {
    ProjectView {
        id: project.id.clone(),
        name: project.name.clone(),
        goal: project.goal.clone(),
        dir: project
            .dir
            .as_ref()
            .map(|dir| dir.to_string_lossy().to_string()),
        created_at: project.created_at,
    }
}

fn project_error(error: tasks::ProjectError) -> ServiceError {
    ServiceError::TaskUnavailable(error.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
