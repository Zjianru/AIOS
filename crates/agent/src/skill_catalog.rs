//! skill 目录注入（T006.11 照 DSH tool-skill 改回）。
//!
//! 目录是带类型来源的持久 user/message（source=skill-catalog：form=catalog、update/incomplete、
//! entries），首次发布=完整目录，之后按 DSH 替换目录协议发 update=true 的完整新目录；
//! 内容 identity 用 entries 的 JSON 规范化 + 完整性标记（DSH SHA-256 的 FNV-1a 适配，不新增依赖）。
//! 非法 skill 的发现警告由 tools 返回，能力层去重后打到 stderr（AIOS 暂无 logger 面，T006.17）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::warn::warn;
use model::{Message, MessageSource, SkillCatalogEntry};
use tools::{BundledSkill, ToolKey, catalog_description, discover_skill_catalog};

/// 会话里的 skill 目录状态：上次发布的 identity + 已记录的警告（去重）。
#[derive(Debug, Default)]
pub struct SkillCatalogState {
    published_digest: Option<String>,
    logged_warnings: BTreeSet<String>,
}

impl SkillCatalogState {
    /// 从既有会话消息对账（续接）：按带类型 source 折叠最后一次目录发布的 identity。
    pub fn rehydrate(&mut self, messages: &[Message]) {
        for message in messages {
            let MessageSource::SkillCatalog {
                incomplete,
                entries,
                ..
            } = &message.source
            else {
                continue;
            };
            self.published_digest = Some(digest_entries(entries, *incomplete));
        }
    }

    /// 每个 step 组装请求前调用：目录首次发布或有变化时返回带类型 source 的 user 消息。
    ///
    /// `tool_keys` 决定哪些自带 skill 现在能进目录；缺凭证的那些不进目录，但会在目录里
    /// 留一句为什么、该让用户去哪里填。
    pub fn prepare(
        &mut self,
        cwd: &Path,
        data_dir: &Path,
        skill_roots: &[PathBuf],
        bundled: &[BundledSkill],
        tool_keys: &BTreeSet<ToolKey>,
    ) -> Option<Message> {
        let available: Vec<BundledSkill> = bundled
            .iter()
            .copied()
            .filter(|skill| skill.available(tool_keys))
            .collect();
        let discovery = discover_skill_catalog(cwd, data_dir, skill_roots, &available);
        for warning in &discovery.warnings {
            // 警告文案在 tools 层已按 DSH skill-filesystem 的 logger.warn 形状生成
            // （`skill file <path> ignored: <reason>` 等），这里只做去重与落 stderr。
            let text = warning.message.clone();
            if self.logged_warnings.insert(text.clone()) {
                // AIOS 没有 logger 设施（T006.17 日志面），先落 stderr，不静默吞。
                warn(format!("[skill] {text}"));
            }
        }
        let entries: Vec<SkillCatalogEntry> = discovery
            .skills
            .iter()
            .map(|skill| SkillCatalogEntry {
                name: skill.name.clone(),
                description: catalog_description(&skill.description),
            })
            .collect();
        let digest = digest_entries(&entries, discovery.incomplete);
        let published = self.published_digest.is_some();
        if self.published_digest.as_deref() == Some(digest.as_str()) {
            return None;
        }
        // 照 DSH：没有发布过、没有 skill、发现完整时不注入空目录。
        if !published && entries.is_empty() && !discovery.incomplete {
            return None;
        }
        // 缺凭证的自带 skill：同名用户 skill 已经进目录时不再提它（用户那份才可用）。
        let unavailable: Vec<BundledSkill> = bundled
            .iter()
            .copied()
            .filter(|skill| !skill.available(tool_keys))
            .filter(|skill| !entries.iter().any(|entry| entry.name == skill.name))
            .collect();
        // 提示不进 digest：它跟着 entries 变——某个自带 skill 缺凭证时它一定不在 entries 里，
        // 凭证补上以后 entries 多了它、digest 跟着变，目录自然重发。续接时不必单独记提示。
        let text = if published {
            render_catalog_update(&entries, discovery.incomplete, &unavailable)
        } else {
            render_catalog_first(&entries, discovery.incomplete, &unavailable)
        };
        self.published_digest = Some(digest);
        Some(Message::skill_catalog(
            text,
            published,
            discovery.incomplete,
            entries,
        ))
    }
}

const INCOMPLETE_NOTICE: &str = "Some skills could not be checked. Only confirmed skills are listed; a lower-priority definition of the same name may be selected when a preferred source is unavailable. Discovery will be retried on the next request.";

fn escape_text(text: &str) -> String {
    text.replace("</system-reminder>", "<\\/system-reminder>")
}

fn entry_lines(entries: &[SkillCatalogEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            format!(
                "- \u{60}{}\u{60}: {}",
                entry.name,
                escape_text(&entry.description)
            )
        })
        .collect()
}

fn catalog_body(
    entries: &[SkillCatalogEntry],
    incomplete: bool,
    unavailable: &[BundledSkill],
) -> Vec<String> {
    let mut lines = vec![
        "<system-reminder>".to_owned(),
        if entries.is_empty() {
            "No skills could be confirmed in this discovery attempt.".to_owned()
        } else {
            "A skill is a reusable set of task-specific instructions. The following skills are available in this session:".to_owned()
        },
        String::new(),
        "<available_skills>".to_owned(),
    ];
    lines.extend(entry_lines(entries));
    lines.push("</available_skills>".to_owned());
    lines.extend(unavailable_lines(unavailable));
    lines.push(String::new());
    if incomplete {
        lines.push(INCOMPLETE_NOTICE.to_owned());
        lines.push(String::new());
    }
    lines.push(if entries.is_empty() {
        "Wait for an updated catalog before calling the \u{60}skill\u{60} tool. Do not infer skill instructions from an empty catalog.".to_owned()
    } else {
        "If the user names a skill, or the task clearly matches a skill's description, call the \u{60}skill\u{60} tool with the exact skill name before taking task actions. Load all applicable skills, then follow their full instructions. This catalog contains summaries only; do not infer or follow a skill's instructions until it has been loaded.".to_owned()
    });
    lines.push("A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the \u{60}skill\u{60} tool again for that skill.".to_owned());
    lines.push("</system-reminder>".to_owned());
    lines
}

fn render_catalog_first(
    entries: &[SkillCatalogEntry],
    incomplete: bool,
    unavailable: &[BundledSkill],
) -> String {
    catalog_body(entries, incomplete, unavailable).join("\n")
}

fn render_catalog_update(
    entries: &[SkillCatalogEntry],
    incomplete: bool,
    unavailable: &[BundledSkill],
) -> String {
    let header = if incomplete {
        "Skill discovery is incomplete. This catalog of currently available skills replaces every earlier available-skills list in this session:"
    } else {
        "The available skill catalog changed. This complete catalog replaces every earlier available-skills list in this session:"
    };
    let (availability, user_invocation) = if entries.is_empty() {
        (
            if incomplete {
                "No skills could be confirmed in this discovery attempt. Do not use names from earlier skill catalogs."
            } else {
                "No skills are currently available through the \u{60}skill\u{60} tool. Do not use names from earlier skill catalogs."
            },
            "A user may still invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the \u{60}skill\u{60} tool for it.",
        )
    } else if incomplete {
        (
            "Use only names in this replacement catalog. If the user names a listed skill, or the task clearly matches its description, call the \u{60}skill\u{60} tool with the exact name before acting.",
            "A user may still invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the \u{60}skill\u{60} tool again for that skill.",
        )
    } else {
        (
            "Use only names in this replacement catalog. If the user names a listed skill, or the task clearly matches its description, call the \u{60}skill\u{60} tool with the exact name before acting.",
            "A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the \u{60}skill\u{60} tool again for that skill.",
        )
    };
    let mut lines = vec![
        "<system-reminder>".to_owned(),
        header.to_owned(),
        String::new(),
        "<available_skills>".to_owned(),
    ];
    lines.extend(entry_lines(entries));
    lines.push("</available_skills>".to_owned());
    lines.extend(unavailable_lines(unavailable));
    lines.push(String::new());
    if incomplete {
        lines.push(INCOMPLETE_NOTICE.to_owned());
        lines.push(String::new());
    }
    lines.push(availability.to_owned());
    lines.push(user_invocation.to_owned());
    lines.push("</system-reminder>".to_owned());
    lines.join("\n")
}

/// 缺工具凭证的自带 skill：名字 + 一句为什么、去哪里填。没有就什么都不加。
fn unavailable_lines(unavailable: &[BundledSkill]) -> Vec<String> {
    if unavailable.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![String::new(), "<unavailable_skills>".to_owned()];
    for skill in unavailable {
        lines.push(format!(
            "- `{}`: {}",
            skill.name,
            escape_text(skill.unavailable_hint)
        ));
    }
    lines.push("</unavailable_skills>".to_owned());
    lines
}

fn digest_entries(entries: &[SkillCatalogEntry], incomplete: bool) -> String {
    let mut canonical = String::new();
    if incomplete {
        canonical.push_str("incomplete\n");
    }
    for entry in entries {
        canonical.push_str(
            &serde_json::to_string(&(entry.name.clone(), entry.description.clone()))
                .unwrap_or_default(),
        );
        canonical.push('\n');
    }
    fnv_hex(&canonical)
}

fn fnv_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
