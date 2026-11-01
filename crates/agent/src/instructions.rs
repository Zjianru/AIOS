//! 指令文件（AGENTS.md / CLAUDE.md）发现、渲染与按步刷新。
//!
//! 照 DSH context/agent-instructions 的可观察语义做最小实现：用户全局链（<data-dir> 与
//! AgentConfig::instruction_roots，不读 ~/.dsh）+ 项目链（.git 根到 cwd，目录从宽泛到具体）；
//! 注入是普通 user/message，首次请求带基线，之后只在「读到更深目录/文件变化」时补刷新消息；
//! 同一候选组内去首尾空白后内容相同的文件只渲染一次；每条链各自 65,536 字节预算。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use model::{ContentBlock, Message};

pub const USER_GLOBAL_FILE: &str = "AGENTS.md";
pub const INSTRUCTION_CANDIDATES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
pub const LOCAL_INSTRUCTION_CANDIDATES: [&str; 2] = ["AGENTS.local.md", "CLAUDE.local.md"];
pub const PROJECT_ROOT_MARKER: &str = ".git";
/// 每条链（用户全局、项目）的渲染预算，照 DSH agent-instructions 默认 maxBytes。
pub const MAX_CHAIN_BYTES: usize = 65_536;
/// 渲染前单个源文件上限，照 DSH maxSourceBytes。
pub const MAX_SOURCE_BYTES: u64 = 1_048_576;

const FRAME_OPEN: &str = "<system-reminder>";
const FRAME_CLOSE: &str = "</system-reminder>";
const SECTION_PREFIX: &str = "Instructions from: ";
const BASELINE_INTRO: &str = "Workspace instructions (AGENTS.md / CLAUDE.md):";
const REFRESH_INTRO: &str = "Workspace instructions update (AGENTS.md / CLAUDE.md):";

/// 指令文件属于哪条链（渲染时分开预算；用户全局在前）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chain {
    UserGlobal,
    Project,
}

/// 一条指令文件：绝对路径、给模型看的路径、内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    pub path: PathBuf,
    pub display: String,
    pub content: String,
    pub chain: Chain,
}

impl InstructionFile {
    fn trimmed(&self) -> String {
        self.content.trim().to_owned()
    }
}

/// 会话内的指令注入状态：已注入的 display → 去空白内容。
#[derive(Debug, Default)]
pub struct InstructionState {
    injected: BTreeMap<String, String>,
    baseline_seen: bool,
}

impl InstructionState {
    /// 从已有会话消息对账（续接）：收集之前注入过的 display 路径。
    pub fn rehydrate(&mut self, messages: &[Message]) {
        for message in messages {
            for block in &message.content {
                let ContentBlock::Text { text } = block else {
                    continue;
                };
                let mut display: Option<String> = None;
                let mut content: Vec<&str> = Vec::new();
                let flush = |display: &mut Option<String>,
                             content: &mut Vec<&str>,
                             injected: &mut BTreeMap<String, String>| {
                    if let Some(display) = display.take() {
                        injected.insert(
                            display,
                            content
                                .join(
                                    "
",
                                )
                                .trim()
                                .to_owned(),
                        );
                        content.clear();
                    }
                };
                for line in text.lines() {
                    if let Some(found) = line.strip_prefix(SECTION_PREFIX) {
                        flush(&mut display, &mut content, &mut self.injected);
                        display = Some(found.trim().to_owned());
                    } else if display.is_some() {
                        if line == FRAME_CLOSE || line.starts_with("Removed: ") {
                            flush(&mut display, &mut content, &mut self.injected);
                        } else {
                            content.push(line);
                        }
                    }
                }
                flush(&mut display, &mut content, &mut self.injected);
            }
        }
        self.baseline_seen = !self.injected.is_empty();
    }

    /// 每个 step 组装请求前调用：发现适用的指令文件，返回要追加的 user/message 文本。
    /// 没有变化返回 None。observed 是 T005.1 的路径观察状态（读/写/编辑过的文件）。
    pub fn prepare(
        &mut self,
        cwd: &Path,
        data_dir: &Path,
        instruction_roots: &[PathBuf],
        observed: impl Iterator<Item = PathBuf>,
    ) -> Option<String> {
        let cwd = canonical(cwd);
        let project_root = find_project_root(&cwd);
        let mut dirs = ancestor_chain(&project_root, &cwd);
        // 多个观察路径来自 HashMap，先按路径排序保证确定性；目录按下标深度、再按路径排序，
        // 追加在 cwd 之后，整体保持 root→cwd→更深目录 的单调顺序。
        let mut observed: Vec<PathBuf> = observed.map(|path| canonical(&path)).collect();
        observed.sort();
        let mut extra: Vec<PathBuf> = Vec::new();
        for path in observed {
            let Some(parent) = path.parent() else {
                continue;
            };
            if !parent.starts_with(&cwd) {
                continue;
            }
            for dir in ancestor_chain(&cwd, parent).into_iter().skip(1) {
                if !extra.contains(&dir) {
                    extra.push(dir);
                }
            }
        }
        extra.sort_by(|left, right| {
            left.components()
                .count()
                .cmp(&right.components().count())
                .then_with(|| left.cmp(right))
        });
        dirs.extend(extra);

        let applicable = discover(&project_root, &dirs, data_dir, instruction_roots);
        let mut changed: Vec<InstructionFile> = Vec::new();
        for file in &applicable {
            match self.injected.get(&file.display) {
                Some(previous) if previous == &file.trimmed() => {}
                _ => changed.push(file.clone()),
            }
        }
        let mut removed: Vec<String> = Vec::new();
        for display in self.injected.keys() {
            if !applicable.iter().any(|file| &file.display == display) {
                removed.push(display.clone());
            }
        }

        self.injected = applicable
            .iter()
            .map(|file| (file.display.clone(), file.trimmed()))
            .collect();
        let baseline = !self.baseline_seen;
        self.baseline_seen = true;
        if changed.is_empty() && removed.is_empty() {
            return None;
        }
        Some(render(&changed, &removed, &applicable, baseline))
    }
}

/// 找项目根：从 cwd 向上找 .git 标记；没有标记时用 cwd 本身（照 DSH findProjectRoot）。
pub fn find_project_root(cwd: &Path) -> PathBuf {
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

/// root 到 cwd 的目录链（从宽泛到具体）；参数不满足祖先关系时退回 [root]。
pub fn ancestor_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        chain.push(current.clone());
        if current == root {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    if chain.last() != Some(&root.to_path_buf()) {
        chain.push(root.to_path_buf());
    }
    chain.reverse();
    chain
}

fn discover(
    project_root: &Path,
    dirs: &[PathBuf],
    data_dir: &Path,
    instruction_roots: &[PathBuf],
) -> Vec<InstructionFile> {
    let mut files = Vec::new();
    // 用户全局候选组：<data-dir>/AGENTS.md 先，再按配置顺序的 instruction_roots/AGENTS.md；
    // 组内去空白内容相同的只保留第一个。
    let mut global_seen = Vec::new();
    let mut roots: Vec<PathBuf> = vec![data_dir.to_path_buf()];
    for root in instruction_roots {
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    for root in roots {
        let path = root.join(USER_GLOBAL_FILE);
        let Some(content) = read_source(&path) else {
            continue;
        };
        let trimmed = content.trim();
        if global_seen.iter().any(|seen: &String| seen == trimmed) {
            continue;
        }
        global_seen.push(trimmed.to_owned());
        let display = if root == data_dir {
            format!("{USER_GLOBAL_FILE} (data dir: {})", root.display())
        } else {
            path.display().to_string()
        };
        files.push(InstructionFile {
            path,
            display,
            content,
            chain: Chain::UserGlobal,
        });
    }
    // 项目链：每个目录一个候选组（AGENTS.md、CLAUDE.md、AGENTS.local.md、CLAUDE.local.md 顺序）。
    for dir in dirs {
        let mut group_seen = Vec::new();
        for name in INSTRUCTION_CANDIDATES
            .into_iter()
            .chain(LOCAL_INSTRUCTION_CANDIDATES)
        {
            let path = dir.join(name);
            let Some(content) = read_source(&path) else {
                continue;
            };
            let trimmed = content.trim();
            if group_seen.iter().any(|seen: &String| seen == trimmed) {
                continue;
            }
            group_seen.push(trimmed.to_owned());
            let display = display_path(project_root, &path);
            files.push(InstructionFile {
                path,
                display,
                content,
                chain: Chain::Project,
            });
        }
    }
    files
}

fn read_source(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn display_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

struct ChainBudget<'a> {
    /// 放进预算的文件（宽泛→具体）；被截断的文件带替换后的内容。
    included: Vec<(&'a InstructionFile, Option<String>)>,
    /// 整份省略的文件 display（最宽泛的在前）。
    omitted: Vec<String>,
    /// 被截断的最具体文件 display。
    truncated: Option<String>,
}

/// 按 DSH 语义做每链预算：放不下时先整份省略最宽泛的文件，保留更具体的；
/// 只剩最具体的一个还放不下时，按 UTF-8 边界安全地按字节截断它并加 [truncated]。
fn budget_chain<'a>(files: &[&'a InstructionFile]) -> ChainBudget<'a> {
    let section_len = |file: &InstructionFile| {
        format!("{SECTION_PREFIX}{}\n\n{}", file.display, file.content).len()
    };
    let total: usize = files.iter().map(|file| section_len(file)).sum();
    if total <= MAX_CHAIN_BYTES {
        return ChainBudget {
            included: files.iter().map(|file| (*file, None)).collect(),
            omitted: Vec::new(),
            truncated: None,
        };
    }
    let mut start = 0;
    while start + 1 < files.len() {
        let remaining: usize = files[start..].iter().map(|file| section_len(file)).sum();
        if remaining <= MAX_CHAIN_BYTES {
            break;
        }
        start += 1;
    }
    let omitted: Vec<String> = files[..start]
        .iter()
        .map(|file| file.display.clone())
        .collect();
    let kept = &files[start..];
    if kept.len() == 1 {
        let only = kept[0];
        if section_len(only) > MAX_CHAIN_BYTES {
            let overhead = SECTION_PREFIX.len() + only.display.len() + "\n\n".len();
            let marker = "\n\n[truncated]";
            let remaining = MAX_CHAIN_BYTES.saturating_sub(overhead + marker.len());
            let truncated_content = truncate_bytes(&only.content, remaining);
            return ChainBudget {
                included: vec![(only, Some(format!("{truncated_content}{marker}")))],
                omitted,
                truncated: Some(only.display.clone()),
            };
        }
    }
    ChainBudget {
        included: kept.iter().map(|file| (*file, None)).collect(),
        omitted,
        truncated: None,
    }
}

/// 按 UTF-8 字符边界截到不超过 max_bytes 字节的前缀（CJK 不会超预算、也不会切坏字符）。
fn truncate_bytes(content: &str, max_bytes: usize) -> String {
    let mut end = 0;
    for (index, character) in content.char_indices() {
        let next = index + character.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    content[..end].to_owned()
}

fn chain_name(chain: Chain) -> &'static str {
    match chain {
        Chain::UserGlobal => "user",
        Chain::Project => "project",
    }
}

fn render(
    changed: &[InstructionFile],
    removed: &[String],
    applicable: &[InstructionFile],
    baseline: bool,
) -> String {
    let changed_displays: std::collections::BTreeSet<&str> =
        changed.iter().map(|file| file.display.as_str()).collect();
    let mut sections = Vec::new();
    let mut notices = Vec::new();
    for chain in [Chain::UserGlobal, Chain::Project] {
        let chain_files: Vec<&InstructionFile> = applicable
            .iter()
            .filter(|file| file.chain == chain)
            .collect();
        let budgeted = budget_chain(&chain_files);
        for (file, content_override) in &budgeted.included {
            if !changed_displays.contains(file.display.as_str()) {
                continue;
            }
            let content = content_override.as_deref().unwrap_or(&file.content);
            sections.push(format!("{SECTION_PREFIX}{}\n\n{content}", file.display));
        }
        let mut parts = Vec::new();
        let omitted_changed: Vec<&str> = budgeted
            .omitted
            .iter()
            .filter(|display| changed_displays.contains(display.as_str()))
            .map(String::as_str)
            .collect();
        if !omitted_changed.is_empty() {
            parts.push(format!("omitted {}", omitted_changed.join(", ")));
        }
        if let Some(truncated) = &budgeted.truncated
            && changed_displays.contains(truncated.as_str())
        {
            parts.push(format!("truncated {truncated}"));
        }
        if !parts.is_empty() {
            notices.push(format!(
                "Workspace instruction budget {MAX_CHAIN_BYTES} bytes ({}): {}",
                chain_name(chain),
                parts.join("; ")
            ));
        }
    }
    for display in removed {
        sections.push(format!("Removed: {display}"));
    }
    let intro = if baseline {
        BASELINE_INTRO
    } else {
        REFRESH_INTRO
    };
    let mut body = format!("{intro}\n");
    for section in sections {
        body.push('\n');
        body.push_str(&escape(&section));
        body.push('\n');
    }
    for notice in notices {
        body.push('\n');
        body.push_str(&notice);
        body.push('\n');
    }
    format!("{FRAME_OPEN}\n{body}{FRAME_CLOSE}")
}

fn escape(text: &str) -> String {
    text.replace(FRAME_CLOSE, "<\\/system-reminder>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn user_global_roots_never_include_dsh_home() {
        let data_dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write(&data_dir.path().join(USER_GLOBAL_FILE), "DATA\n");
        write(&root.path().join(USER_GLOBAL_FILE), "ROOT\n");
        let files = discover(
            data_dir.path(),
            &[],
            data_dir.path(),
            &[root.path().to_path_buf()],
        );
        assert_eq!(files.len(), 2, "两个用户级文件都应被发现");
        for file in &files {
            assert_eq!(file.chain, Chain::UserGlobal);
            let path = file.path.display().to_string();
            assert!(
                file.path.starts_with(data_dir.path()) || file.path.starts_with(root.path()),
                "用户级候选只能来自 data-dir / 配置根：{path}"
            );
            assert!(!path.contains(".dsh"), "不许读 ~/.dsh：{path}");
        }
    }

    #[test]
    fn duplicate_content_within_group_renders_once() {
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join(".git").join("HEAD"), "");
        write(&project.path().join("AGENTS.md"), "SAME\n");
        write(&project.path().join("CLAUDE.md"), "SAME\n");
        let files = discover(
            project.path(),
            &[project.path().to_path_buf()],
            project.path(),
            &[],
        );
        let project_files: Vec<&InstructionFile> = files
            .iter()
            .filter(|file| project.path().join(&file.path).exists() && file.chain == Chain::Project)
            .collect();
        assert_eq!(project_files.len(), 1, "同组相同内容只保留一个");
        assert!(project_files[0].display.ends_with("AGENTS.md"));
    }

    #[test]
    fn observed_descendant_dirs_extend_chain() {
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join(".git").join("HEAD"), "");
        write(&project.path().join("AGENTS.md"), "ROOT\n");
        let sub = project.path().join("sub");
        write(&sub.join("AGENTS.md"), "SUB\n");
        let deep = sub.join("deep");
        write(&deep.join("AGENTS.md"), "DEEP\n");
        write(
            &deep.join("file.txt"),
            "hello
",
        );
        let mut state = InstructionState::default();

        let baseline = state
            .prepare(project.path(), project.path(), &[], std::iter::empty())
            .unwrap();
        assert!(baseline.contains("ROOT"));
        assert!(!baseline.contains("DEEP"), "基线不含未读到目录的指令");

        let refresh = state
            .prepare(
                project.path(),
                project.path(),
                &[],
                vec![deep.join("file.txt")].into_iter(),
            )
            .unwrap();
        assert!(refresh.contains("DEEP"), "读到更深目录后补入");
        assert!(refresh.contains("SUB"), "中间目录也补入");
        assert!(!refresh.contains("ROOT"), "已有基线不重复注入");
    }

    #[test]
    fn baseline_order_is_broad_to_specific() {
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join(".git").join("HEAD"), "");
        write(&project.path().join("AGENTS.md"), "ROOT\n");
        let sub = project.path().join("sub");
        write(&sub.join("AGENTS.md"), "SUB\n");
        let mut state = InstructionState::default();
        let text = state
            .prepare(&sub, project.path(), &[], std::iter::empty())
            .unwrap();
        let root = text.find("ROOT").unwrap();
        let sub_at = text.find("SUB").unwrap();
        assert!(root < sub_at, "目录链从宽泛到具体");
    }

    #[test]
    fn rehydrate_does_not_reinject_baseline() {
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join(".git").join("HEAD"), "");
        write(&project.path().join("AGENTS.md"), "ROOT\n");
        let mut first = InstructionState::default();
        let baseline = first
            .prepare(project.path(), project.path(), &[], std::iter::empty())
            .unwrap();
        let message = model::Message::text(model::Role::User, baseline);
        let mut resumed = InstructionState::default();
        resumed.rehydrate(&[message]);
        assert!(
            resumed
                .prepare(project.path(), project.path(), &[], std::iter::empty())
                .is_none(),
            "续接后不重复注入同一基线"
        );
    }

    fn file(display: &str, content: &str) -> InstructionFile {
        InstructionFile {
            path: PathBuf::from(display),
            display: display.to_owned(),
            content: content.to_owned(),
            chain: Chain::Project,
        }
    }

    #[test]
    fn budget_omits_broadest_and_keeps_specific() {
        let broad = file("AGENTS.md", &"a".repeat(70_000));
        let specific = file("sub/AGENTS.md", "SPECIFIC");
        let files = [&broad, &specific];
        let budgeted = budget_chain(&files);
        assert_eq!(
            budgeted.omitted,
            vec!["AGENTS.md".to_owned()],
            "先整份省略最宽泛的"
        );
        assert_eq!(budgeted.included.len(), 1);
        assert_eq!(budgeted.included[0].0.display, "sub/AGENTS.md");
        assert!(budgeted.truncated.is_none(), "剩下的放得下就不截断");
    }

    #[test]
    fn budget_truncates_most_specific_only_when_still_over() {
        let huge = file("AGENTS.md", &"a".repeat(80_000));
        let files = [&huge];
        let budgeted = budget_chain(&files);
        assert_eq!(budgeted.omitted, Vec::<String>::new());
        assert_eq!(budgeted.truncated.as_deref(), Some("AGENTS.md"));
        let content = budgeted.included[0].1.as_deref().unwrap();
        let section_len = SECTION_PREFIX.len() + "AGENTS.md".len() + "\n\n".len() + content.len();
        assert!(
            section_len <= MAX_CHAIN_BYTES,
            "section {section_len} > 预算"
        );
        assert!(content.ends_with("[truncated]"));
    }

    #[test]
    fn budget_truncation_is_byte_safe_for_cjk() {
        // 40,000 个 CJK 字符 = 120,000 字节，超过 65,536 字节预算。
        let huge = file("AGENTS.md", &"中".repeat(40_000));
        let files = [&huge];
        let budgeted = budget_chain(&files);
        let content = budgeted.included[0].1.as_deref().unwrap();
        let body = content.strip_suffix("\n\n[truncated]").unwrap();
        assert!(body.is_char_boundary(body.len()));
        assert_eq!(body.chars().count(), body.len() / 3, "CJK 截断不能切坏字符");
        let section_len = SECTION_PREFIX.len() + "AGENTS.md".len() + "\n\n".len() + content.len();
        assert!(
            section_len <= MAX_CHAIN_BYTES,
            "按字节不能超预算：{section_len}"
        );
    }

    #[test]
    fn budget_notice_is_rendered() {
        let data_dir = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join(".git").join("HEAD"), "");
        write(&project.path().join("AGENTS.md"), &"a".repeat(70_000));
        let sub = project.path().join("sub");
        write(&sub.join("AGENTS.md"), "SPECIFIC\n");
        let mut state = InstructionState::default();
        let text = state
            .prepare(&sub, data_dir.path(), &[], std::iter::empty())
            .unwrap();
        assert!(text.contains("SPECIFIC"), "具体文件要保留");
        assert!(
            !text.contains(&"a".repeat(100)),
            "宽泛文件被省略：{}",
            &text[..text.len().min(400)]
        );
        assert!(
            text.contains("Workspace instruction budget"),
            "要有预算 notice"
        );
        assert!(text.contains("omitted"), "notice 说明省略了哪些");
        assert!(text.contains("AGENTS.md"));
    }

    #[test]
    fn multiple_observed_dirs_extend_chain_in_stable_order() {
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join(".git").join("HEAD"), "");
        write(&project.path().join("AGENTS.md"), "ROOT\n");
        let a = project.path().join("a");
        write(&a.join("AGENTS.md"), "AAA\n");
        write(&a.join("x.txt"), "x\n");
        let b = project.path().join("b");
        let deep = b.join("deep");
        write(&deep.join("AGENTS.md"), "DEEP\n");
        write(&deep.join("y.txt"), "y\n");
        let mut state = InstructionState::default();
        state
            .prepare(project.path(), project.path(), &[], std::iter::empty())
            .unwrap();
        // 观察顺序故意反过来（HashMap 迭代顺序不确定），渲染顺序必须仍稳定。
        let refresh = state
            .prepare(
                project.path(),
                project.path(),
                &[],
                vec![deep.join("y.txt"), a.join("x.txt")].into_iter(),
            )
            .unwrap();
        let aaa = refresh.find("AAA").unwrap();
        let deep_at = refresh.find("DEEP").unwrap();
        assert!(aaa < deep_at, "宽泛→具体：a 在 b/deep 前");
        assert!(!refresh.contains("ROOT"), "已有基线不重复");
    }
}
