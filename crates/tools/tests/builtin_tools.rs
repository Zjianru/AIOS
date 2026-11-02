//! 接缝 2：内置工具的 Tool::execute，在临时目录上跑（测试清单第 13 条）。

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tools::{GlobTool, GrepTool, ReadTool, Tool, ToolCall, ToolContext, ToolResult, ToolSet};

async fn run(tool: &dyn Tool, context: &ToolContext, arguments: Value) -> ToolResult {
    tool.execute(
        ToolCall {
            id: "call_1".to_owned(),
            name: tool.name().to_owned(),
            arguments: arguments.to_string(),
        },
        context,
    )
    .await
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn set_modified(path: &Path, seconds: u64) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn read_tool_limits_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    write(&file, "alpha\nbeta\ngamma\n");
    let display = canonical(&file);
    let tool = ReadTool::new();

    // 正常：行号 + 尾注 + 绝对路径信封
    let result = run(&tool, &context, json!({"file_path": "a.txt"})).await;
    assert!(!result.is_error);
    assert_eq!(
        result.content,
        format!(
            "<path>{display}</path>\n<type>file</type>\n<content>\n1: alpha\n2: beta\n3: gamma\n\n(End of file - total 3 lines)\n</content>"
        )
    );

    // 正常：offset / limit 分页
    let result = run(
        &tool,
        &context,
        json!({"file_path": "a.txt", "offset": 2, "limit": 1}),
    )
    .await;
    assert!(result.content.contains("2: beta"));
    assert!(
        result
            .content
            .contains("(Showing lines 2-2 of 3. Use offset=3 to continue.)")
    );

    // 超上限：行数上限
    let result = run(
        &tool,
        &context,
        json!({"file_path": "a.txt", "limit": 5000}),
    )
    .await;
    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("limit must be less than or equal to 2000")
    );
    assert_eq!(result.error_code, None);

    // 超上限：大文件按行数截断
    let big = dir.path().join("big.txt");
    let lines: Vec<String> = (1..=3000).map(|number| format!("line{number}")).collect();
    write(&big, &format!("{}\n", lines.join("\n")));
    let result = run(&tool, &context, json!({"file_path": "big.txt"})).await;
    assert!(!result.is_error);
    assert!(result.content.contains("1: line1"));
    assert!(result.content.contains("2000: line2000"));
    assert!(
        result
            .content
            .contains("(Showing lines 1-2000 of 3000. Use offset=2001 to continue.)")
    );
    assert!(!result.content.contains("2001: line2001"));

    // 超上限：字节上限
    let wide = dir.path().join("wide.txt");
    let rows: Vec<String> = (1..=40)
        .map(|number| format!("{number}{}", "x".repeat(1990)))
        .collect();
    write(&wide, &format!("{}\n", rows.join("\n")));
    let result = run(&tool, &context, json!({"file_path": "wide.txt"})).await;
    assert!(!result.is_error);
    assert!(
        result
            .content
            .contains("(Output capped. Showing lines 1-25. Use offset=26 to continue.)")
    );

    // 正常：CRLF 行尾剥掉（照 DSH read-render 的 stripCarriageReturn）
    let crlf = dir.path().join("crlf.txt");
    std::fs::write(&crlf, b"one\r\ntwo\r\n").unwrap();
    let result = run(&tool, &context, json!({"file_path": "crlf.txt"})).await;
    assert!(!result.is_error);
    assert_eq!(
        result.content,
        format!(
            "<path>{}</path>\n<type>file</type>\n<content>\n1: one\n2: two\n\n(End of file - total 2 lines)\n</content>",
            canonical(&crlf)
        )
    );

    // 错误码：二进制文件（前 8192 字节含 NUL）→ FS_NOT_TEXT
    let binary = dir.path().join("bin.dat");
    std::fs::write(&binary, b"abc\0def\n").unwrap();
    let result = run(&tool, &context, json!({"file_path": "bin.dat"})).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_NOT_TEXT"));
    assert_eq!(
        result.content,
        format!("Error: cannot read \"{}\": binary file", canonical(&binary))
    );

    // 错误码：非法 UTF-8 → FS_NOT_TEXT（不是 FS_IO_ERROR）
    let invalid = dir.path().join("bad.txt");
    std::fs::write(&invalid, [0xFF_u8, 0xFE]).unwrap();
    let result = run(&tool, &context, json!({"file_path": "bad.txt"})).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_NOT_TEXT"));
    assert_eq!(
        result.content,
        format!(
            "Error: cannot read \"{}\": invalid UTF-8 text",
            canonical(&invalid)
        )
    );

    // 错误码：找不到、不是普通文件、offset 越界
    let result = run(&tool, &context, json!({"file_path": "missing.txt"})).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_NOT_FOUND"));
    assert!(result.content.starts_with("Error: cannot read \""));
    assert!(result.content.ends_with("\": not found"));

    let result = run(&tool, &context, json!({"file_path": "."})).await;
    assert_eq!(
        result.content,
        format!(
            "Error: cannot read \"{}\": not a regular file",
            canonical(dir.path())
        )
    );

    let result = run(&tool, &context, json!({"file_path": "a.txt", "offset": 10})).await;
    assert_eq!(result.error_code.as_deref(), Some("FS_NOT_FOUND"));
    assert_eq!(
        result.content,
        format!("Error: offset 10 is out of range for \"{display}\" (3 lines)")
    );

    // 参数错误：普通错误（无结构化错误码）
    let result = run(&tool, &context, json!({"file_path": "  "})).await;
    assert!(result.is_error);
    assert_eq!(
        result.content,
        "Error: file_path must be a non-empty string"
    );
    assert_eq!(result.error_code, None);
}

#[tokio::test]
async fn glob_tool_limits_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    write(&dir.path().join("src/alpha.ts"), "");
    write(&dir.path().join("src/beta.ts"), "");
    write(&dir.path().join("notes.md"), "");
    write(&dir.path().join(".hidden.ts"), "");
    write(&dir.path().join(".git/config.ts"), "");
    set_modified(&dir.path().join("src/alpha.ts"), 1_000);
    set_modified(&dir.path().join("src/beta.ts"), 2_000);
    set_modified(&dir.path().join(".hidden.ts"), 3_000);
    set_modified(&dir.path().join(".git/config.ts"), 4_000);
    let tool = GlobTool::new();

    // 正常：修改时间升序（ripgrep --sort=modified；最旧在前）、包含隐藏文件、排除 VCS 元数据
    let result = run(&tool, &context, json!({"pattern": "**/*.ts"})).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "src/alpha.ts\nsrc/beta.ts\n.hidden.ts");

    // 正常：path 收窄搜索根
    let result = run(&tool, &context, json!({"pattern": "*.ts", "path": "src"})).await;
    assert_eq!(result.content, "src/alpha.ts\nsrc/beta.ts");

    // 正常：path "." + 带 / 的模式——匹配用未加 ./ 的 cwd 相对路径，输出仍加 ./ 前缀
    let result = run(&tool, &context, json!({"pattern": "src/*.ts", "path": "."})).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "./src/alpha.ts\n./src/beta.ts");

    // 无匹配
    let result = run(&tool, &context, json!({"pattern": "*.rs"})).await;
    assert_eq!(result.content, "No files found");

    // 超上限：只显示前 100 条 + 恢复提示
    for index in 0..120 {
        write(&dir.path().join(format!("many/f{index:03}.txt")), "");
    }
    let result = run(&tool, &context, json!({"pattern": "many/*.txt"})).await;
    assert!(!result.is_error);
    let lines: Vec<&str> = result.content.split('\n').collect();
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("many/"))
            .count(),
        100
    );
    assert!(result.content.ends_with(
        "(Showing 100 of 120 paths. The complete result could not be saved; narrow pattern or path to see more.)"
    ));

    // 错误码：非法 pattern、访问不到的搜索根
    let result = run(&tool, &context, json!({"pattern": "["})).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("SEARCH_INVALID_PATTERN"));
    assert!(
        result
            .content
            .starts_with("Error: glob pattern rejected by ripgrep:")
    );

    let result = run(
        &tool,
        &context,
        json!({"pattern": "*.ts", "path": "missing"}),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("SEARCH_FAILED"));
    assert!(result.content.starts_with("Error: glob search failed:"));

    // 参数错误：普通错误
    let result = run(&tool, &context, json!({"pattern": "  "})).await;
    assert_eq!(result.content, "Error: pattern must be a non-empty string");
    assert_eq!(result.error_code, None);
}

#[tokio::test]
async fn grep_tool_limits_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    write(
        &dir.path().join("src/alpha.ts"),
        "export const alpha = 1\n// TODO: refit alpha\n",
    );
    write(&dir.path().join("notes.md"), "alpha appears here too\n");
    let tool = GrepTool::new();

    // 正常：按文件分组、行号前缀
    let result = run(&tool, &context, json!({"pattern": "alpha"})).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        result.content,
        "Found 3 matches\n\nnotes.md\nLine 1: alpha appears here too\n\nsrc/alpha.ts\nLine 1: export const alpha = 1\nLine 2: // TODO: refit alpha"
    );

    // 正常：include 收窄文件
    let result = run(
        &tool,
        &context,
        json!({"pattern": "alpha", "include": "*.ts"}),
    )
    .await;
    assert_eq!(
        result.content,
        "Found 2 matches\n\nsrc/alpha.ts\nLine 1: export const alpha = 1\nLine 2: // TODO: refit alpha"
    );

    // 无匹配
    let result = run(&tool, &context, json!({"pattern": "zzz"})).await;
    assert_eq!(result.content, "No matches found");

    // 超上限 + 行预览截断
    let many = dir.path().join("many.txt");
    let mut body = String::new();
    body.push_str(&format!("{}hit\n", "y".repeat(2100)));
    for number in 2..=300 {
        body.push_str(&format!("hit line {number}\n"));
    }
    write(&many, &body);
    let result = run(&tool, &context, json!({"pattern": "hit"})).await;
    assert!(!result.is_error);
    assert!(
        result
            .content
            .starts_with("Found 250 of 300 matches\n\nmany.txt\nLine 1: ")
    );
    assert!(result.content.contains(" (line truncated)"));
    assert!(result.content.ends_with(
        "(The complete result could not be saved; narrow pattern, path, or include to see more.)"
    ));

    // 错误码：非法正则、访问不到的搜索根
    let result = run(&tool, &context, json!({"pattern": "["})).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("SEARCH_INVALID_PATTERN"));
    assert!(
        result
            .content
            .starts_with("Error: grep pattern rejected by ripgrep:")
    );

    let result = run(
        &tool,
        &context,
        json!({"pattern": "alpha", "path": "missing"}),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("SEARCH_FAILED"));
    assert!(result.content.starts_with("Error: grep search failed:"));

    // 参数错误：普通错误（无结构化错误码）
    let result = run(&tool, &context, json!({"pattern": ""})).await;
    assert_eq!(result.content, "Error: pattern must be a non-empty string");
    assert_eq!(result.error_code, None);
    let result = run(&tool, &context, json!({"pattern": "x", "include": "!*.ts"})).await;
    assert_eq!(result.error_code, None);
    assert!(result.content.contains("negated patterns"));
}

/// T006.6：默认注册照 DSH tool-fs（applyPatch 默认 false，显式开启才注册）。
#[test]
fn builtin_default_registration_matches_dsh_apply_patch_default() {
    let default = ToolSet::builtin();
    for name in ["read", "glob", "grep", "write", "edit", "bash"] {
        assert!(default.get(name).is_some(), "默认工具集缺 {name}");
    }
    assert!(
        default.get("apply_patch").is_none(),
        "DSH tool-fs 的 applyPatch 默认 false：默认工具集不注册 apply_patch"
    );
    assert!(
        !default
            .definitions()
            .iter()
            .any(|schema| schema.name == "apply_patch"),
        "默认请求的 tools 列表也不带 apply_patch"
    );
    let with_patch = ToolSet::builtin_with_patch();
    assert!(
        with_patch.get("apply_patch").is_some(),
        "显式开启（等价 applyPatch: true）才注册 apply_patch"
    );
    assert_eq!(
        with_patch.definitions().len(),
        default.definitions().len() + 1,
        "其余默认工具不变"
    );
}
