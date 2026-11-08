//! skill 发现、frontmatter 解析与加载（T006.11 照 DSH skill/skill-filesystem 改回）。
//!
//! 根优先级（后者被前者遮蔽）：项目根 /.agents/skills、<data-dir>/skills、配置的 skill_roots；
//! 不读 ~/.dsh。完整 YAML frontmatter（yaml-rust2 0.13，default-features=false：skill 文件按 UTF-8
//! 读，不需要 encoding 特性）照 DSH 逐字段校验：name/description 必需非空、whenToUse 可选字符串、
//! metadata 可选映射、disable-model-invocation/user-invocable 宽松布尔（bool、1/0、'1'/'0'、
//! true/yes/on/false/no/off），旧键名直接报错；非法 skill 记警告（不再静默丢）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use yaml_rust2::yaml::Hash;
use yaml_rust2::{Yaml, YamlLoader};

use crate::ToolFailure;

/// skill bundle 的入口文件名。
pub const SKILL_FILE: &str = "SKILL.md";
/// 项目根下的 skill 根（照 DSH project-agents）。
pub const PROJECT_SKILLS: &str = ".agents/skills";
/// 项目根标记。
pub const PROJECT_ROOT_MARKER: &str = ".git";
/// 目录里渲染的 description 上限（照 DSH catalogDescriptionMaxLength 默认 500）。
pub const CATALOG_DESCRIPTION_MAX: usize = 500;

/// 一条可用 skill 的目录摘要（DSH SkillSummary 的子集 + AIOS 的 path/base_dir）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    /// 可选路由提示；照 DSH 不渲染进目录/工具结果，只供消费者读取。
    pub when_to_use: Option<String>,
    pub invocation: SkillInvocation,
    /// 可选 frontmatter metadata 映射；照 DSH 只供消费者读取。
    pub metadata: Option<serde_json::Value>,
    pub path: PathBuf,
    pub base_dir: PathBuf,
}

/// 加载后的 skill：摘要 + 全文 + 来源目录。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillContent {
    pub summary: SkillSummary,
    pub body: String,
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

/// 程序自带的一份 skill：正文编进二进制，来源没有目录，排在所有用户 skill 之后。
///
/// 重名时用户的赢（发现按来源顺序先到先得，用户根在 `skill_roots` 里排在前面）。
/// 自带哪几份由产品装配决定（`Agent::open` 传 `bundled_skills()`）；发现和加载本身
/// 不猜产品有什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundledSkill {
    /// 程序里的名字；发现时仍以正文 frontmatter 的 name 为准。
    pub name: &'static str,
    /// 正文（含 frontmatter），编译期从 `bundled/<name>/SKILL.md` 读进来。
    pub raw: &'static str,
}

/// 随二进制发布、用户可以加载的 skill。
pub fn bundled_skills() -> &'static [BundledSkill] {
    const BUNDLED: &[BundledSkill] = &[BundledSkill {
        name: "aios-settings",
        raw: include_str!("../bundled/aios-settings/SKILL.md"),
    }];
    BUNDLED
}

/// 一条发现警告（DSH 对非法 skill 打 logger.warn，不静默丢）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillWarning {
    pub path: PathBuf,
    pub message: String,
}

/// 一次发现的完整结果：可用 skill（仅 model-invocable）+ 警告 + 是否不完整。
#[derive(Debug, Default)]
pub struct SkillDiscovery {
    pub skills: Vec<SkillSummary>,
    pub warnings: Vec<SkillWarning>,
    pub incomplete: bool,
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

/// 发现所有可用 skill（仅 model-invocable，供目录注入）。
pub fn discover_skills(
    cwd: &Path,
    data_dir: &Path,
    configured: &[PathBuf],
    bundled: &[BundledSkill],
) -> Vec<SkillSummary> {
    discover_skill_catalog(cwd, data_dir, configured, bundled).skills
}

/// 发现 skill + 警告 + 完整性（T006.11：非法 skill 不再静默丢）。
pub fn discover_skill_catalog(
    cwd: &Path,
    data_dir: &Path,
    configured: &[PathBuf],
    bundled: &[BundledSkill],
) -> SkillDiscovery {
    let (found, warnings, incomplete) = discover(cwd, data_dir, configured, bundled);
    SkillDiscovery {
        skills: found
            .into_iter()
            .filter(|found| found.summary.invocation.model_invocable)
            .map(|found| found.summary)
            .collect(),
        warnings,
        incomplete,
    }
}

/// 按名字加载 skill 全文；名字非法、找不到、或模型不可调用都返回错误。
pub fn load_skill(
    cwd: &Path,
    data_dir: &Path,
    configured: &[PathBuf],
    bundled: &[BundledSkill],
    name: &str,
) -> Result<SkillContent, ToolFailure> {
    if !is_skill_name(name) {
        return Err(ToolFailure::plain(format!("invalid skill name \"{name}\"")));
    }
    let (found, _warnings, _incomplete) = discover(cwd, data_dir, configured, bundled);
    let Some(found) = found.into_iter().find(|found| found.summary.name == name) else {
        return Err(ToolFailure::plain(format!(
            "skill \"{name}\" is unknown or no longer available"
        )));
    };
    if !found.summary.invocation.model_invocable {
        return Err(ToolFailure::plain(format!(
            "skill \"{name}\" is not available for model invocation"
        )));
    }
    Ok(SkillContent {
        summary: found.summary,
        body: found.body,
    })
}

/// 用户显式 /name 调用通道（T006.11b 照 DSH tool-skill 的 /name gesture）：
/// 只认 user-invocable 的 skill；未知或 user-invocable=false 的名字静默跳过（DSH：仍是普通文本）。
/// 不做模型调用策略检查——这条通道正是 disable-model-invocation skill 的唯一入口。
pub fn load_user_invocable_skill(
    cwd: &Path,
    data_dir: &Path,
    configured: &[PathBuf],
    bundled: &[BundledSkill],
    name: &str,
) -> Option<SkillContent> {
    if !is_skill_name(name) {
        return None;
    }
    let (found, _warnings, _incomplete) = discover(cwd, data_dir, configured, bundled);
    let found = found.into_iter().find(|found| found.summary.name == name)?;
    if !found.summary.invocation.user_invocable {
        return None;
    }
    Some(SkillContent {
        summary: found.summary,
        body: found.body,
    })
}

/// 从一条用户文本里取 /name 手势 token，按首次出现顺序去重（DSH tool-skill 的
/// SKILL_GESTURE：空白边界、名字走公开的 kebab-case 语法；`/usr/bin`、`5/8` 不算）。
pub fn invoked_skill_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for token in text.split_whitespace() {
        let Some(name) = token.strip_prefix('/') else {
            continue;
        };
        if is_skill_name(name) && !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// skill 名是否合法：kebab-case，即 [a-z0-9]+(-[a-z0-9]+)*。
pub fn is_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

/// 渲染 <skill_content> 块（加载工具返回给模型的正文 + 来源目录）。
pub fn render_skill_content(skill: &SkillContent) -> String {
    // 编进二进制的 skill 没有目录：不写「按基目录解析」这段，免得模型去解析一个空路径。
    let resources = if skill.summary.base_dir.as_os_str().is_empty() {
        String::new()
    } else {
        format!(
            "<skill_resources>\nBase directory for this skill: {}\nResolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.\n</skill_resources>\n\n",
            skill.summary.base_dir.display()
        )
    };
    format!(
        "<skill_content name=\"{}\">\n{resources}<skill_instructions>\n{}\n</skill_instructions>\n</skill_content>",
        skill.summary.name,
        skill.body.trim_end()
    )
}

/// 目录里的一行描述：压成单行、按 DSH catalogDescription 截断（超长留 max-3 字符 + 三个点）；
/// 转义属于渲染框架，不在这里做（typed source 存原文）。
pub fn catalog_description(description: &str) -> String {
    let normalized = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= CATALOG_DESCRIPTION_MAX {
        return normalized;
    }
    let truncated: String = normalized
        .chars()
        .take(CATALOG_DESCRIPTION_MAX - 3)
        .collect();
    format!("{truncated}...")
}

/// 解析 SKILL.md 的 YAML frontmatter 为 DSH 完整字段；错误消息照 DSH skill-filesystem 的 warn 文案。
pub fn parse_skill_frontmatter(raw: &str) -> Result<SkillFrontmatter, String> {
    let (yaml_text, _body) = split_frontmatter(raw)?;
    let documents = YamlLoader::load_from_str(yaml_text)
        .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;
    let Some(document) = documents.first() else {
        return Err("missing YAML frontmatter".to_owned());
    };
    let Yaml::Hash(mapping) = document else {
        return Err("missing YAML frontmatter".to_owned());
    };
    let name = string_field(mapping, "name");
    let description = string_field(mapping, "description");
    let (Some(name), Some(description)) = (name, description) else {
        return Err("frontmatter requires name and description".to_owned());
    };
    if !is_skill_name(&name) {
        return Err(format!("invalid skill name \"{name}\""));
    }
    let when_to_use = optional_string(mapping, "whenToUse");
    let metadata = optional_metadata(mapping);
    let invocation = parse_invocation_policy(mapping)
        .map_err(|error| format!("invalid invocation frontmatter: {error}"))?;
    Ok(SkillFrontmatter {
        name,
        description,
        when_to_use,
        metadata,
        invocation,
    })
}

fn discover(
    cwd: &Path,
    data_dir: &Path,
    configured: &[PathBuf],
    bundled: &[BundledSkill],
) -> (Vec<Discovered>, Vec<SkillWarning>, bool) {
    let mut found: BTreeMap<String, Discovered> = BTreeMap::new();
    let mut warnings: Vec<SkillWarning> = Vec::new();
    let mut incomplete = false;
    for root in skill_roots(cwd, data_dir, configured) {
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) => {
                if error.kind() == std::io::ErrorKind::NotFound {
                    continue;
                }
                incomplete = true;
                warnings.push(SkillWarning {
                    path: root.clone(),
                    message: format!(
                        "skill root \"{}\" skipped: {error}. Other roots remain available; check this directory's permissions and symlinks. Discovery retries on the next lookup.",
                        root.display()
                    ),
                });
                continue;
            }
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for entry in entries {
            let entry_name = entry
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if entry_name.starts_with('.') {
                continue;
            }
            let (skill_file, base_dir) = if entry.is_dir() {
                (entry.join(SKILL_FILE), entry.clone())
            } else if entry_name.ends_with(".md") {
                (entry.clone(), root.clone())
            } else {
                continue;
            };
            let raw = match std::fs::read_to_string(&skill_file) {
                Ok(raw) => raw,
                Err(error) => {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        continue;
                    }
                    incomplete = true;
                    warnings.push(SkillWarning {
                        path: skill_file.clone(),
                        message: format!(
                            "skill file {} ignored: failed to read text file: {error}",
                            skill_file.display()
                        ),
                    });
                    continue;
                }
            };
            match parse_skill_frontmatter(&raw) {
                Ok(frontmatter) => {
                    if found.contains_key(&frontmatter.name) {
                        continue;
                    }
                    let body = split_frontmatter(&raw)
                        .map(|(_, body)| body.trim().to_owned())
                        .unwrap_or_default();
                    found.insert(
                        frontmatter.name.clone(),
                        Discovered {
                            summary: SkillSummary {
                                name: frontmatter.name,
                                description: frontmatter.description,
                                when_to_use: frontmatter.when_to_use,
                                invocation: frontmatter.invocation,
                                metadata: frontmatter.metadata,
                                path: skill_file,
                                base_dir,
                            },
                            body,
                        },
                    );
                }
                Err(message) => warnings.push(SkillWarning {
                    path: skill_file.clone(),
                    message: format!("skill file {} ignored: {message}", skill_file.display()),
                }),
            }
        }
    }
    // 程序自带的 skill 排在最后：用户根的条目先插入，重名时用户的赢。
    for bundled in bundled {
        match parse_skill_frontmatter(bundled.raw) {
            Ok(frontmatter) => {
                if found.contains_key(&frontmatter.name) {
                    continue;
                }
                let body = split_frontmatter(bundled.raw)
                    .map(|(_, body)| body.trim().to_owned())
                    .unwrap_or_default();
                found.insert(
                    frontmatter.name.clone(),
                    Discovered {
                        summary: SkillSummary {
                            name: frontmatter.name,
                            description: frontmatter.description,
                            when_to_use: frontmatter.when_to_use,
                            invocation: frontmatter.invocation,
                            metadata: frontmatter.metadata,
                            // 编进二进制的 skill 没有目录：路径与基目录留空，
                            // 渲染时不写「按基目录解析」的提示。
                            path: PathBuf::new(),
                            base_dir: PathBuf::new(),
                        },
                        body,
                    },
                );
            }
            // 二进制里的 skill 解析失败是构建期错误；仍走警告，不静默丢。
            Err(message) => warnings.push(SkillWarning {
                path: PathBuf::from(bundled.name),
                message: format!("bundled skill {} ignored: {message}", bundled.name),
            }),
        }
    }
    (found.into_values().collect(), warnings, incomplete)
}

fn split_frontmatter(raw: &str) -> Result<(&str, &str), String> {
    let Some(first_end) = raw.find('\n') else {
        return Err("missing YAML frontmatter".to_owned());
    };
    if raw[..first_end].trim_end_matches('\r') != "---" {
        return Err("missing YAML frontmatter".to_owned());
    }
    let start = first_end + 1;
    let mut line_start = start;
    loop {
        match raw[line_start..].find('\n') {
            Some(relative) => {
                let line_end = line_start + relative;
                if raw[line_start..line_end].trim_end_matches('\r') == "---" {
                    return Ok((&raw[start..line_start], &raw[line_end + 1..]));
                }
                line_start = line_end + 1;
            }
            None => {
                if raw[line_start..].trim_end_matches('\r') == "---" {
                    return Ok((&raw[start..line_start], ""));
                }
                return Err("missing YAML frontmatter".to_owned());
            }
        }
    }
}

fn hash_get<'a>(mapping: &'a Hash, key: &str) -> Option<&'a Yaml> {
    mapping.get(&Yaml::String(key.to_owned()))
}

fn string_field(mapping: &Hash, key: &str) -> Option<String> {
    match hash_get(mapping, key) {
        Some(Yaml::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn optional_string(mapping: &Hash, key: &str) -> Option<String> {
    string_field(mapping, key)
}

fn optional_metadata(mapping: &Hash) -> Option<serde_json::Value> {
    match hash_get(mapping, "metadata") {
        Some(Yaml::Hash(metadata)) => Some(yaml_to_json(&Yaml::Hash(metadata.clone()))),
        _ => None,
    }
}

fn yaml_to_json(yaml: &Yaml) -> serde_json::Value {
    match yaml {
        Yaml::Real(real) => real
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(real.clone())),
        Yaml::Integer(integer) => serde_json::Value::Number((*integer).into()),
        Yaml::String(string) => serde_json::Value::String(string.clone()),
        Yaml::Boolean(boolean) => serde_json::Value::Bool(*boolean),
        Yaml::Array(array) => serde_json::Value::Array(array.iter().map(yaml_to_json).collect()),
        Yaml::Hash(mapping) => {
            let mut object = serde_json::Map::new();
            for (key, value) in mapping {
                let key = match key {
                    Yaml::String(key) => key.clone(),
                    other => format!("{other:?}"),
                };
                object.insert(key, yaml_to_json(value));
            }
            serde_json::Value::Object(object)
        }
        Yaml::Alias(_) | Yaml::Null | Yaml::BadValue => serde_json::Value::Null,
    }
}

fn frontmatter_boolean(mapping: &Hash, key: &str) -> Result<Option<bool>, String> {
    let Some(value) = hash_get(mapping, key) else {
        return Ok(None);
    };
    match value {
        Yaml::Boolean(boolean) => Ok(Some(*boolean)),
        Yaml::Integer(1) => Ok(Some(true)),
        Yaml::Integer(0) => Ok(Some(false)),
        Yaml::String(text) => match text.as_str() {
            "1" => Ok(Some(true)),
            "0" => Ok(Some(false)),
            _ => match text.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" => Ok(Some(true)),
                "false" | "no" | "off" => Ok(Some(false)),
                _ => Err(format!("frontmatter field \"{key}\" must be a boolean")),
            },
        },
        _ => Err(format!("frontmatter field \"{key}\" must be a boolean")),
    }
}

fn parse_invocation_policy(mapping: &Hash) -> Result<SkillInvocation, String> {
    for (legacy, canonical) in [
        ("disableModelInvocation", "disable-model-invocation"),
        ("modelInvocable", "disable-model-invocation"),
        ("userInvocable", "user-invocable"),
    ] {
        if hash_get(mapping, legacy).is_some() {
            return Err(format!(
                "frontmatter field \"{legacy}\" is unsupported; use \"{canonical}\""
            ));
        }
    }
    let disable_model_invocation = frontmatter_boolean(mapping, "disable-model-invocation")?;
    let user_invocable = frontmatter_boolean(mapping, "user-invocable")?;
    Ok(SkillInvocation {
        model_invocable: disable_model_invocation != Some(true),
        user_invocable: user_invocable != Some(false),
    })
}

#[cfg(test)]
#[derive(Debug)]
struct ParsedSkill {
    name: String,
    description: String,
    body: String,
}

#[cfg(test)]
fn parse_skill(raw: &str) -> Result<ParsedSkill, String> {
    let frontmatter = parse_skill_frontmatter(raw)?;
    let body = split_frontmatter(raw)?.1.trim().to_owned();
    Ok(ParsedSkill {
        name: frontmatter.name,
        description: frontmatter.description,
        body,
    })
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
        // 完整 YAML 1.2：| 块标量保留 clip 末尾换行（DSH yaml.parse 同）。
        assert_eq!(parsed.description, "line one\nline two\n");
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
        assert_eq!(parsed.description, "Multi\nline\n");
        assert_eq!(
            parsed.when_to_use.as_deref(),
            Some("Use when needed\n"),
            "> 折叠块标量同样保留 clip 末尾换行"
        );
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
            assert_eq!(
                parsed.invocation.model_invocable, model_invocable,
                "{value}"
            );
        }
        let invalid = "---\nname: bad\ndescription: d\nuser-invocable: maybe\n---\nBody\n";
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
        let discovery =
            discover_skill_catalog(dir.path(), data.path(), std::slice::from_ref(&root), &[]);
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
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.message.contains("invalid YAML frontmatter"))
        );
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.message.contains("requires name and description"))
        );
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

/// T006.19 红测试：非法 skill 的警告文案照 DSH skill-filesystem 的 logger.warn 形状
/// （`skill file <path> ignored: <reason>`，skill-filesystem/src/index.ts:904-926）。
#[cfg(test)]
mod t006_19_warning_format_tests {
    use super::*;

    #[test]
    fn invalid_skill_warning_is_prefixed_like_dsh_logger() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        std::fs::create_dir_all(skills.join("bad")).unwrap();
        std::fs::write(skills.join("bad").join(SKILL_FILE), "no frontmatter here").unwrap();
        let discovery = discover_skill_catalog(dir.path(), dir.path(), &[skills], &[]);
        let warning = discovery
            .warnings
            .iter()
            .find(|warning| warning.message.contains("frontmatter"))
            .expect("非法 skill 必须有警告");
        assert!(
            warning.message.starts_with("skill file "),
            "DSH 文案以 skill file 开头：{}",
            warning.message
        );
        assert!(
            warning.message.contains(" ignored: "),
            "DSH 文案带 ignored 前缀：{}",
            warning.message
        );
    }
}
