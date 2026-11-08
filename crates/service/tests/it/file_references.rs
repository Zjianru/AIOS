//! T013.3：@ 文件候选的服务 op（清单 5）。

use model::DEFAULT_CONTEXT_WINDOW;
use service::{Call, FileReferencesListRequest, RootHost};
use tempfile::tempdir;

#[tokio::test]
async fn file_references_list_returns_text_files_of_the_session_cwd() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    std::fs::write(cwd.path().join("README.md"), "readme").unwrap();
    std::fs::create_dir_all(cwd.path().join("src")).unwrap();
    std::fs::write(cwd.path().join("src/a.ts"), "content").unwrap();
    std::fs::write(cwd.path().join("bin.dat"), "\u{0}binary").unwrap();
    std::fs::create_dir_all(cwd.path().join("node_modules/x")).unwrap();
    std::fs::write(cwd.path().join("node_modules/x/index.js"), "x").unwrap();
    let mut options = crate::common::options(data.path());
    options.cwd = cwd.path().to_path_buf();
    let host = RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await;
    let response = host
        .dispatch(Call::FileReferencesList(FileReferencesListRequest {
            query: String::new(),
        }))
        .await
        .expect("file-references-list 应成功");
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["method"], "file-references-list");
    let references = json["references"].as_array().expect("references 数组");
    let paths: Vec<&str> = references
        .iter()
        .map(|reference| reference["path"].as_str().unwrap_or_default())
        .collect();
    assert!(paths.contains(&"README.md"), "{paths:?}");
    assert!(paths.contains(&"src/a.ts"), "{paths:?}");
    assert!(
        !paths.iter().any(|path| path.starts_with("node_modules")),
        "排除目录不出现：{paths:?}"
    );
    assert!(!paths.contains(&"bin.dat"), "二进制不出现：{paths:?}");
    assert!(
        !paths.iter().any(|path| path == &"src"),
        "不列目录：{paths:?}"
    );
    let first = &references[0];
    assert!(first["bytes"].as_u64().is_some(), "每条带 bytes：{first}");
}

#[tokio::test]
async fn file_references_list_returns_empty_for_outside_queries() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    std::fs::write(cwd.path().join("a.ts"), "a").unwrap();
    let mut options = crate::common::options(data.path());
    options.cwd = cwd.path().to_path_buf();
    let host = RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await;
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
async fn file_references_list_ranks_the_query() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    std::fs::write(cwd.path().join("abc.ts"), "abc").unwrap();
    std::fs::write(cwd.path().join("a-b-c.ts"), "abc").unwrap();
    std::fs::create_dir_all(cwd.path().join("src")).unwrap();
    std::fs::write(cwd.path().join("src/abc.ts"), "abc").unwrap();
    let mut options = crate::common::options(data.path());
    options.cwd = cwd.path().to_path_buf();
    let host = RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await;
    let response = host
        .dispatch(Call::FileReferencesList(FileReferencesListRequest {
            query: "abc".to_owned(),
        }))
        .await
        .expect("查询");
    let json = serde_json::to_value(&response).unwrap();
    let paths: Vec<&str> = json["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reference| reference["path"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        paths,
        vec!["abc.ts", "src/abc.ts", "a-b-c.ts"],
        "同名 > 名字前缀/路径里有（按长度）；子序列最后"
    );
}
