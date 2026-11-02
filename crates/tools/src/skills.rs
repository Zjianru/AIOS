//! skill 发现、frontmatter 解析与加载（照 DSH skill/skill-filesystem 的可观察语义做最小实现）。
//!
//! 根优先级（后者被前者遮蔽）：项目根 `/.agents/skills`、`<data-dir>/skills`、配置的 skill_roots。
//! 每个根只看顶层：目录 bundle `<name>/SKILL.md` 或平铺 `<name>.md`；frontmatter 必填 name/description，
//! 名字必须是 kebab-case；解析失败或不合法的 skill 直接丢弃（工具也拒绝加载）。
//! 不读 `~/.dsh`：根只来自项目根、data-dir 和配置。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ToolFailure;

/// skill bundle 的入口文件名。
pub const SKILL_FILE: &str = "SKILL.md";
/// 项目根下的 skill 根（照 DSH project-agents）。
pub const PROJECT_SKILLS: &str = ".agents/skills";
/// 项目根标记。
pub const PROJECT_ROOT_MARKER: &str = ".git";
/// 目录里渲染的 description 上限（照 DSH catalogDescriptionMaxLength 默认 500）。
pub const CATALOG_DESCRIPTION_MAX: usize = 500;

/// 一条可用 skill 的目录摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub base_dir: PathBuf,
}

/// 加载后的 skill：摘要 + 全文 + 来源目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillContent {
    pub summary: SkillSummary,
    pub body: String,
}

struct Discovered {
    summary: SkillSummary,
    body: String,
}

/// 会话可用的 skill 根：项目根/.agents/skills、<data-dir>/skills、配置根（去重、保序）。
pub fn skill_roots(cwd: &Path, data_dir: &Path, configured: &[PathBuf]) -> Vec<PathBuf> {
    let project_root = find_project_root(cwd);
    let mut roots = vec![project_root.join(PROJECT_SKILLS), data_dir.join("skills")];
    for root in configured {
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    roots
}

/// 发现所有可用 skill：按根顺序扫描，同名（先出现者胜）只保留一个。
pub fn discover_skills(cwd: &Path, data_dir: &Path, configured: &[PathBuf]) -> Vec<SkillSummary> {
    discover(cwd, data_dir, configured)
        .into_iter()
        .map(|found| found.summary)
        .collect()
}

/// 按名字加载 skill 全文；名字非法或找不到返回错误。
pub fn load_skill(
    cwd: &Path,
    data_dir: &Path,
    configured: &[PathBuf],
    name: &str,
) -> Result<SkillContent, ToolFailure> {
    if !is_skill_name(name) {
        return Err(ToolFailure::plain(format!("invalid skill name \"{name}\"")));
    }
    discover(cwd, data_dir, configured)
        .into_iter()
        .find(|found| found.summary.name == name)
        .map(|found| SkillContent {
            summary: found.summary,
            body: found.body,
        })
        .ok_or_else(|| {
            ToolFailure::plain(format!(
                "skill \"{name}\" is unknown or no longer available"
            ))
        })
}

/// skill 名是否合法：kebab-case（`[a-z0-9]+(-[a-z0-9]+)*`）。
pub fn is_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

/// 渲染 `<skill_content>` 块（加载工具返回给模型的正文 + 来源目录）。
pub fn render_skill_content(skill: &SkillContent) -> String {
    format!(
        "<skill_content name=\"{}\">\n<skill_resources>\nBase directory for this skill: {}\nResolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.\n</skill_resources>\n\n<skill_instructions>\n{}\n</skill_instructions>\n</skill_content>",
        skill.summary.name,
        skill.summary.base_dir.display(),
        skill.body.trim_end()
    )
}

/// 目录里的一行描述：压成单行、限制长度（超长加省略号），并转义 system-reminder 结束标签。
pub fn catalog_description(description: &str) -> String {
    let single_line: String = description
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ");
    let truncated: String = single_line.chars().take(CATALOG_DESCRIPTION_MAX).collect();
    let mut rendered = if single_line.chars().count() > CATALOG_DESCRIPTION_MAX {
        format!("{truncated}…")
    } else {
        truncated
    };
    rendered = rendered.replace("</system-reminder>", "<\\/system-reminder>");
    rendered
}

fn discover(cwd: &Path, data_dir: &Path, configured: &[PathBuf]) -> Vec<Discovered> {
    let mut found: BTreeMap<String, Discovered> = BTreeMap::new();
    for root in skill_roots(cwd, data_dir, configured) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for entry in entries {
            let name = entry
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            let (skill_file, base_dir) = if entry.is_dir() {
                (entry.join(SKILL_FILE), entry.clone())
            } else if name.ends_with(".md") {
                (entry.clone(), root.clone())
            } else {
                continue;
            };
            let Ok(raw) = std::fs::read_to_string(&skill_file) else {
                continue;
            };
            let Ok(parsed) = parse_skill(&raw) else {
                continue;
            };
            if found.contains_key(&parsed.name) {
                continue;
            }
            found.insert(
                parsed.name.clone(),
                Discovered {
                    summary: SkillSummary {
                        name: parsed.name,
                        description: parsed.description,
                        path: skill_file,
                        base_dir,
                    },
                    body: parsed.body,
                },
            );
        }
    }
    found.into_values().collect()
}

struct ParsedSkill {
    name: String,
    description: String,
    body: String,
}

/// 解析 SKILL.md 的 YAML frontmatter：只支持 name/description（普通标量、引号、`|`/`>` 块标量），
/// 其余键忽略；缺分隔线、缺字段、名字不合法都算解析失败。
fn parse_skill(raw: &str) -> Result<ParsedSkill, String> {
    let mut lines = raw.lines();
    let first = lines.next().ok_or("missing YAML frontmatter")?;
    if first.trim() != "---" {
        return Err("missing YAML frontmatter".to_owned());
    }
    let mut frontmatter: Vec<&str> = Vec::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines {
        if !closed {
            if line.trim() == "---" {
                closed = true;
            } else {
                frontmatter.push(line);
            }
        } else {
            body_lines.push(line);
        }
    }
    if !closed {
        return Err("unterminated YAML frontmatter".to_owned());
    }
    let fields = parse_simple_yaml(&frontmatter)?;
    let name = fields.get("name").cloned().ok_or("missing name")?;
    let description = fields
        .get("description")
        .cloned()
        .ok_or("missing description")?;
    for (field, value) in [("name", &name), ("description", &description)] {
        if value.starts_with('[') || value.starts_with('{') {
            return Err(format!("frontmatter field {field} must be a scalar string"));
        }
    }
    if !is_skill_name(&name) {
        return Err(format!("invalid skill name \"{name}\""));
    }
    if description.trim().is_empty() {
        return Err("empty description".to_owned());
    }
    Ok(ParsedSkill {
        name,
        description: description.trim().to_owned(),
        body: body_lines.join("\n").trim().to_owned(),
    })
}

/// 块标量记号（允许 YAML 的 chomping/indent 修饰：|-、>+、|2 等）。
fn block_scalar_marker(value: &str) -> Option<char> {
    let trimmed =
        value.trim_end_matches(|character: char| matches!(character, '+' | '-' | '0'..='9'));
    match trimmed {
        "|" => Some('|'),
        ">" => Some('>'),
        _ => None,
    }
}

/// 双引号标量内不能有未转义的引号。
fn has_unescaped_double_quote(inner: &str) -> bool {
    let mut escaped = false;
    for character in inner.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => return true,
            _ => {}
        }
    }
    escaped
}

fn parse_simple_yaml(lines: &[&str]) -> Result<BTreeMap<String, String>, String> {
    let mut fields = BTreeMap::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            return Err(format!("invalid frontmatter line: {trimmed}"));
        };
        let key = key.trim().to_owned();
        let value = value.trim();
        let parsed = if let Some(marker) = block_scalar_marker(value) {
            let folded = marker == '>';
            let mut block: Vec<&str> = Vec::new();
            while index < lines.len() {
                let candidate = lines[index];
                let indented = candidate.starts_with(' ') || candidate.starts_with('\t');
                if !indented && !candidate.trim().is_empty() {
                    break;
                }
                block.push(candidate.trim());
                index += 1;
            }
            if folded {
                block.join(" ")
            } else {
                block.join("\n")
            }
        } else if value.starts_with('"') || value.starts_with('\'') {
            let quote = value.chars().next().expect("quote");
            if value.len() < 2 || !value.ends_with(quote) {
                return Err(format!("unterminated quoted scalar: {value}"));
            }
            let inner = &value[1..value.len() - 1];
            if quote == '"' {
                if has_unescaped_double_quote(inner) {
                    return Err(format!("unescaped quote in scalar: {value}"));
                }
            } else if inner.contains('\'') {
                return Err(format!("unsupported single-quote escape: {value}"));
            }
            inner.to_owned()
        } else if value.starts_with('[') || value.starts_with('{') {
            let closing = if value.starts_with('[') { ']' } else { '}' };
            if !value.ends_with(closing) {
                return Err(format!("unterminated flow collection: {value}"));
            }
            // 闭合的 flow 集合不是字符串标量：保留原文，由消费字段的类型检查拒绝。
            value.to_owned()
        } else {
            value.to_owned()
        };
        fields.insert(key, parsed);
    }
    Ok(fields)
}

/// skill frontmatter 的完整字段（DSH skill-filesystem 的 ParsedSkill）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub invocation: SkillInvocation,
}

/// 模型/用户两侧调用策略（DSH SkillInvocationPolicy 的子集：AIOS 没有用户显式 /name 通道）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillInvocation {
    pub model_invocable: bool,
    pub user_invocable: bool,
}

impl Default for SkillInvocation {
    fn default() -> Self {
        Self {
            model_invocable: true,
            user_invocable: true,
        }
    }
}

/// 一条发现警告（DSH 对非法 skill 打 logger.warn，不静默丢）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillWarning {
    pub path: PathBuf,
    pub message: String,
}

/// 一次发现的完整结果：可用 skill + 警告（DSH candidates/incomplete 的 AIOS 形状）。
#[derive(Debug, Default)]
pub struct SkillDiscovery {
    pub skills: Vec<SkillSummary>,
    pub warnings: Vec<SkillWarning>,
}

/// 解析 frontmatter 为完整字段。骨架阶段先沿用最小解析（只 name/description），
/// 完整 YAML/字段/调用策略在 T006.11 实现。
pub fn parse_skill_frontmatter(raw: &str) -> Result<SkillFrontmatter, String> {
    let parsed = parse_skill(raw)?;
    Ok(SkillFrontmatter {
        name: parsed.name,
        description: parsed.description,
        when_to_use: None,
        metadata: None,
        invocation: SkillInvocation::default(),
    })
}

/// 发现 skill + 警告（骨架阶段行为同 discover_skills，warnings 恒空）。
pub fn discover_skill_catalog(cwd: &Path, data_dir: &Path, configured: &[PathBuf]) -> SkillDiscovery {
    SkillDiscovery {
        skills: discover_skills(cwd, data_dir, configured),
        warnings: Vec::new(),
    }
}

fn find_project_root(cwd: &Path) -> PathBuf {
    let mut current = cwd.to_path_buf();
    loop {
        if current.join(PROJECT_ROOT_MARKER).exists() {
            return current;
        }
        let Some(parent) = current.parent() else {
            return cwd.to_path_buf();
        };
        if parent == current {
            return cwd.to_path_buf();
        }
        current = parent.to_path_buf();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_names_are_kebab_case() {
        for valid in ["pdf", "pdf-tools", "a1-b2", "x"] {
            assert!(is_skill_name(valid), "{valid} 应合法");
        }
        for invalid in ["", "PDF", "a_b", "-a", "a-", "a--b", "a b"] {
            assert!(!is_skill_name(invalid), "{invalid} 应非法");
        }
    }

    #[test]
    fn frontmatter_parser_handles_block_and_quoted_values() {
        let raw = "---\nname: demo\ndescription: |\n  line one\n  line two\n---\nBody.\n";
        let parsed = parse_skill(raw).unwrap();
        assert_eq!(parsed.name, "demo");
        assert_eq!(parsed.description, "line one\nline two");
        assert_eq!(parsed.body, "Body.");
        let quoted = "---\nname: 'quoted-skill'\ndescription: \"Desc.\"\n---\nBody\n";
        assert_eq!(parse_skill(quoted).unwrap().description, "Desc.");
    }

    #[test]
    fn invalid_frontmatter_is_rejected() {
        for raw in [
            "no frontmatter",
            "---\nname: x\n",
            "---\nname: Bad_X\ndescription: d\n---\nbody",
            "---\ndescription: no name\n---\nbody",
            "---\nname: good\ndescription: \"unclosed\n---\nbody",
            "---\nname: good\ndescription: [unclosed\n---\nbody",
            "---\nname: good\ndescription: [a, b]\n---\nbody",
        ] {
            assert!(parse_skill(raw).is_err(), "应拒绝：{raw}");
        }
    }

    // ---- T006.11 红：完整 YAML / DSH 字段 / 警告（skeleton 上这些断言应失败）----

    fn full_yaml_doc() -> &'static str {
        "---\nname: pdf-tools\ndescription: |\n  Multi\n  line\nwhenToUse: >\n  Use when\n  needed\nmetadata:\n  owner: aios\n  tags:\n    - one\n    - two\ndisable-model-invocation: yes\nuser-invocable: 'false'\n---\n# Body\n"
    }

    #[test]
    fn full_yaml_frontmatter_parses_dsh_fields() {
        let parsed = parse_skill_frontmatter(full_yaml_doc()).unwrap();
        assert_eq!(parsed.name, "pdf-tools");
        assert_eq!(parsed.description, "Multi\nline");
        assert_eq!(parsed.when_to_use.as_deref(), Some("Use when needed"));
        let metadata = parsed.metadata.expect("metadata 是映射");
        assert_eq!(metadata["owner"], "aios");
        assert_eq!(metadata["tags"][1], "two");
        assert!(!parsed.invocation.model_invocable, "DSH 宽松 bool：yes");
        assert!(!parsed.invocation.user_invocable, "DSH 宽松 bool：'false'");
    }

    #[test]
    fn yaml_anchors_and_aliases_are_resolved() {
        let raw = "---\nname: anchor-skill\ndescription: &desc \"shared text\"\nwhenToUse: *desc\n---\nBody\n";
        let parsed = parse_skill_frontmatter(raw).unwrap();
        assert_eq!(parsed.description, "shared text");
        assert_eq!(parsed.when_to_use.as_deref(), Some("shared text"));
    }

    #[test]
    fn invocation_booleans_follow_dsh_leniency_and_legacy_keys_fail() {
        for (value, model_invocable) in [
            ("yes", false),
            ("no", true),
            ("1", false),
            ("0", true),
            ("off", true),
            ("on", false),
            ("'true'", false),
        ] {
            let raw = format!(
                "---\nname: lenient\ndescription: d\ndisable-model-invocation: {value}\n---\nBody\n"
            );
            let parsed = parse_skill_frontmatter(&raw).unwrap();
            assert_eq!(parsed.invocation.model_invocable, model_invocable, "{value}");
        }
        let invalid =
            "---\nname: bad\ndescription: d\nuser-invocable: maybe\n---\nBody\n";
        assert!(
            parse_skill_frontmatter(invalid).is_err(),
            "非布尔要报错（不能静默）"
        );
        for legacy in [
            "disableModelInvocation: true",
            "modelInvocable: false",
            "userInvocable: false",
        ] {
            let raw = format!("---\nname: legacy\ndescription: d\n{legacy}\n---\nBody\n");
            let error = parse_skill_frontmatter(&raw).unwrap_err();
            assert!(error.contains("unsupported"), "旧键名要报错：{error}");
        }
    }

    #[test]
    fn invalid_skills_are_reported_as_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");
        std::fs::create_dir_all(root.join("good")).unwrap();
        std::fs::write(
            root.join("good/SKILL.md"),
            "---\nname: good\ndescription: Good skill.\n---\nBody\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("bad-yaml")).unwrap();
        std::fs::write(
            root.join("bad-yaml/SKILL.md"),
            "---\nname: bad-yaml\ndescription: [unclosed\n---\nBody\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("missing-desc")).unwrap();
        std::fs::write(
            root.join("missing-desc/SKILL.md"),
            "---\nname: missing-desc\n---\nBody\n",
        )
        .unwrap();
        let data = tempfile::tempdir().unwrap();
        let discovery = discover_skill_catalog(
            dir.path(),
            data.path(),
            std::slice::from_ref(&root),
        );
        assert!(
            discovery.skills.iter().any(|skill| skill.name == "good"),
            "合法 skill 仍可用"
        );
        assert_eq!(
            discovery.warnings.len(),
            2,
            "非法 skill 各记一条警告：{:?}",
            discovery.warnings
        );
        assert!(discovery
            .warnings
            .iter()
            .any(|warning| warning.message.contains("invalid YAML frontmatter")));
        assert!(discovery
            .warnings
            .iter()
            .any(|warning| warning.message.contains("requires name and description")));
    }

    #[test]
    fn catalog_description_matches_dsh_normalization() {
        let long = format!("{}word", "a ".repeat(400));
        let rendered = catalog_description(&long);
        assert!(
            rendered.chars().count() <= CATALOG_DESCRIPTION_MAX,
            "预算内：{}",
            rendered.chars().count()
        );
        assert!(rendered.ends_with("..."), "DSH 截断用三个点：{rendered}");
        let multiline = catalog_description("  line one\n line two\t ");
        assert_eq!(multiline, "line one line two");
    }
}
