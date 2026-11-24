//! 项目默认目录和资料库的 @ 候选：不列 Kind 启动/会话目录，返回绝对路径。

use std::path::Path;

use model::DEFAULT_CONTEXT_WINDOW;
use service::{Call, FileReferencesListRequest, RootHost};
use tempfile::tempdir;

fn project(data: &Path, dir: &Path) -> String {
    let store = tasks::ProjectStore::open(data.join(tasks::PROJECTS_DB_FILE)).expect("项目库");
    store
        .create(
            tasks::NewProject {
                name: "农技站".to_owned(),
                goal: "记农事台账".to_owned(),
                dir: Some(dir.to_path_buf()),
            },
            1_700_000_000_000,
        )
        .expect("建项目")
        .id
}

async fn host(data: &Path, startup_cwd: &Path) -> RootHost {
    let mut options = crate::common::options(data);
    options.cwd = startup_cwd.to_path_buf();
    RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await
}

fn paths(response: serde_json::Value) -> Vec<String> {
    response["references"]
        .as_array()
        .expect("references 数组")
        .iter()
        .map(|reference| reference["path"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn file_references_list_uses_project_dirs_not_the_startup_cwd() {
    let data = tempdir().unwrap();
    let project_dir = tempdir().unwrap();
    let startup = tempdir().unwrap();
    std::fs::write(project_dir.path().join("README.md"), "readme").unwrap();
    std::fs::create_dir_all(project_dir.path().join("src")).unwrap();
    std::fs::write(project_dir.path().join("src/a.ts"), "content").unwrap();
    std::fs::write(project_dir.path().join("bin.dat"), "\u{0}binary").unwrap();
    std::fs::create_dir_all(project_dir.path().join("node_modules/x")).unwrap();
    std::fs::write(project_dir.path().join("node_modules/x/index.js"), "x").unwrap();
    std::fs::write(startup.path().join("startup-only.md"), "never").unwrap();
    project(data.path(), project_dir.path());
    let host = host(data.path(), startup.path()).await;
    let response = host
        .dispatch(Call::FileReferencesList(FileReferencesListRequest {
            query: String::new(),
        }))
        .await
        .expect("file-references-list 应成功");
    let json = serde_json::to_value(&response).unwrap();
    let paths = paths(json);
    let readme = project_dir
        .path()
        .join("README.md")
        .to_string_lossy()
        .into_owned();
    let a_ts = project_dir
        .path()
        .join("src/a.ts")
        .to_string_lossy()
        .into_owned();
    assert!(paths.contains(&readme), "{paths:?}");
    assert!(paths.contains(&a_ts), "{paths:?}");
    assert!(
        paths.iter().all(|path| Path::new(path).is_absolute()),
        "{paths:?}"
    );
    assert!(
        !paths.iter().any(|path| path.contains("node_modules")),
        "排除目录不出现：{paths:?}"
    );
    assert!(
        !paths.iter().any(|path| path.ends_with("bin.dat")),
        "二进制不出现：{paths:?}"
    );
    assert!(
        !paths.iter().any(|path| path.contains("startup-only")),
        "不列启动目录：{paths:?}"
    );
}

#[tokio::test]
async fn file_references_list_returns_empty_for_outside_queries() {
    let data = tempdir().unwrap();
    let project_dir = tempdir().unwrap();
    std::fs::write(project_dir.path().join("a.ts"), "a").unwrap();
    project(data.path(), project_dir.path());
    let host = host(data.path(), project_dir.path()).await;
    let response = host
        .dispatch(Call::FileReferencesList(FileReferencesListRequest {
            query: "../".to_owned(),
        }))
        .await
        .expect("越界查询也应是正常空结果");
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["references"], serde_json::json!([]));
}

#[tokio::test]
async fn file_references_list_ranks_the_query_inside_the_project_dir() {
    let data = tempdir().unwrap();
    let project_dir = tempdir().unwrap();
    std::fs::write(project_dir.path().join("abc.ts"), "abc").unwrap();
    std::fs::write(project_dir.path().join("a-b-c.ts"), "abc").unwrap();
    std::fs::create_dir_all(project_dir.path().join("src")).unwrap();
    std::fs::write(project_dir.path().join("src/abc.ts"), "abc").unwrap();
    project(data.path(), project_dir.path());
    let host = host(data.path(), project_dir.path()).await;
    let response = host
        .dispatch(Call::FileReferencesList(FileReferencesListRequest {
            query: "abc".to_owned(),
        }))
        .await
        .expect("查询");
    let json = serde_json::to_value(&response).unwrap();
    let paths = paths(json);
    let expected: Vec<String> = ["abc.ts", "src/abc.ts", "a-b-c.ts"]
        .iter()
        .map(|name| project_dir.path().join(name).to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        paths, expected,
        "同名 > 名字前缀/路径里有（按长度）；子序列最后"
    );
}

#[tokio::test]
async fn file_references_list_includes_the_library() {
    let data = tempdir().unwrap();
    let project_dir = tempdir().unwrap();
    std::fs::create_dir_all(data.path().join("library")).unwrap();
    std::fs::write(data.path().join("library/book.md"), "book").unwrap();
    project(data.path(), project_dir.path());
    let host = host(data.path(), project_dir.path()).await;
    let response = host
        .dispatch(Call::FileReferencesList(FileReferencesListRequest {
            query: String::new(),
        }))
        .await
        .expect("查询");
    let paths = paths(serde_json::to_value(&response).unwrap());
    let book = data
        .path()
        .join("library/book.md")
        .to_string_lossy()
        .into_owned();
    assert!(paths.contains(&book), "{paths:?}");
}
