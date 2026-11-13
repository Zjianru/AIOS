//! 按条件开出的工具：本会话加载过哪些 skill、哪些工具已经记进日志。
//!
//! 条件挂在工具上（`tools::ToolCondition`），每步开始时求值一次。求值的依据是会话日志：
//! `skill` 工具成功加载过哪些名字；加载记录（`tool/registry`）已经记过的工具继续开着
//! （只加不减）。两者都从日志里恢复，所以续接以后工具表和续接前一样。

use std::collections::{BTreeMap, BTreeSet};

use model::{Message, Role, ToolAddition, ToolHistory, ToolSchema};
use session::{Event, EventKind};
use tools::ToolSet;

/// `skill` 工具名（加载成功是开出工具的依据）。
const SKILL_TOOL: &str = "skill";

/// 成功加载过的 skill 名；`turn` 是 Some(n) 时只看第 n 轮，None 看整个会话。
///
/// 只看 `skill` 调用的结果：失败的加载不算（工具结果 is_error，或带结构化错误）。
/// 工具条件按会话级求值，确认卡这类动作在同一轮执行时再按轮收紧，两处共用这一份解析。
pub fn loaded_skills(events: &[Event], turn: Option<u64>) -> BTreeSet<String> {
    let mut requested: BTreeMap<&str, String> = BTreeMap::new();
    for event in events {
        if let EventKind::ToolCall {
            turn: call_turn,
            call_id,
            name,
            arguments,
            ..
        } = &event.kind
            && turn.is_none_or(|scope| *call_turn == scope)
            && name == SKILL_TOOL
            && let Some(skill) = skill_name_of(arguments)
        {
            requested.insert(call_id.as_str(), skill);
        }
    }
    let mut loaded = BTreeSet::new();
    for event in events {
        let EventKind::ToolResult {
            turn: result_turn,
            message,
            error,
            ..
        } = &event.kind
        else {
            continue;
        };
        if turn.is_some_and(|scope| *result_turn != scope)
            || message.is_error == Some(true)
            || error.is_some()
        {
            continue;
        }
        let Some(call_id) = message.tool_call_id.as_deref() else {
            continue;
        };
        if let Some(skill) = requested.get(call_id) {
            loaded.insert(skill.clone());
        }
    }
    loaded
}

/// T046：未声明 addition-only 的路由，本步工具表 = 该会话角色 ToolSet 的全量成员（技能条件不再改变
/// 工具表）。角色/安全边界在构造期 retain_named 已收窄，这里只按 ToolSet 成员开，不会跨边界。
pub(crate) fn all_tool_names(tools: &ToolSet) -> BTreeSet<String> {
    tools.names().into_iter().map(str::to_owned).collect()
}

/// 一次请求的工具表与加载历史：条件开出的工具 + 从日志重建的声明序列。
///
/// 主请求与压缩摘要请求共用这一份计算（压缩请求不能带没开出的工具）。
pub(crate) fn request_tools(
    tools: &ToolSet,
    events: &[Event],
    derived: &[(u64, Message)],
    tool_keys: &BTreeSet<tools::ToolKey>,
) -> (Vec<ToolSchema>, Option<ToolHistory>) {
    let loaded = loaded_skills(events, None);
    let registry = ToolRegistryState::rehydrate(events);
    let enabled = registry.enabled(tools, &loaded, tool_keys);
    (
        tools.definitions_for(&enabled),
        tool_history(events, derived),
    )
}

/// 从会话日志恢复请求投影要用的加载历史（主请求与压缩摘要请求共用）。
///
/// 第一条 `tool/registry` 是第一次请求的声明（立即可用）；之后每条是某个加载点。
/// 加载点用「它前面最后一条可见的 user/tool 消息」定位：tool_addition 只能紧跟 user 或
/// tool-result 回合，日志只追加，所以续接前后算出来一样。加载点被压缩 shadow 掉以后，
/// 前面只剩 system，锚点为 None——投影退回当前完整工具表，不把 tool_addition 插到 system 后面。
pub(crate) fn tool_history(events: &[Event], derived: &[(u64, Message)]) -> Option<ToolHistory> {
    let mut registries = events.iter().filter_map(|event| match &event.kind {
        EventKind::ToolRegistry { additions, .. } => Some((event.seq, additions)),
        _ => None,
    });
    let (_, baseline) = registries.next()?;
    let mut updates = Vec::new();
    for (seq, additions) in registries {
        let after = derived
            .iter()
            .rev()
            .find(|(message_seq, message)| {
                *message_seq < seq && matches!(message.role, Role::User | Role::Tool)
            })
            .map(|(_, message)| message.id.clone());
        updates.push(ToolAddition {
            after,
            additions: additions.clone(),
        });
    }
    Some(ToolHistory {
        baseline: baseline.clone(),
        updates,
    })
}

fn skill_name_of(arguments: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(arguments).ok()?;
    value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// 加载记录的内存状态：已经记过的工具名（从日志恢复）。
#[derive(Debug, Default)]
pub(crate) struct ToolRegistryState {
    recorded: BTreeSet<String>,
}

impl ToolRegistryState {
    /// 从会话日志重建（续接时）：把每条 `tool/registry` 的 additions 收进来。
    pub(crate) fn rehydrate(events: &[Event]) -> Self {
        let mut recorded = BTreeSet::new();
        for event in events {
            if let EventKind::ToolRegistry { additions, .. } = &event.kind {
                for tool in additions {
                    recorded.insert(tool.name.clone());
                }
            }
        }
        Self { recorded }
    }

    /// 本步的工具表：条件满足的，加上已经记过的（只加不减；工具集里没有的名字忽略）。
    pub(crate) fn enabled(
        &self,
        tools: &ToolSet,
        loaded_skills: &BTreeSet<String>,
        tool_keys: &BTreeSet<tools::ToolKey>,
    ) -> BTreeSet<String> {
        let mut enabled = tools.enabled(loaded_skills, tool_keys);
        for name in &self.recorded {
            if tools.get(name).is_some() {
                enabled.insert(name.clone());
            }
        }
        enabled
    }

    /// 本步要记进日志的新增工具（带完整定义，按注册顺序）；没有新增就是空。
    pub(crate) fn additions(&self, tools: &ToolSet, enabled: &BTreeSet<String>) -> Vec<ToolSchema> {
        tools
            .definitions_for(enabled)
            .into_iter()
            .filter(|tool| !self.recorded.contains(&tool.name))
            .collect()
    }

    /// 记一条加载记录（日志追加成功之后调用）。
    pub(crate) fn commit(&mut self, additions: &[ToolSchema]) {
        for tool in additions {
            self.recorded.insert(tool.name.clone());
        }
    }
}

/// T046：未声明 addition-only 的路由，工具表从第一轮起就是该会话角色 ToolSet 的全量成员。
#[cfg(test)]
mod t046_stable_tool_tests {
    use std::collections::BTreeSet;

    use tools::ToolSet;

    use super::*;

    fn retained(names: &[&str]) -> ToolSet {
        let mut tools = ToolSet::builtin();
        let keep: BTreeSet<String> = names.iter().map(|name| (*name).to_owned()).collect();
        tools.retain_named(&keep);
        tools
    }

    /// 同一会话跨轮 + 加载 skill 前后：不支持 addition-only 的路由，工具名列表不变（全量固定）。
    #[test]
    fn fixed_selection_ignores_loaded_skills_and_turns() {
        let tools = ToolSet::builtin();
        let before = all_tool_names(&tools);
        assert_eq!(before, tools.names().into_iter().map(str::to_owned).collect::<BTreeSet<_>>());
        // 加载过 skill 的那一轮，gate 的 enabled 会变；固定选择不看它。
        let mut skills = BTreeSet::new();
        skills.insert("web".to_owned());
        let after = all_tool_names(&tools);
        assert_eq!(before, after, "加载 skill 前后工具名列表必须一致");
    }

    /// 角色/安全边界在构造期 retain 之后仍然成立：全开只开该角色 ToolSet 里的成员。
    #[test]
    fn full_open_never_crosses_the_role_boundary() {
        // Kind 的 only 名单里没有 write/edit/bash/job_*（root.rs KIND_TOOLS）。
        let kind_like = retained(&[
            "read",
            "glob",
            "grep",
            "skill",
            "todo_write",
            "ask_user_question",
            "canvas",
        ]);
        let names = all_tool_names(&kind_like);
        for forbidden in ["write", "edit", "bash", "job_output", "job_list", "job_kill"] {
            assert!(!names.contains(forbidden), "Kind 全开后不能出现 {forbidden}: {names:?}");
        }
        // ThinAgent 只读任务：没有 write/edit/bash/job_*。
        let thin_read = retained(&["read", "glob", "grep", "skill", "todo_write"]);
        let names = all_tool_names(&thin_read);
        for forbidden in ["write", "edit", "bash", "job_output"] {
            assert!(!names.contains(forbidden), "ThinAgent 只读全开后不能出现 {forbidden}");
        }
        // 写任务名单有 write/edit，但没有 bash/job_*。
        let thin_write = retained(&["read", "glob", "grep", "write", "edit", "skill", "todo_write"]);
        let names = all_tool_names(&thin_write);
        assert!(names.contains("write") && names.contains("edit"));
        assert!(!names.contains("bash"));
    }
}
