//! 对话中途加载工具：路由能力与请求投影（照 DSH 的 `projectToolUpdates`）。
//!
//! 声明了能力的路由（目前只有 `addition-only`）：加载过的工具在声明里带 `defer_loading`，
//! 加载处插一条 system 角色的 `tool_addition`，请求头加 beta。没声明的路由发当前完整工具表，
//! 不带这些。投影只依赖日志恢复出的 [`ToolHistory`]，所以续接前后同一步的请求逐字节相同。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{Message, MessageId, ToolSchema};

/// 路由声明支持的对话中途工具更新模式。AIOS 只认 `addition-only`（不做移除/in-history）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ToolUpdate {
    AdditionOnly,
}

impl ToolUpdate {
    /// 从目录/厂商列表的字符串读能力；不认识的值读成 None（按不支持处理）。
    pub fn parse(raw: &str) -> Option<Self> {
        (raw == "addition-only").then_some(Self::AdditionOnly)
    }
}

/// 一次请求的工具表与加载历史（模型层投影的输入）。
#[derive(Debug, Clone, Default)]
pub struct ToolRequest {
    /// 本步开出的工具定义（当前有效表）。
    pub tools: Vec<ToolSchema>,
    /// 从日志恢复的声明序列；None = 没有加载记录（按没声明能力处理）。
    pub history: Option<ToolHistory>,
}

/// 从会话日志恢复的声明序列：第一次请求的声明 + 按位置排的加载。
#[derive(Debug, Clone, Default)]
pub struct ToolHistory {
    /// 第一次请求就带来的声明（立即可用，不延迟）。
    pub baseline: Vec<ToolSchema>,
    /// 之后按日志顺序的加载点。
    pub updates: Vec<ToolAddition>,
}

/// 一次加载：在哪条消息之后、加了哪些工具。
#[derive(Debug, Clone)]
pub struct ToolAddition {
    /// 加在哪条消息之后；None = 请求里没有可锚定的 user/tool 消息（还没有消息，或加载点被压缩 shadow）。
    pub after: Option<MessageId>,
    /// 这次新加的工具（完整定义）。
    pub additions: Vec<ToolSchema>,
}

/// 一次投影的结果：线上工具声明 + 要插入的 `tool_addition`。
pub(crate) struct ProjectedTools {
    /// 线上声明（加载过的带 `defer_loading`）。
    pub tools: Vec<ToolSchema>,
    /// 消息下标 → 该条消息之后插入一条 system `tool_addition`（按名字）。
    pub insertions: BTreeMap<usize, Vec<String>>,
}

/// 按路由能力投影一次请求。没声明能力的路由发当前完整工具表，不带 `defer_loading`。
pub(crate) fn project(
    messages: &[Message],
    request: &ToolRequest,
    update: Option<ToolUpdate>,
) -> ProjectedTools {
    let immediate = || ProjectedTools {
        tools: request
            .tools
            .iter()
            .map(|tool| {
                let mut tool = tool.clone();
                tool.defer_loading = false;
                tool
            })
            .collect(),
        insertions: BTreeMap::new(),
    };
    let Some(ToolUpdate::AdditionOnly) = update else {
        return immediate();
    };
    let Some(history) = &request.history else {
        return immediate();
    };
    // 加载点必须都锚在请求里的 user/tool 消息上；辅助请求（压缩前缀）截掉了加载点、
    // 或加载点被压缩 shadow 掉时，退回当前完整声明。
    let positions: BTreeMap<&str, usize> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| (message.id.as_str(), index))
        .collect();
    if history
        .updates
        .iter()
        .any(|addition| match &addition.after {
            None => true,
            Some(after) => !positions.contains_key(after.as_str()),
        })
    {
        return immediate();
    }

    // 声明 = 基线 + 按加载顺序追加（后加载的带 defer_loading）；只保留当前有效的工具。
    let mut declarations: Vec<ToolSchema> = Vec::new();
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for tool in &history.baseline {
        if declared.insert(tool.name.clone()) {
            let mut tool = tool.clone();
            tool.defer_loading = false;
            declarations.push(tool);
        }
    }
    for addition in &history.updates {
        for tool in &addition.additions {
            if declared.insert(tool.name.clone()) {
                let mut tool = tool.clone();
                tool.defer_loading = true;
                declarations.push(tool);
            }
        }
    }
    let active: BTreeSet<&str> = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    declarations.retain(|tool| active.contains(tool.name.as_str()));

    // 基线工具第一次请求就可用；每个加载点只报还没激活的声明。
    let mut offered: BTreeSet<String> = history
        .baseline
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    let mut insertions: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for addition in &history.updates {
        let Some(after) = &addition.after else {
            continue;
        };
        let Some(index) = positions.get(after.as_str()).copied() else {
            continue;
        };
        let names: Vec<String> = addition
            .additions
            .iter()
            .filter(|tool| declared.contains(&tool.name) && !offered.contains(&tool.name))
            .map(|tool| tool.name.clone())
            .collect();
        if names.is_empty() {
            continue;
        }
        offered.extend(names.iter().cloned());
        insertions.entry(index).or_default().extend(names);
    }

    ProjectedTools {
        tools: declarations,
        insertions,
    }
}
