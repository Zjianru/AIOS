//! T013.3：@ 文件候选（清单 5；DSH context/file-reference-local/tests/search.spec.ts 翻改）。
//!
//! 和 DSH 的不同（spec v1.2）：不列目录；排好序后按 git 规则判断文字，凑满 20 条或看完。

use std::fs;
use std::path::Path;

use tempfile::TempDir;
use tools::file_references::{
    FileReference, FileReferenceError, FileSearchConfig, list, list_with,
};

fn workspace() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        ("README.md", "readme"),
        ("src/terminal-view.ts", "terminal view"),
        ("src/tui.spec.ts", "tui spec"),
        ("src/deep/nested.ts", "nested"),
        ("docs/design notes.md", "design notes"),
        (".hidden/secret.txt", "secret"),
        ("node_modules/left-pad/index.js", "left pad"),
        ("dist/bundle.js", "bundle"),
        ("target/debug/out.bin", "target"),
        ("bin.dat", "\u{0}binary"),
    ];
    for (path, content) in files {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, content).unwrap();
    }
    dir
}

fn paths(references: &[FileReference]) -> Vec<String> {
    references
        .iter()
        .map(|reference| reference.path.clone())
        .collect()
}

// ── 遍历与过滤（清单 5）──────────────────────────────────────────

#[test]
fn lists_only_text_files_and_skips_excluded_directories() {
    let dir = workspace();
    let references = list(dir.path(), "").expect("列候选");
    let listed = paths(&references);
    assert!(
        listed.contains(&"README.md".to_owned()),
        "普通文件要出现：{listed:?}"
    );
    assert!(
        listed.contains(&"src/terminal-view.ts".to_owned()),
        "子目录里的文件要出现（递归遍历）：{listed:?}"
    );
    assert!(
        listed.contains(&"docs/design notes.md".to_owned()),
        "带空格的路径要出现：{listed:?}"
    );
    for excluded in ["node_modules", "dist", "target"] {
        assert!(
            !listed.iter().any(|path| path.starts_with(excluded)),
            "排除目录不能出现（{excluded}）：{listed:?}"
        );
    }
    assert!(
        !listed.iter().any(|path| path == "bin.dat"),
        "二进制文件不能出现（按 git 规则）：{listed:?}"
    );
    assert!(
        !listed.iter().any(|path| path == "src" || path == "docs"),
        "不列目录：{listed:?}"
    );
    assert!(
        !listed.iter().any(|path| path.starts_with(".hidden")),
        "默认不列隐藏路径：{listed:?}"
    );
}

#[test]
fn dot_queries_see_hidden_paths() {
    let dir = workspace();
    let listed = paths(&list(dir.path(), ".hidden").expect("列隐藏"));
    assert_eq!(listed, vec![".hidden/secret.txt".to_owned()]);
}

#[test]
fn query_with_slash_lists_that_directorys_files() {
    let dir = workspace();
    let listed = paths(&list(dir.path(), "src/").expect("列 src"));
    let mut expected = vec![
        "src/deep/nested.ts".to_owned(),
        "src/terminal-view.ts".to_owned(),
        "src/tui.spec.ts".to_owned(),
    ];
    expected.sort();
    let mut got = listed.clone();
    got.sort();
    assert_eq!(got, expected, "src/ 下所有文字文件（含子目录）");
    // 目录里的下一次查询按 fragment 排序。
    let listed = paths(&list(dir.path(), "src/tu").expect("列 src/tu"));
    assert!(
        listed.first().is_some_and(|path| path == "src/tui.spec.ts"),
        "fragment 命中名字前缀的排前面：{listed:?}"
    );
}

#[test]
fn outside_queries_and_bad_paths_return_empty() {
    let dir = workspace();
    for query in ["../", "..", "~/.missing/", "missing/"] {
        assert!(
            list(dir.path(), query).expect("查询").is_empty(),
            "{query:?} 不能列出东西"
        );
    }
}

// ── 排序（清单 5：同名 > 前缀 > 名字里有 > 路径里有 > 子序列）────────

#[test]
fn ranks_exact_prefix_name_path_then_subsequence() {
    let dir = tempfile::tempdir().unwrap();
    for path in [
        "src/deep/terminal.ts",
        "terminal-view.ts",
        "src/terminal.ts",
        "docs/terminal-guide.md",
        "src/t-e-r-m-i-n-a-l-not.ts",
    ] {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, "content").unwrap();
    }
    let listed = paths(&list(dir.path(), "terminal").expect("排序"));
    // 同名（terminal.ts）> 名字前缀（terminal-view.ts）> 名字里有（src/terminal.ts）
    // > 路径里有（docs/terminal-guide.md）> 子序列（t-e-r-m-i-n-a-l-not.ts）。
    assert_eq!(
        listed,
        vec![
            "src/terminal.ts".to_owned(),
            "terminal-view.ts".to_owned(),
            "src/deep/terminal.ts".to_owned(),
            "docs/terminal-guide.md".to_owned(),
            "src/t-e-r-m-i-n-a-l-not.ts".to_owned(),
        ]
    );
}

#[test]
fn empty_query_ranks_short_then_alphabetical() {
    let dir = tempfile::tempdir().unwrap();
    for path in ["bbb.ts", "a.ts", "cccc.ts"] {
        fs::write(dir.path().join(path), "x").unwrap();
    }
    assert_eq!(
        paths(&list(dir.path(), "").expect("空查询")),
        vec!["a.ts".to_owned(), "bbb.ts".to_owned(), "cccc.ts".to_owned()],
        "空查询按路径长度、再按字典序"
    );
}

// ── 上限（清单 5：最多 20；索引 5 万）────────────────────────────

#[test]
fn at_most_twenty_results() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..25 {
        fs::write(dir.path().join(format!("file-{index:02}.ts")), "x").unwrap();
    }
    let references = list(dir.path(), "file-").expect("列候选");
    assert_eq!(references.len(), 20, "最多 20 条");
}

#[test]
fn entry_cap_stops_indexing() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..10 {
        fs::write(dir.path().join(format!("a{index}.ts")), "x").unwrap();
    }
    let config = FileSearchConfig {
        max_entries: 3,
        ..FileSearchConfig::default()
    };
    let references = list_with(&config, dir.path(), "").expect("戴上上限");
    assert_eq!(references.len(), 3, "索引最多 3 条");
}

#[test]
fn invalid_config_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for config in [
        FileSearchConfig {
            max_results: 0,
            ..FileSearchConfig::default()
        },
        FileSearchConfig {
            max_entries: 0,
            ..FileSearchConfig::default()
        },
        FileSearchConfig {
            excluded_directories: vec!["a/b".to_owned()],
            ..FileSearchConfig::default()
        },
    ] {
        let error = list_with(&config, dir.path(), "").expect_err("坏配置要报错");
        assert!(
            matches!(error, FileReferenceError::Config(_)),
            "坏配置错误：{error}"
        );
    }
}

#[test]
fn unreadable_subtree_costs_only_its_candidates() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("README.md"), "readme").unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("sealed.ts"), "sealed").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(&locked, permissions).unwrap();
    }
    let listed = paths(&list(dir.path(), "").expect("读树"));
    assert!(
        listed.contains(&"README.md".to_owned()),
        "别的分支照常：{listed:?}"
    );
    assert!(
        !listed.iter().any(|path| path.contains("sealed")),
        "读不了的分支没有候选：{listed:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&locked, permissions).unwrap();
    }
}

#[test]
fn bytes_are_reported_for_each_candidate() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.ts"), "hello").unwrap();
    let references = list(dir.path(), "a").expect("列候选");
    assert_eq!(references[0].bytes, 5, "bytes 是内容字节数");
}
