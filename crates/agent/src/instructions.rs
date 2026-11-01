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
        for path in observed {
            let path = canonical(&path);
            let Some(parent) = path.parent() else {
                continue;
            };
            if !parent.starts_with(&cwd) {
                continue;
            }
            for dir in ancestor_chain(&cwd, parent).into_iter().skip(1) {
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }

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
        Some(render(&changed, &removed, baseline))
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

fn render(changed: &[InstructionFile], removed: &[String], baseline: bool) -> String {
    let global_files: Vec<&InstructionFile> = changed
        .iter()
        .filter(|file| file.chain == Chain::UserGlobal)
        .collect();
    let project_files: Vec<&InstructionFile> = changed
        .iter()
        .filter(|file| file.chain == Chain::Project)
        .collect();
    let mut sections = Vec::new();
    let mut notices = Vec::new();
    for (chain, files) in [("user", global_files), ("project", project_files)] {
        let rendered = budget_chain(chain, &files, &mut notices);
        sections.extend(rendered);
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

fn budget_chain(chain: &str, files: &[&InstructionFile], notices: &mut Vec<String>) -> Vec<String> {
    let mut sections = Vec::new();
    let mut used = 0_usize;
    let mut omitted = Vec::new();
    for file in files {
        let section = format!("{SECTION_PREFIX}{}\n\n{}", file.display, file.content);
        if used + section.len() <= MAX_CHAIN_BYTES {
            used += section.len();
            sections.push(section);
            continue;
        }
        // 放不下：这条链更宽泛的文件先省略；还没有任何 section 时截断这个文件。
        if sections.is_empty() {
            let remaining =
                MAX_CHAIN_BYTES.saturating_sub(SECTION_PREFIX.len() + file.display.len() + 4);
            let mut truncated: String = file.content.chars().take(remaining).collect();
            truncated.push_str("\n\n[truncated]");
            used += SECTION_PREFIX.len() + file.display.len() + truncated.len();
            sections.push(format!("{SECTION_PREFIX}{}\n\n{truncated}", file.display));
        } else {
            omitted.push(file.display.clone());
        }
        let _ = used;
    }
    if !omitted.is_empty() {
        notices.push(format!(
            "Workspace instruction budget {MAX_CHAIN_BYTES} bytes ({chain}): omitted {}",
            omitted.join(", ")
        ));
    }
    sections
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
}
