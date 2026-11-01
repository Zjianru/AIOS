//! skill 目录注入：发现可用 skill，把「名字 + description」渲染成一条持久 user/message。
//!
//! 选型：照 DSH tool-skill 的做法把目录作为独立的 system-reminder 上下文块（user 角色），
//! 而不是塞进系统提示：目录随文件系统变化，单独一条消息可以按变化替换/续接对账，
//! 也不让系统提示承担可变内容（T005.5 的指令注入同理）。

use std::path::{Path, PathBuf};

use model::{ContentBlock, Message};

use tools::{SkillSummary, catalog_description, discover_skills};

/// 会话里的 skill 目录状态：上次渲染的完整文本（用于去重/续接对账）。
#[derive(Debug, Default)]
pub struct SkillCatalogState {
    rendered: Option<String>,
}

impl SkillCatalogState {
    /// 从既有会话消息对账（续接）：找到之前注入过的目录消息，避免重复注入。
    pub fn rehydrate(&mut self, messages: &[Message]) {
        for message in messages {
            for block in &message.content {
                let ContentBlock::Text { text } = block else {
                    continue;
                };
                if text.contains("<available_skills>") {
                    self.rendered = Some(text.clone());
                }
            }
        }
    }

    /// 每个 step 组装请求前调用：目录有变化时返回要追加的 user/message 文本。
    pub fn prepare(
        &mut self,
        cwd: &Path,
        data_dir: &Path,
        skill_roots: &[PathBuf],
    ) -> Option<String> {
        let skills = discover_skills(cwd, data_dir, skill_roots);
        if skills.is_empty() {
            if self.rendered.take().is_some() {
                return Some(render_empty());
            }
            return None;
        }
        let text = render_catalog(&skills);
        if self.rendered.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.rendered = Some(text.clone());
        Some(text)
    }
}

fn render_entries(skills: &[SkillSummary]) -> Vec<String> {
    skills
        .iter()
        .map(|skill| {
            format!(
                "- `{}`: {}",
                skill.name,
                catalog_description(&skill.description)
            )
        })
        .collect()
}

fn render_catalog(skills: &[SkillSummary]) -> String {
    let mut lines = vec![
        "<system-reminder>".to_owned(),
        "A skill is a reusable set of task-specific instructions. The following skills are available in this session:".to_owned(),
        String::new(),
        "<available_skills>".to_owned(),
    ];
    lines.extend(render_entries(skills));
    lines.extend([
        "</available_skills>".to_owned(),
        String::new(),
        "If the user names a skill, or the task clearly matches a skill's description, call the `skill` tool with the exact skill name before taking task actions. Load all applicable skills, then follow their full instructions. This catalog contains summaries only; do not infer or follow a skill's instructions until it has been loaded.".to_owned(),
        "</system-reminder>".to_owned(),
    ]);
    lines.join("\n")
}

fn render_empty() -> String {
    [
        "<system-reminder>",
        "No skills are currently available through the `skill` tool. Do not use names from earlier skill catalogs.",
        "</system-reminder>",
    ]
    .join("\n")
}
