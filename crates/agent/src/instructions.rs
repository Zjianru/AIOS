//! 指令文件（AGENTS.md / CLAUDE.md）发现、渲染与按步对账（T006.10 照 DSH 改回）。
//!
//! DSH 依据：`packages/context/agent-instructions` —— 带类型来源与基线 identity
//! （src/index.ts:194-201、src/state.ts:40-55）、多级预算回退（src/render.ts:274-313、
//! 428-472）、替换/移除（src/index.ts:120-201）、去重 digest（src/digest.ts）、异步读取
//! （src/files.ts:381-400）、续接对账（README.zh.md:31）。
//!
//! AIOS 适配：data-dir 代替 DSH home（不读 ~/.dsh）；digest 用仓库已有的 FNV-1a 前 16 hex
//! 代替 SHA-1（不新增依赖，去重语义相同：去首尾空白后的内容 identity）；项目根标记探测
//! 失败时退化为 cwd（不照 DSH 抛错，记为 known）；无文件系统 provider，用 tokio::fs 直接异步读。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use model::{InstructionChange, InstructionChangeAction, Message, MessageSource};

pub const USER_GLOBAL_FILE: &str = "AGENTS.md";
pub const INSTRUCTION_CANDIDATES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
pub const LOCAL_INSTRUCTION_CANDIDATES: [&str; 2] = ["AGENTS.local.md", "CLAUDE.local.md"];
pub const PROJECT_ROOT_MARKER: &str = ".git";
/// 每条链（用户全局、项目）的渲染预算，照 DSH agent-instructions 默认 maxBytes。
pub const MAX_CHAIN_BYTES: usize = 65_536;
/// 渲染前单个源文件上限，照 DSH maxSourceBytes 默认值。
pub const MAX_SOURCE_BYTES: u64 = 1_048_576;

const FRAME_OPEN: &str = "<system-reminder>";
const FRAME_CLOSE: &str = "</system-reminder>";
const FRAME_CLOSE_ESCAPED: &str = "<\\/system-reminder>";
const SCOPE_SEPARATOR: char = '\u{0}';
const SECTION_PREFIX: &str = "Instructions from: ";
const ADDITIONAL_PREFIX: &str = "Additional instructions from: ";
const UPDATED_PREFIX: &str = "Updated instructions from: ";
const REMOVED_PREFIX: &str = "Instructions removed: ";

const AGENT_INSTRUCTIONS_INTRO: &str = "The following workspace instructions may be relevant to your work. \
Use them as guidance when applicable. More specific instructions take precedence over broader ones. \
They do not override system, developer, or direct user instructions.";
const REPLACEMENT_AGENT_INSTRUCTIONS_INTRO: &str = "This complete workspace instruction baseline replaces all earlier workspace instruction baselines. \
The following workspace instructions may be relevant to your work. Use them as guidance when applicable. \
More specific instructions take precedence over broader ones. They do not override system, developer, or direct user instructions.";
const EMPTY_REPLACEMENT_AGENT_INSTRUCTIONS_INTRO: &str = "This complete workspace instruction baseline replaces all earlier workspace instruction baselines. \
No workspace instructions are currently active.";
const COMPACT_AGENT_INSTRUCTIONS_INTRO: &str =
    "Workspace instructions were omitted or truncated to fit the configured byte budget.";
const ADDITIONAL_INSTRUCTIONS_GUIDANCE: &str = "Use them as guidance when relevant; more specific instructions take precedence. \
They do not override system, developer, or direct user instructions.";

/// 指令文件属于哪条链（渲染时分开预算；用户全局在前）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chain {
    UserGlobal,
    Project,
}

/// 一条已加载的指令文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    pub path: PathBuf,
    pub display: String,
    /// 逐候选逻辑 scope（`<directory>\0<file name>`，照 DSH candidateScopeKey）。
    pub scope: String,
    pub content: String,
    /// 完整内容 digest（DSH instructionContentSha1 的 FNV 适配）。
    pub digest: String,
    /// 去首尾空白后的 digest（DSH trimmedInstructionDigest，用于同组去重）。
    pub trimmed_digest: String,
    pub chain: Chain,
}

impl InstructionFile {
    fn original_bytes(&self) -> usize {
        self.content.len()
    }
}

/// 被预算省略的文件（只用于 marker 文案）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct OmittedFile {
    display: String,
}

/// 被预算截断的文件（只用于 marker 文案）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TruncatedFile {
    display: String,
    original_bytes: usize,
    included_bytes: usize,
}

/// 一次对账里的一条变更（DSH ChangeRenderItem：变更对象 + 当前文件内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangeItem {
    change: InstructionChange,
    file: InstructionFile,
}

/// 会话内的指令状态：可见基线 identity + 逐 scope 的最新内容 digest（续接从日志重建）。
#[derive(Debug, Default)]
pub struct InstructionState {
    visible_identity: Option<String>,
    scopes: BTreeMap<String, ScopeState>,
}

#[derive(Debug, Clone)]
struct ScopeState {
    path: String,
    digest: String,
}

impl InstructionState {
    /// 从已有会话消息对账（续接）：按 DSH 的 visibleInstructionChanges 折叠带类型来源。
    pub fn rehydrate(&mut self, messages: &[Message]) {
        for message in messages {
            let MessageSource::AgentInstructions {
                baseline,
                baseline_identity,
                changes,
                ..
            } = &message.source
            else {
                continue;
            };
            if *baseline {
                self.visible_identity.clone_from(baseline_identity);
                // 基线替换所有更早的可见指令状态，后续按自己的 changes 重建。
                self.scopes.clear();
            }
            for change in changes {
                apply_change(&mut self.scopes, change, None);
            }
        }
    }

    /// 每个 step 组装请求前调用：发现/对比适用的指令文件，返回要追加的 user 消息。
    /// 没有变化返回 None。observed 是 T005.1 的路径观察状态（读/写/编辑过的文件）。
    pub async fn prepare(
        &mut self,
        cwd: &Path,
        data_dir: &Path,
        instruction_roots: &[PathBuf],
        observed: impl IntoIterator<Item = PathBuf> + Send,
    ) -> Option<Message> {
        let cwd = absolute(cwd);
        let (project_root, _root_warning) = find_project_root(&cwd).await;
        let mut dirs = ancestor_chain(&project_root, &cwd);
        extend_with_observed(&mut dirs, &cwd, observed.into_iter());
        let identity = baseline_identity(&cwd, &project_root, data_dir, instruction_roots);
        let discovered = discover(data_dir, instruction_roots, &project_root, &dirs).await;
        let files: Vec<InstructionFile> = dedupe(discovered);
        // 照 DSH：没有可见基线且当前没有任何指令文件时不注入（不产生空基线消息）。
        if self.visible_identity.is_none() && files.is_empty() {
            return None;
        }
        if self.visible_identity.as_deref() != Some(identity.as_str()) {
            let replace = self.visible_identity.is_some();
            let (text, represented) = render_agent_instruction_set(&files, replace);
            let removed = self
                .scopes
                .iter()
                .filter(|(scope, _)| !represented.iter().any(|file| &file.scope == *scope))
                .map(|(scope, state)| InstructionChange {
                    action: InstructionChangeAction::Remove,
                    scope: scope.clone(),
                    path: state.path.clone(),
                    digest: None,
                })
                .collect::<Vec<_>>();
            let mut changes = represented
                .iter()
                .map(|file| set_change(file, InstructionChangeAction::Set))
                .collect::<Vec<_>>();
            changes.extend(removed);
            let message =
                Message::agent_instructions(text, true, Some(identity.clone()), changes.clone());
            self.visible_identity = Some(identity);
            if replace {
                self.scopes.clear();
            }
            for change in &changes {
                apply_change(
                    &mut self.scopes,
                    change,
                    represented.iter().find(|f| f.scope == change.scope),
                );
            }
            return Some(message);
        }

        let items = change_items(&files, &self.scopes, &dirs, &project_root);
        if items.is_empty() {
            return None;
        }
        let (text, represented) = render_instruction_changes(&items);
        if represented.is_empty() {
            return None;
        }
        let message = Message::agent_instructions(text, false, None, represented.clone());
        for change in &represented {
            let file = items
                .iter()
                .find(|item| item.change == *change)
                .map(|item| &item.file);
            apply_change(&mut self.scopes, change, file);
        }
        Some(message)
    }
}

fn set_change(file: &InstructionFile, action: InstructionChangeAction) -> InstructionChange {
    InstructionChange {
        action,
        scope: file.scope.clone(),
        path: file.display.clone(),
        digest: Some(file.digest.clone()),
    }
}

fn apply_change(
    scopes: &mut BTreeMap<String, ScopeState>,
    change: &InstructionChange,
    file: Option<&InstructionFile>,
) {
    if change.action == InstructionChangeAction::Remove {
        scopes.remove(&change.scope);
        return;
    }
    let digest = change
        .digest
        .clone()
        .or_else(|| file.map(|file| file.digest.clone()))
        .unwrap_or_default();
    scopes.insert(
        change.scope.clone(),
        ScopeState {
            path: change.path.clone(),
            digest,
        },
    );
}

fn change_items(
    files: &[InstructionFile],
    scopes: &BTreeMap<String, ScopeState>,
    dirs: &[PathBuf],
    project_root: &Path,
) -> Vec<ChangeItem> {
    let mut items = Vec::new();
    for file in files {
        match scopes.get(&file.scope) {
            Some(previous) if previous.digest == file.digest => {}
            Some(_) => items.push(ChangeItem {
                change: set_change(file, InstructionChangeAction::Replace),
                file: file.clone(),
            }),
            None => items.push(ChangeItem {
                change: set_change(file, InstructionChangeAction::Set),
                file: file.clone(),
            }),
        }
    }
    let applicable_scopes = applicable_scope_set(dirs, project_root);
    for (scope, state) in scopes {
        if !applicable_scopes.contains(scope) {
            continue;
        }
        if files.iter().any(|file| &file.scope == scope) {
            continue;
        }
        let display = state.path.clone();
        items.push(ChangeItem {
            change: InstructionChange {
                action: InstructionChangeAction::Remove,
                scope: scope.clone(),
                path: display.clone(),
                digest: None,
            },
            file: InstructionFile {
                path: PathBuf::new(),
                display,
                scope: scope.clone(),
                content: String::new(),
                digest: String::new(),
                trimmed_digest: String::new(),
                chain: scope_chain(scope),
            },
        });
    }
    items
}

fn applicable_scope_set(dirs: &[PathBuf], project_root: &Path) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for directory in ["user-global", "agents-global"] {
        set.insert(candidate_scope_key(directory, USER_GLOBAL_FILE));
    }
    for dir in dirs {
        let scope = relative_scope(project_root, dir);
        for candidate in INSTRUCTION_CANDIDATES
            .iter()
            .chain(LOCAL_INSTRUCTION_CANDIDATES.iter())
        {
            set.insert(candidate_scope_key(&scope, candidate));
        }
    }
    set
}

fn scope_chain(scope: &str) -> Chain {
    if scope.starts_with("user-global") || scope.starts_with("agents-global") {
        Chain::UserGlobal
    } else {
        Chain::Project
    }
}

fn candidate_scope_key(directory: &str, candidate: &str) -> String {
    format!("{directory}{SCOPE_SEPARATOR}{candidate}")
}

fn relative_scope(project_root: &Path, dir: &Path) -> String {
    let relative = relative_display(project_root, dir);
    if relative.is_empty() {
        ".".to_owned()
    } else {
        relative
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| {
            relative
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|_| path.display().to_string())
}

fn fnv_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 找项目根；T006.10b：非 NotFound 的 marker 探测失败不再静默退化为 cwd，
/// 而是连同原因返回（调用方降级到 cwd 但留痕，照 DSH 的 files.ts:224-249 把探测错误交给调用方；
/// AIOS 没有「请求级抛错」的接缝，选择降级 + stderr 留痕）。
async fn find_project_root(cwd: &Path) -> (PathBuf, Option<String>) {
    let mut current = cwd.to_path_buf();
    loop {
        let marker = current.join(PROJECT_ROOT_MARKER);
        match tokio::fs::metadata(&marker).await {
            Ok(_) => return (current, None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return (
                    cwd.to_path_buf(),
                    Some(format!(
                        "failed to probe project root marker {}: {error}",
                        marker.display()
                    )),
                );
            }
        }
        let Some(parent) = current.parent() else {
            return (cwd.to_path_buf(), None);
        };
        if parent == current {
            return (cwd.to_path_buf(), None);
        }
        current = parent.to_path_buf();
    }
}

fn ancestor_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut current = cwd.to_path_buf();
    while current != root {
        chain.push(current.clone());
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent.to_path_buf();
    }
    chain.push(root.to_path_buf());
    chain.reverse();
    chain
}

fn extend_with_observed(
    dirs: &mut Vec<PathBuf>,
    cwd: &Path,
    observed: impl Iterator<Item = PathBuf>,
) {
    let mut observed: Vec<PathBuf> = observed.map(|path| absolute(&path)).collect();
    observed.sort();
    let mut extra: Vec<PathBuf> = Vec::new();
    for path in observed {
        let Some(parent) = path.parent() else {
            continue;
        };
        if !parent.starts_with(cwd) {
            continue;
        }
        for dir in ancestor_chain(cwd, parent).into_iter().skip(1) {
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
    for dir in extra {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
}

fn baseline_identity(
    cwd: &Path,
    project_root: &Path,
    data_dir: &Path,
    instruction_roots: &[PathBuf],
) -> String {
    let relative_cwd = relative_display(project_root, cwd);
    let mut roots = Vec::new();
    roots.push(data_dir.display().to_string());
    roots.extend(
        instruction_roots
            .iter()
            .map(|root| root.display().to_string()),
    );
    fnv_hex(&format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        relative_cwd,
        project_root.display(),
        PROJECT_ROOT_MARKER,
        MAX_CHAIN_BYTES,
        MAX_SOURCE_BYTES,
        INSTRUCTION_CANDIDATES.join(","),
        LOCAL_INSTRUCTION_CANDIDATES.join(",")
    ))
    .to_owned()
        + "|"
        + &roots.join(";")
}

async fn discover(
    data_dir: &Path,
    instruction_roots: &[PathBuf],
    project_root: &Path,
    dirs: &[PathBuf],
) -> Vec<InstructionFile> {
    let mut files = Vec::new();
    let mut global_roots: Vec<(PathBuf, String)> =
        vec![(data_dir.to_path_buf(), "$AIOS_HOME".to_owned())];
    global_roots.extend(
        instruction_roots
            .iter()
            .map(|root| (root.clone(), root.display().to_string())),
    );
    let mut seen_paths = BTreeSet::new();
    for (index, (root, display_root)) in global_roots.into_iter().enumerate() {
        let path = root.join(USER_GLOBAL_FILE);
        let absolute_path = absolute(&path);
        if !seen_paths.insert(absolute_path.clone()) {
            continue;
        }
        // 照 DSH：data-dir（harness home 的 AIOS 对应）与配置根是两个 user-global scope，
        // 同属一个候选组做去重；scope 分开才不会互相覆盖 digest。
        let scope_dir = if index == 0 {
            "user-global"
        } else {
            "agents-global"
        };
        let display = format!("{display_root}/{USER_GLOBAL_FILE}");
        if let Some(file) =
            read_candidate(&absolute_path, &display, scope_dir, Chain::UserGlobal).await
        {
            files.push(file);
        }
    }
    for dir in dirs {
        let scope = relative_scope(project_root, dir);
        for candidate in INSTRUCTION_CANDIDATES
            .iter()
            .chain(LOCAL_INSTRUCTION_CANDIDATES.iter())
        {
            let path = dir.join(candidate);
            let display = relative_display(project_root, &path);
            let scope_key = candidate_scope_key(&scope, candidate);
            if let Some(file) =
                read_candidate(&absolute(&path), &display, &scope_key, Chain::Project).await
            {
                files.push(file);
            }
        }
    }
    files
}

async fn read_candidate(
    path: &Path,
    display: &str,
    scope: &str,
    chain: Chain,
) -> Option<InstructionFile> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let content = tokio::fs::read_to_string(path).await.ok()?;
    let digest = fnv_hex(&content);
    let trimmed_digest = fnv_hex(content.trim());
    Some(InstructionFile {
        path: path.to_path_buf(),
        display: display.to_owned(),
        scope: scope.to_owned(),
        content,
        digest,
        trimmed_digest,
        chain,
    })
}

fn dedupe(files: Vec<InstructionFile>) -> Vec<InstructionFile> {
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut kept = Vec::new();
    for file in files {
        let group = candidate_group(&file);
        let digests = seen.entry(group).or_default();
        if !digests.insert(file.trimmed_digest.clone()) {
            continue;
        }
        kept.push(file);
    }
    kept
}

fn candidate_group(file: &InstructionFile) -> String {
    if file.chain == Chain::UserGlobal {
        "user-global".to_owned()
    } else {
        file.scope
            .split(SCOPE_SEPARATOR)
            .next()
            .unwrap_or(".")
            .to_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Baseline,
    Additional,
    Updated,
    Removed,
}

#[derive(Debug, Clone)]
struct RenderItem {
    file: InstructionFile,
    kind: SectionKind,
}

#[derive(Debug, Clone, Default)]
struct RenderedContext {
    text: String,
    omitted: Vec<OmittedFile>,
    truncated: Vec<TruncatedFile>,
    /// 渲染文本实际表示的文件（在输入项里的下标，按输入顺序）。
    represented: Vec<usize>,
}

fn byte_len(value: &str) -> usize {
    value.len()
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn escape_frame_body(body: &str) -> String {
    body.replace(FRAME_CLOSE, FRAME_CLOSE_ESCAPED)
}

fn section_text(file: &InstructionFile) -> String {
    format!("{SECTION_PREFIX}{}\n\n{}", file.display, file.content)
}

fn additional_section_text(file: &InstructionFile) -> String {
    let scope = file.scope.split(SCOPE_SEPARATOR).next().unwrap_or(".");
    let applicability = if matches!(file.chain, Chain::UserGlobal) {
        "These user-global instructions apply to all work."
    } else if scope == "." {
        "These instructions apply to work in this project."
    } else {
        return format!(
            "{ADDITIONAL_PREFIX}{}\n\nThese instructions apply to work under `{scope}`. {ADDITIONAL_INSTRUCTIONS_GUIDANCE}\n\n{}",
            file.display, file.content
        );
    };
    format!(
        "{ADDITIONAL_PREFIX}{}\n\n{applicability} {ADDITIONAL_INSTRUCTIONS_GUIDANCE}\n\n{}",
        file.display, file.content
    )
}

fn updated_section_text(file: &InstructionFile) -> String {
    format!(
        "{UPDATED_PREFIX}{}\n\nThis file changed after it was loaded. Use the following content instead of the previously loaded instructions from this file.\n\n{}",
        file.display, file.content
    )
}

fn removed_section_text(file: &InstructionFile) -> String {
    format!(
        "{REMOVED_PREFIX}{}\n\nThe previously loaded instructions from this file no longer apply.",
        file.display
    )
}

fn section_for(item: &RenderItem) -> String {
    match item.kind {
        SectionKind::Baseline => section_text(&item.file),
        SectionKind::Additional => additional_section_text(&item.file),
        SectionKind::Updated => updated_section_text(&item.file),
        SectionKind::Removed => removed_section_text(&item.file),
    }
}

fn marker_text(max_bytes: usize, omitted: &[OmittedFile], truncated: &[TruncatedFile]) -> String {
    if omitted.is_empty() && truncated.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    if !omitted.is_empty() {
        parts.push(format!(
            "omitted {}",
            omitted
                .iter()
                .map(|file| file.display.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !truncated.is_empty() {
        parts.push(format!(
            "truncated {}",
            truncated
                .iter()
                .map(|file| format!(
                    "{} from {} to {} bytes",
                    file.display, file.original_bytes, file.included_bytes
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    format!(
        "Workspace instruction budget {max_bytes} bytes: {}",
        parts.join("; ")
    )
}

fn build_instruction_text(
    items: &[RenderItem],
    max_bytes: usize,
    omitted: &[OmittedFile],
    truncated: &[TruncatedFile],
    intro: &str,
) -> String {
    let marker = marker_text(max_bytes, omitted, truncated);
    let mut blocks: Vec<String> = Vec::new();
    if !marker.is_empty() {
        blocks.push(marker);
    }
    if !intro.is_empty() {
        blocks.push(intro.to_owned());
    }
    blocks.extend(items.iter().map(section_for));
    let body = blocks
        .into_iter()
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{FRAME_OPEN}\n{}\n{FRAME_CLOSE}", escape_frame_body(&body))
}

fn with_truncated_content(file: &InstructionFile, included_bytes: usize) -> InstructionFile {
    InstructionFile {
        content: truncate_utf8(&file.content, included_bytes),
        ..file.clone()
    }
}

fn truncate_to_fit(
    item: &RenderItem,
    max_bytes: usize,
    omitted: &[OmittedFile],
    intro: &str,
) -> InstructionFile {
    let original_bytes = item.file.original_bytes();
    let mut low = 0_usize;
    let mut high = original_bytes;
    let mut best = with_truncated_content(&item.file, 0);
    while low <= high {
        let mid = low + (high - low) / 2;
        let candidate = with_truncated_content(&item.file, mid);
        let included_bytes = candidate.content.len();
        let truncated = [TruncatedFile {
            display: item.file.display.clone(),
            original_bytes,
            included_bytes,
        }];
        let candidate_item = RenderItem {
            file: candidate.clone(),
            kind: item.kind,
        };
        let text = build_instruction_text(&[candidate_item], max_bytes, omitted, &truncated, intro);
        if byte_len(&text) <= max_bytes {
            best = candidate;
            low = mid + 1;
        } else {
            if mid == 0 {
                break;
            }
            high = mid - 1;
        }
    }
    best
}

fn render_context(items: &[RenderItem], max_bytes: usize, intro: &str) -> RenderedContext {
    if max_bytes == 0 {
        return RenderedContext {
            omitted: items
                .iter()
                .map(|item| OmittedFile {
                    display: item.file.display.clone(),
                })
                .collect(),
            ..RenderedContext::default()
        };
    }
    let full_text = build_instruction_text(items, max_bytes, &[], &[], intro);
    if byte_len(&full_text) <= max_bytes {
        return RenderedContext {
            text: full_text,
            omitted: Vec::new(),
            truncated: Vec::new(),
            represented: (0..items.len()).collect(),
        };
    }
    for start in 1..items.len() {
        let included = &items[start..];
        let omitted: Vec<OmittedFile> = items[..start]
            .iter()
            .map(|item| OmittedFile {
                display: item.file.display.clone(),
            })
            .collect();
        let text = build_instruction_text(included, max_bytes, &omitted, &[], intro);
        if byte_len(&text) <= max_bytes {
            return RenderedContext {
                text,
                omitted,
                truncated: Vec::new(),
                represented: (start..items.len()).collect(),
            };
        }
    }
    let Some(last) = items.last() else {
        return RenderedContext::default();
    };
    let omitted: Vec<OmittedFile> = items[..items.len() - 1]
        .iter()
        .map(|item| OmittedFile {
            display: item.file.display.clone(),
        })
        .collect();
    let original_bytes = last.file.original_bytes();
    for candidate_intro in [
        intro.to_owned(),
        COMPACT_AGENT_INSTRUCTIONS_INTRO.to_owned(),
    ] {
        let truncated_file = truncate_to_fit(last, max_bytes, &omitted, &candidate_intro);
        let included_bytes = truncated_file.content.len();
        let truncated = [TruncatedFile {
            display: last.file.display.clone(),
            original_bytes,
            included_bytes,
        }];
        let candidate_item = RenderItem {
            file: truncated_file.clone(),
            kind: last.kind,
        };
        let text = build_instruction_text(
            &[candidate_item],
            max_bytes,
            &omitted,
            &truncated,
            &candidate_intro,
        );
        if byte_len(&text) <= max_bytes {
            let represented = if included_bytes > 0 || original_bytes == 0 {
                vec![items.len() - 1]
            } else {
                Vec::new()
            };
            return RenderedContext {
                text,
                omitted,
                truncated: truncated.to_vec(),
                represented,
            };
        }
    }
    let truncated = [TruncatedFile {
        display: last.file.display.clone(),
        original_bytes,
        included_bytes: 0,
    }];
    let compact_notice = escape_frame_body(&marker_text(max_bytes, &omitted, &truncated));
    let heading = RenderItem {
        file: with_truncated_content(&last.file, 0),
        kind: last.kind,
    };
    let compact_with_heading = escape_frame_body(
        &[compact_notice.clone(), section_for(&heading)]
            .into_iter()
            .filter(|block| !block.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    if byte_len(&compact_with_heading) <= max_bytes {
        let represented = if original_bytes == 0 {
            vec![items.len() - 1]
        } else {
            Vec::new()
        };
        return RenderedContext {
            text: compact_with_heading,
            omitted,
            truncated: truncated.to_vec(),
            represented,
        };
    }
    let text = if byte_len(&compact_notice) <= max_bytes {
        compact_notice
    } else {
        truncate_utf8(&compact_notice, max_bytes)
    };
    RenderedContext {
        text,
        omitted,
        truncated: truncated.to_vec(),
        represented: Vec::new(),
    }
}

/// DSH renderInstructionChains 的 intro 选择：有链保留 full-frame intro 就用 base intro，
/// 否则用第一个幸存的（compact）intro，都没有就不加 intro。
/// skeleton 先恒返回 base（行为不变），红测试后补选择逻辑。
fn merged_intro(base_intro: &str, _chain_texts: &[&str]) -> String {
    base_intro.to_owned()
}

fn render_chains(items: &[RenderItem], max_bytes: usize, intro: &str) -> RenderedContext {
    let whole = render_context(items, max_bytes, intro);
    let user_global: Vec<RenderItem> = items
        .iter()
        .filter(|item| item.file.chain == Chain::UserGlobal)
        .cloned()
        .collect();
    let project: Vec<RenderItem> = items
        .iter()
        .filter(|item| item.file.chain == Chain::Project)
        .cloned()
        .collect();
    if max_bytes == 0
        || user_global.is_empty()
        || project.is_empty()
        || (whole.omitted.is_empty() && whole.truncated.is_empty())
    {
        return whole;
    }
    let user_rendered = render_context(&user_global, max_bytes, intro);
    let project_rendered = render_context(&project, max_bytes, intro);
    let order_of = |display: &str| {
        items
            .iter()
            .position(|item| item.file.display == display)
            .unwrap_or(usize::MAX)
    };
    // 合并时按输入顺序还原（照 DSH inLoadedOrder）：chain 内部下标换算回全局下标。
    let mut represented: Vec<usize> = Vec::new();
    for (chain_items, chain) in [
        (&user_global, &user_rendered),
        (&project, &project_rendered),
    ] {
        for index in &chain.represented {
            let Some(item) = chain_items.get(*index) else {
                continue;
            };
            if let Some(global) = items
                .iter()
                .position(|candidate| candidate.file.display == item.file.display)
            {
                represented.push(global);
            }
        }
    }
    represented.sort_unstable();
    let sections: Vec<RenderItem> = represented
        .iter()
        .filter_map(|index| items.get(*index).cloned())
        .collect();
    let mut omitted: Vec<OmittedFile> = Vec::new();
    for chain in [&user_rendered, &project_rendered] {
        omitted.extend(chain.omitted.iter().cloned());
    }
    omitted.sort_by_key(|file| order_of(&file.display));
    let mut truncated: Vec<TruncatedFile> = Vec::new();
    for chain in [&user_rendered, &project_rendered] {
        truncated.extend(chain.truncated.iter().cloned());
    }
    truncated.sort_by_key(|file| order_of(&file.display));
    let chain_texts = [user_rendered.text.as_str(), project_rendered.text.as_str()];
    let merged_intro = merged_intro(intro, &chain_texts);
    let text = build_instruction_text(&sections, max_bytes, &omitted, &truncated, &merged_intro);
    if byte_len(&text) > 2 * max_bytes {
        return whole;
    }
    RenderedContext {
        text,
        omitted,
        truncated,
        represented,
    }
}

fn render_agent_instruction_set(
    files: &[InstructionFile],
    replace_previous_baseline: bool,
) -> (String, Vec<InstructionFile>) {
    let items: Vec<RenderItem> = files
        .iter()
        .map(|file| RenderItem {
            file: file.clone(),
            kind: SectionKind::Baseline,
        })
        .collect();
    let intro = if replace_previous_baseline {
        if files.is_empty() {
            EMPTY_REPLACEMENT_AGENT_INSTRUCTIONS_INTRO.to_owned()
        } else {
            REPLACEMENT_AGENT_INSTRUCTIONS_INTRO.to_owned()
        }
    } else {
        AGENT_INSTRUCTIONS_INTRO.to_owned()
    };
    let rendered = render_chains(&items, MAX_CHAIN_BYTES, &intro);
    let represented = rendered
        .represented
        .iter()
        .filter_map(|index| files.get(*index).cloned())
        .collect();
    (rendered.text, represented)
}

fn render_instruction_changes(items: &[ChangeItem]) -> (String, Vec<InstructionChange>) {
    let render_items: Vec<RenderItem> = items
        .iter()
        .map(|item| RenderItem {
            file: item.file.clone(),
            kind: match item.change.action {
                InstructionChangeAction::Set => SectionKind::Additional,
                InstructionChangeAction::Replace => SectionKind::Updated,
                InstructionChangeAction::Remove => SectionKind::Removed,
            },
        })
        .collect();
    let rendered = render_chains(&render_items, MAX_CHAIN_BYTES, "");
    let changes = rendered
        .represented
        .iter()
        .filter_map(|index| items.get(*index).map(|item| item.change.clone()))
        .collect();
    (rendered.text, changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(display: &str, scope_dir: &str, content: &str, chain: Chain) -> InstructionFile {
        InstructionFile {
            path: PathBuf::from(format!("/tmp/{display}")),
            display: display.to_owned(),
            scope: candidate_scope_key(scope_dir, "AGENTS.md"),
            content: content.to_owned(),
            digest: fnv_hex(content),
            trimmed_digest: fnv_hex(content.trim()),
            chain,
        }
    }

    #[test]
    fn digest_is_fnv_and_dedup_keys_ignore_surrounding_whitespace() {
        let same = dedupe(vec![
            file("a/AGENTS.md", "a", "hello", Chain::Project),
            file("a/CLAUDE.md", "a", "  hello\n", Chain::Project),
        ]);
        assert_eq!(same.len(), 1, "去首尾空白后相同内容只保留第一个");
        assert_eq!(fnv_hex("hello"), fnv_hex("hello"));
        assert_ne!(fnv_hex("hello"), fnv_hex("hello!"));
    }

    #[test]
    fn baseline_identity_is_stable_and_tracks_cwd_and_roots() {
        let root = Path::new("/repo");
        let data = Path::new("/data");
        let roots = vec![PathBuf::from("/home/.agents")];
        let first = baseline_identity(&root.join("a/b"), root, data, &roots);
        let same = baseline_identity(&root.join("a/b"), root, data, &roots);
        let other_cwd = baseline_identity(&root.join("a"), root, data, &roots);
        let other_roots = baseline_identity(&root.join("a/b"), root, data, &[PathBuf::from("/x")]);
        assert_eq!(first, same, "同一工作区 identity 稳定");
        assert_ne!(first, other_cwd, "cwd 变化改变 identity");
        assert_ne!(first, other_roots, "指令根变化改变 identity");
    }

    #[test]
    fn budget_omits_broadest_files_and_keeps_specific() {
        let broad = file(
            ".git/AGENTS.md",
            ".",
            &"b".repeat(MAX_CHAIN_BYTES / 2),
            Chain::Project,
        );
        let specific = file(
            "a/AGENTS.md",
            "a",
            &"s".repeat(MAX_CHAIN_BYTES / 2),
            Chain::Project,
        );
        let (text, represented) = render_agent_instruction_set(&[broad, specific], false);
        assert!(text.len() <= MAX_CHAIN_BYTES, "单链预算内：{}", text.len());
        assert!(
            text.contains("Workspace instruction budget")
                && text.contains("omitted .git/AGENTS.md"),
            "先省略最宽泛文件：{text}"
        );
        assert!(!text.contains(&"b".repeat(1000)), "宽泛文件内容不出现");
        assert_eq!(represented.len(), 1);
        assert_eq!(represented[0].display, "a/AGENTS.md");
    }

    #[test]
    fn budget_truncates_most_specific_and_is_utf8_safe() {
        let huge = file(
            "a/AGENTS.md",
            "a",
            &"指".repeat(MAX_CHAIN_BYTES),
            Chain::Project,
        );
        let (text, represented) = render_agent_instruction_set(&[huge], false);
        assert!(text.len() <= MAX_CHAIN_BYTES, "预算内：{}", text.len());
        assert!(
            text.contains("truncated a/AGENTS.md") && text.contains("bytes"),
            "截断 marker：{text}"
        );
        assert!(
            text.contains(COMPACT_AGENT_INSTRUCTIONS_INTRO)
                || text.contains(AGENT_INSTRUCTIONS_INTRO),
            "多级回退保留 intro/compact intro：{text}"
        );
        assert_eq!(represented.len(), 1, "截断后仍表示该文件（有内容存活）");
        // UTF-8 安全：truncate_utf8 结果必须是合法字符串（Rust String 天然保证），且不超预算。
        assert!(text.is_char_boundary(text.len()));
    }

    #[test]
    fn changes_render_set_replace_remove_texts() {
        let set_file = file("a/AGENTS.md", "a", "set content", Chain::Project);
        let updated_file = file("x/AGENTS.md", "x", "updated content", Chain::Project);
        let removed_file = file("old/AGENTS.md", "old", "", Chain::Project);
        let items = vec![
            ChangeItem {
                change: set_change(&set_file, InstructionChangeAction::Set),
                file: set_file,
            },
            ChangeItem {
                change: set_change(&updated_file, InstructionChangeAction::Replace),
                file: updated_file,
            },
            ChangeItem {
                change: InstructionChange {
                    action: InstructionChangeAction::Remove,
                    scope: removed_file.scope.clone(),
                    path: removed_file.display.clone(),
                    digest: None,
                },
                file: removed_file,
            },
        ];
        let (text, represented) = render_instruction_changes(&items);
        assert!(
            text.contains("Additional instructions from: a/AGENTS.md"),
            "{text}"
        );
        assert!(
            text.contains("Updated instructions from: x/AGENTS.md"),
            "{text}"
        );
        assert!(
            text.contains("Instructions removed: old/AGENTS.md"),
            "{text}"
        );
        assert!(
            text.contains("The previously loaded instructions from this file no longer apply.")
        );
        assert_eq!(represented.len(), 3, "小批量全部渲染");
        assert!(text.starts_with(FRAME_OPEN) && text.ends_with(FRAME_CLOSE));
    }

    #[test]
    fn typed_source_serializes_dsh_shape() {
        let message = Message::agent_instructions(
            "body",
            true,
            Some("identity".to_owned()),
            vec![InstructionChange {
                action: InstructionChangeAction::Set,
                scope: "user-global\u{0}AGENTS.md".to_owned(),
                path: "$AIOS_HOME/AGENTS.md".to_owned(),
                digest: Some("abc".to_owned()),
            }],
        );
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(
            value["source"],
            serde_json::json!({
                "kind": "agent-instructions",
                "form": "instructions",
                "baseline": true,
                "baselineIdentity": "identity",
                "changes": [{"action": "set", "scope": "user-global\u{0}AGENTS.md", "path": "$AIOS_HOME/AGENTS.md", "digest": "abc"}],
            })
        );
        assert_eq!(value["role"], "user");
    }

    #[test]
    fn rehydrate_folds_typed_sources_and_baseline_identity() {
        let baseline = Message::agent_instructions(
            "baseline",
            true,
            Some("id-1".to_owned()),
            vec![
                InstructionChange {
                    action: InstructionChangeAction::Set,
                    scope: "a\u{0}AGENTS.md".to_owned(),
                    path: "a/AGENTS.md".to_owned(),
                    digest: Some("d1".to_owned()),
                },
                InstructionChange {
                    action: InstructionChangeAction::Set,
                    scope: "b\u{0}AGENTS.md".to_owned(),
                    path: "b/AGENTS.md".to_owned(),
                    digest: Some("d2".to_owned()),
                },
            ],
        );
        let update = Message::agent_instructions(
            "update",
            false,
            None,
            vec![
                InstructionChange {
                    action: InstructionChangeAction::Replace,
                    scope: "a\u{0}AGENTS.md".to_owned(),
                    path: "a/AGENTS.md".to_owned(),
                    digest: Some("d3".to_owned()),
                },
                InstructionChange {
                    action: InstructionChangeAction::Remove,
                    scope: "b\u{0}AGENTS.md".to_owned(),
                    path: "b/AGENTS.md".to_owned(),
                    digest: None,
                },
            ],
        );
        let mut state = InstructionState::default();
        state.rehydrate(&[baseline, update]);
        assert_eq!(state.visible_identity.as_deref(), Some("id-1"));
        assert_eq!(state.scopes.len(), 1);
        assert_eq!(state.scopes["a\u{0}AGENTS.md"].digest, "d3");
        assert!(!state.scopes.contains_key("b\u{0}AGENTS.md"));
    }
}

// __END__
