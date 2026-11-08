//! 接缝 2：skill 发现与加载（测试清单第 6 条）。

use std::path::Path;

use serde_json::json;
use tools::{SkillTool, ToolContext, ToolResult, discover_skills};

use crate::common::{run, session_context};

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

async fn load_skill(name: &str, context: &ToolContext) -> ToolResult {
    run(&SkillTool::new(), json!({"name": name}), context).await
}

const PDF_SKILL: &str = "---\nname: pdf-tools\ndescription: Handle PDF files.\n---\n# PDF tools\nUse the pdf command.\n";

#[tokio::test]
async fn skill_tool_loads_full_text_from_project_root() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/pdf-tools/SKILL.md"),
        PDF_SKILL,
    );
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(project.path(), data_dir.path());

    let result = load_skill("pdf-tools", &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("Use the pdf command."),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("Base directory for this skill"),
        "{}",
        result.content
    );
    assert!(result.content.contains("pdf-tools"), "{}", result.content);
}

#[tokio::test]
async fn skill_tool_loads_from_user_data_dir_root() {
    let cwd = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    write(
        &data_dir.path().join("skills/user-here/SKILL.md"),
        "---\nname: user-here\ndescription: A user skill.\n---\nUser skill body.\n",
    );
    let context = session_context(cwd.path(), data_dir.path());

    let result = load_skill("user-here", &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("User skill body."),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn skill_tool_loads_from_configured_root() {
    let cwd = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let configured = tempfile::tempdir().unwrap();
    write(
        &configured.path().join("from-config/SKILL.md"),
        "---\nname: from-config\ndescription: From config root.\n---\nConfig body.\n",
    );
    let context = session_context(cwd.path(), data_dir.path());
    context
        .session()
        .set_skill_roots(vec![configured.path().to_path_buf()]);

    let result = load_skill("from-config", &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("Config body."),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn invalid_frontmatter_skills_are_discarded() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let skills = project.path().join(".agents/skills");
    write(
        &skills.join("good/SKILL.md"),
        "---\nname: good\ndescription: Good skill.\n---\nGood body.\n",
    );
    // 缺 description
    write(
        &skills.join("nodesc/SKILL.md"),
        "---\nname: nodesc\n---\nBody.\n",
    );
    // 名字不合法（非 kebab-case）
    write(
        &skills.join("badname/SKILL.md"),
        "---\nname: Bad_Name\ndescription: Bad.\n---\nBody.\n",
    );
    // YAML frontmatter 坏掉（没有结束分隔线）
    write(
        &skills.join("badyaml/SKILL.md"),
        "---\nname: badyaml\ndescription: Bad yaml.\nBody without closing fence.\n",
    );
    let data_dir = tempfile::tempdir().unwrap();
    let discovered = discover_skills(project.path(), data_dir.path(), &[], &[]);
    let names: Vec<&str> = discovered.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["good"], "非法 frontmatter 的 skill 不应进目录");

    let context = session_context(project.path(), data_dir.path());
    let invalid_name = load_skill("Bad_Name", &context).await;
    assert!(invalid_name.is_error, "{}", invalid_name.content);
    assert!(
        invalid_name.content.contains("invalid skill name"),
        "{}",
        invalid_name.content
    );
    let missing = load_skill("nodesc", &context).await;
    assert!(missing.is_error, "{}", missing.content);
    assert!(
        missing.content.contains("unknown or no longer available"),
        "{}",
        missing.content
    );
}

#[tokio::test]
async fn unknown_skill_name_is_error() {
    let cwd = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(cwd.path(), data_dir.path());
    let result = load_skill("no-such-skill", &context).await;
    assert!(result.is_error);
    assert!(
        result.content.contains("unknown or no longer available"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn project_skill_shadows_user_skill_with_same_name() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/dup/SKILL.md"),
        "---\nname: dup\ndescription: From project.\n---\nProject body.\n",
    );
    let cwd = project.path();
    let data_dir = tempfile::tempdir().unwrap();
    write(
        &data_dir.path().join("skills/dup/SKILL.md"),
        "---\nname: dup\ndescription: From user.\n---\nUser body.\n",
    );
    let discovered = discover_skills(cwd, data_dir.path(), &[], &[]);
    let dup = discovered.iter().find(|skill| skill.name == "dup");
    assert!(dup.is_some(), "dup 应被发现");
    assert_eq!(dup.unwrap().description, "From project.", "项目根优先");
}

#[tokio::test]
async fn flat_markdown_skill_is_discovered() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/flat-skill.md"),
        "---\nname: flat-skill\ndescription: Flat file skill.\n---\nFlat body.\n",
    );
    let data_dir = tempfile::tempdir().unwrap();
    let discovered = discover_skills(project.path(), data_dir.path(), &[], &[]);
    assert!(
        discovered.iter().any(|skill| skill.name == "flat-skill"),
        "平铺 <name>.md 也应被发现"
    );
}

#[test]
fn skill_roots_never_include_dsh_home() {
    let cwd = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let configured = tempfile::tempdir().unwrap();
    let roots = tools::skill_roots(
        cwd.path(),
        data_dir.path(),
        &[configured.path().to_path_buf()],
    );
    assert_eq!(roots.len(), 3, "项目根 + data-dir + 配置根");
    for root in &roots {
        assert!(
            !root.display().to_string().contains(".dsh"),
            "不许读 ~/.dsh：{}",
            root.display()
        );
    }
    assert!(roots[1].starts_with(data_dir.path()));
    assert!(roots[2].starts_with(configured.path()));
}

#[tokio::test]
async fn malformed_quotes_and_flow_frontmatter_are_discarded() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let skills = project.path().join(".agents/skills");
    write(
        &skills.join("good/SKILL.md"),
        "---\nname: good\ndescription: Good skill.\n---\nGood body.\n",
    );
    // 引号不闭合：不是合法 YAML，整个 skill 丢弃。
    write(
        &skills.join("badquote/SKILL.md"),
        "---\nname: badquote\ndescription: \"unterminated\n---\nBody.\n",
    );
    // flow 序列不闭合：同样丢弃。
    write(
        &skills.join("badflow/SKILL.md"),
        "---\nname: badflow\ndescription: [unclosed\n---\nBody.\n",
    );
    let data_dir = tempfile::tempdir().unwrap();
    let discovered = discover_skills(project.path(), data_dir.path(), &[], &[]);
    let names: Vec<&str> = discovered.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["good"], "坏引号/坏 flow 的 skill 不应进目录");

    let context = session_context(project.path(), data_dir.path());
    for name in ["badquote", "badflow"] {
        let result = load_skill(name, &context).await;
        assert!(result.is_error, "{name} 不应能加载");
        assert!(
            result.content.contains("unknown or no longer available"),
            "{}",
            result.content
        );
    }
}

/// T006.11b：/name 手势照 DSH SKILL_GESTURE——空白边界、kebab-case 名字；
/// /usr/bin、5/8、大小写不合法与重复都不算/去重。
#[test]
fn user_invocation_names_follow_dsh_gesture() {
    assert_eq!(tools::invoked_skill_names("/alpha 做事"), vec!["alpha"]);
    assert_eq!(
        tools::invoked_skill_names("请用 /alpha 和 /beta 再 /alpha"),
        vec!["alpha", "beta"]
    );
    assert!(
        tools::invoked_skill_names("/usr/bin 5/8 x/y /UPPER //double").is_empty(),
        "路径/分数/大写/双斜杠都不是手势"
    );
}

/// T014.2：程序自带的 skill 排在用户 skill 后面；用户放一个同名的，目录里只有用户那份。
#[tokio::test]
async fn bundled_skills_come_after_user_skills() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let catalog = discover_skills(
        project.path(),
        data_dir.path(),
        &[],
        tools::bundled_skills(),
    );
    let bundled = catalog
        .iter()
        .find(|skill| skill.name == "aios-settings")
        .expect("程序自带 aios-settings");
    assert!(
        !bundled.description.is_empty(),
        "目录里要有这一句用途：{}",
        bundled.description
    );
    assert!(
        bundled.base_dir.as_os_str().is_empty(),
        "编进二进制的 skill 没有目录"
    );
    let context = session_context(project.path(), data_dir.path());
    context
        .shared_session()
        .set_bundled_skills(tools::bundled_skills().to_vec());
    let result = load_skill("aios-settings", &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        !result.content.contains("Base directory for this skill"),
        "自带 skill 没有目录，不写按基目录解析的提示：{}",
        result.content
    );

    // 用户放一份同名的：用户的赢，目录里只有一条，正文是用户的。
    write(
        &project.path().join(".agents/skills/aios-settings/SKILL.md"),
        "---\nname: aios-settings\ndescription: 用户自己的。\n---\nUser body.\n",
    );
    let catalog = discover_skills(
        project.path(),
        data_dir.path(),
        &[],
        tools::bundled_skills(),
    );
    let matches: Vec<_> = catalog
        .iter()
        .filter(|skill| skill.name == "aios-settings")
        .collect();
    assert_eq!(matches.len(), 1, "重名时只留一条：{catalog:?}");
    assert_eq!(matches[0].description, "用户自己的。", "重名时用户的赢");
    let result = load_skill("aios-settings", &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("User body."),
        "加载到的是用户那份：{}",
        result.content
    );
}

/// T014.2：自带 skill 的正文要写清什么时候查、什么时候提议、一轮一张卡、只说一句为什么。
#[test]
fn bundled_aios_settings_body_covers_the_four_rules() {
    let cwd = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let skill = tools::load_skill(
        cwd.path(),
        data_dir.path(),
        &[],
        tools::bundled_skills(),
        "aios-settings",
    )
    .expect("加载自带 aios-settings");
    for needle in [
        "aios_query",
        "propose_action",
        "One card per turn",
        "one sentence on why",
    ] {
        assert!(
            skill.body.contains(needle),
            "正文要写到 {needle}：\n{}",
            skill.body
        );
    }
}

/// T006.11b：用户显式通道只认 user-invocable，不看 disable-model-invocation
/// （DSH：这是隐藏 skill 的唯一入口）；未知名字静默返回 None。
#[test]
fn user_invocable_loader_ignores_model_policy_and_honors_user_policy() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/hidden/SKILL.md"),
        "---\nname: hidden\ndescription: H.\ndisable-model-invocation: true\n---\nHidden body.\n",
    );
    write(
        &project.path().join(".agents/skills/quiet/SKILL.md"),
        "---\nname: quiet\ndescription: Q.\nuser-invocable: false\n---\nQuiet body.\n",
    );
    write(
        &project.path().join(".agents/skills/normal/SKILL.md"),
        "---\nname: normal\ndescription: N.\n---\nNormal body.\n",
    );
    let data_dir = tempfile::tempdir().unwrap();
    let hidden =
        tools::load_user_invocable_skill(project.path(), data_dir.path(), &[], &[], "hidden");
    assert!(
        hidden.is_some(),
        "disable-model-invocation 不影响用户显式调用"
    );
    let quiet =
        tools::load_user_invocable_skill(project.path(), data_dir.path(), &[], &[], "quiet");
    assert!(quiet.is_none(), "user-invocable: false 不注入");
    assert!(
        tools::load_user_invocable_skill(project.path(), data_dir.path(), &[], &[], "nope")
            .is_none(),
        "未知名字静默跳过"
    );
    let normal =
        tools::load_user_invocable_skill(project.path(), data_dir.path(), &[], &[], "normal");
    assert_eq!(normal.unwrap().body, "Normal body.");
}
