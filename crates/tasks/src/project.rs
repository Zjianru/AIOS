//! T079：项目对象的最小存储。内核只存 id/name/goal/dir/created_at；
//! 首字 code、颜色和页面统计都不进库。

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// 项目库文件名（放在 AIOS 数据目录）。
pub const PROJECTS_DB_FILE: &str = "projects.sqlite";

/// 一个项目；旧行没有 dir 时按 None 读。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    pub created_at: i64,
}

/// 新建项目。校验在服务端一处做（名字、重名、目录），存储只负责落库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject {
    pub name: String,
    pub goal: String,
    pub dir: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("项目数据目录或文件不可用: {0}")]
    Io(#[from] std::io::Error),
    #[error("SQLite 错误: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("项目连接锁已中毒")]
    Poisoned,
    #[error("项目名字不能为空")]
    EmptyName,
    #[error("项目名字「{0}」已经存在")]
    DuplicateName(String),
    #[error("项目不存在: {0}")]
    ProjectNotFound(String),
}

/// 项目库；单独一个 SQLite 文件，和任务库同一个数据目录。
pub struct ProjectStore {
    conn: Mutex<Connection>,
}

impl ProjectStore {
    pub fn open(path: std::path::PathBuf) -> Result<Self, ProjectError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS project (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                goal TEXT NOT NULL,
                dir TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS project_created_at ON project(created_at);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, ProjectError> {
        self.conn.lock().map_err(|_| ProjectError::Poisoned)
    }

    pub fn list(&self) -> Result<Vec<Project>, ProjectError> {
        let conn = self.lock()?;
        let mut statement = conn.prepare(
            "SELECT id, name, goal, dir, created_at FROM project ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], row_to_project)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get(&self, id: &str) -> Result<Option<Project>, ProjectError> {
        let conn = self.lock()?;
        Ok(conn
            .query_row(
                "SELECT id, name, goal, dir, created_at FROM project WHERE id = ?1",
                params![id],
                row_to_project,
            )
            .optional()?)
    }

    /// 名字是否已被别的项目占用（改名时排除自己）。
    pub fn name_taken(&self, name: &str, exclude: Option<&str>) -> Result<bool, ProjectError> {
        let conn = self.lock()?;
        let found: Option<String> = conn
            .query_row(
                "SELECT id FROM project WHERE name = ?1 AND id <> COALESCE(?2, '')",
                params![name, exclude],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn create(&self, draft: NewProject, now: i64) -> Result<Project, ProjectError> {
        let name = draft.name.trim().to_owned();
        if name.is_empty() {
            return Err(ProjectError::EmptyName);
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction()?;
        let taken: Option<String> = transaction
            .query_row(
                "SELECT id FROM project WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(ProjectError::DuplicateName(name));
        }
        let id: String =
            transaction.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
        transaction.execute(
            "INSERT INTO project (id, name, goal, dir, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                name,
                draft.goal.trim(),
                draft
                    .dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().to_string()),
                now
            ],
        )?;
        transaction.commit()?;
        Ok(Project {
            id,
            name,
            goal: draft.goal.trim().to_owned(),
            dir: draft.dir,
            created_at: now,
        })
    }

    pub fn update(&self, id: &str, draft: NewProject) -> Result<Project, ProjectError> {
        let name = draft.name.trim().to_owned();
        if name.is_empty() {
            return Err(ProjectError::EmptyName);
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT id, name, goal, dir, created_at FROM project WHERE id = ?1",
                params![id],
                row_to_project,
            )
            .optional()?
            .ok_or_else(|| ProjectError::ProjectNotFound(id.to_owned()))?;
        let taken: Option<String> = transaction
            .query_row(
                "SELECT id FROM project WHERE name = ?1 AND id <> ?2",
                params![name, id],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(ProjectError::DuplicateName(name));
        }
        transaction.execute(
            "UPDATE project SET name = ?1, goal = ?2, dir = ?3 WHERE id = ?4",
            params![
                name,
                draft.goal.trim(),
                draft
                    .dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().to_string()),
                id
            ],
        )?;
        transaction.commit()?;
        Ok(Project {
            id: existing.id,
            name,
            goal: draft.goal.trim().to_owned(),
            dir: draft.dir,
            created_at: existing.created_at,
        })
    }
}

fn row_to_project(row: &rusqlite::Row<'_>) -> Result<Project, rusqlite::Error> {
    let dir: Option<String> = row.get(3)?;
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        goal: row.get(2)?,
        dir: dir.map(PathBuf::from),
        created_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ProjectStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = ProjectStore::open(directory.path().join(PROJECTS_DB_FILE)).unwrap();
        (directory, store)
    }

    fn draft(name: &str) -> NewProject {
        NewProject {
            name: name.to_owned(),
            goal: "做一件事".to_owned(),
            dir: None,
        }
    }

    #[test]
    fn creates_lists_and_updates_a_project() {
        let (_directory, store) = store();
        let created = store.create(draft("清河县农技站"), 100).unwrap();
        assert_eq!(created.name, "清河县农技站");
        assert_eq!(created.created_at, 100);
        assert_eq!(store.list().unwrap(), vec![created.clone()]);
        let updated = store
            .update(
                &created.id,
                NewProject {
                    name: "清河农技站".to_owned(),
                    goal: "换一个目标".to_owned(),
                    dir: Some(PathBuf::from("/tmp/项目")),
                },
            )
            .unwrap();
        assert_eq!(updated.name, "清河农技站");
        assert_eq!(updated.goal, "换一个目标");
        assert_eq!(updated.dir, Some(PathBuf::from("/tmp/项目")));
        assert_eq!(store.get(&created.id).unwrap(), Some(updated));
    }

    #[test]
    fn duplicate_and_empty_names_are_rejected() {
        let (_directory, store) = store();
        store.create(draft("同名"), 1).unwrap();
        assert!(matches!(
            store.create(draft("同名"), 2),
            Err(ProjectError::DuplicateName(_))
        ));
        assert!(matches!(
            store.create(draft("  "), 3),
            Err(ProjectError::EmptyName)
        ));
        let other = store.create(draft("别的"), 4).unwrap();
        assert!(matches!(
            store.update(&other.id, draft("同名")),
            Err(ProjectError::DuplicateName(_))
        ));
        // 改名成自己现有的名字允许。
        store.update(&other.id, draft("别的")).unwrap();
    }
}

#[cfg(test)]
mod task_project_tests {
    use crate::{ConversationId, ModelChoice, NewGraph, NewSubtask, NewTask, Store, TaskPolicy};

    fn task(cwd: &std::path::Path, title: &str) -> NewTask {
        NewTask {
            title: title.to_owned(),
            requirement: "做".to_owned(),
            dod: "做完".to_owned(),
            cwd: cwd.to_path_buf(),
            conversation: ConversationId::new("kind-project-test"),
            model: ModelChoice {
                connection: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                effort_wanted: "high".to_owned(),
            },
            tools: vec!["read".to_owned()],
        }
    }

    fn sub(cwd: &std::path::Path, title: &str) -> NewSubtask {
        NewSubtask {
            task: task(cwd, title),
            policy: TaskPolicy {
                write_scope: None,
                worktree: false,
                bash: false,
            },
            after: Vec::new(),
            milestone: false,
            program_gate: Vec::new(),
        }
    }

    #[test]
    fn task_project_is_optional_and_graph_children_follow_the_root() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
        let single = store
            .create_task(task(directory.path(), "个人"), 1)
            .unwrap();
        assert_eq!(single.project_id, None, "旧/默认任务归个人");

        let graph = store
            .create_graph(
                NewGraph {
                    root: task(directory.path(), "总任务"),
                    subtasks: vec![sub(directory.path(), "甲"), sub(directory.path(), "乙")],
                },
                2,
            )
            .unwrap();
        let children = store.children(&graph.id).unwrap();
        assert_eq!(children.len(), 2);
        store.set_project(&graph.id, Some("p1")).unwrap();
        let root = store.task(&graph.id).unwrap().unwrap();
        assert_eq!(root.project_id.as_deref(), Some("p1"));
        for child in store.children(&graph.id).unwrap() {
            assert_eq!(child.project_id.as_deref(), Some("p1"), "子任务继承总任务");
        }

        // 改任一子任务，整张图一起改；单派任务不动。
        let child = children[0].id.clone();
        store.set_project(&child, Some("p2")).unwrap();
        assert_eq!(
            store.task(&child).unwrap().unwrap().project_id.as_deref(),
            Some("p2")
        );
        assert_eq!(
            store
                .task(&graph.id)
                .unwrap()
                .unwrap()
                .project_id
                .as_deref(),
            Some("p2")
        );
        assert_eq!(store.task(&single.id).unwrap().unwrap().project_id, None);
        store.set_project(&graph.id, None).unwrap();
        for child in store.children(&graph.id).unwrap() {
            assert_eq!(child.project_id, None, "清空也整张图");
        }
    }
}
