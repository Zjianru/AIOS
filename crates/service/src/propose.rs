//! 提议工具 propose_action：给用户一张确认卡，卡片下面什么都不做，用户点了才执行。
//!
//! 开出的条件是本轮之前加载过登记了动作的 skill；调用时再查一次本轮加载过没有。
//! 拒绝的原因直接回给模型（照 action-cards 的措辞）；接受时的工具结果是一句固定原文，
//! 后面写回答的模型照着它邀请用户，不重复卡片内容。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tools::{Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolResult};

use crate::actions::{ActionEnv, ActionRegistry};
use crate::cards::{self, CardMetaOption, ProposeArgs};

/// 接受时照抄的结果原文：一句话说卡片在回答下面，一句邀请。
pub(crate) const ACCEPTED_RESULT: &str = "Card shown below your answer. Do not restate it. Invite the user to confirm or dismiss the card.";

/// 提议工具。
pub struct ProposeActionTool {
    actions: Arc<ActionRegistry>,
    data_dir: PathBuf,
    identity: Option<PathBuf>,
    parameters: Value,
}

impl ProposeActionTool {
    pub fn new(
        actions: Arc<ActionRegistry>,
        data_dir: PathBuf,
        identity: Option<PathBuf>,
    ) -> Self {
        Self {
            actions,
            data_dir,
            identity,
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Card title. One short line." },
                    "brief": { "type": "string", "description": "What the action does, in the user's language." },
                    "options": {
                        "type": "array",
                        "minItems": 1,
                        "description": "One option per choice the user can make; the card asks the user to pick one and confirm.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Unique within this card." },
                                "action": { "type": "string", "description": "Action name from the loaded skill." },
                                "label": { "type": "string", "description": "Everyday name of the choice." },
                                "description": { "type": "string", "description": "One sentence for this option." },
                                "details": { "type": "object", "description": "Fields the skill lists for this action." }
                            },
                            "required": ["id", "action", "label", "details"]
                        }
                    }
                },
                "required": ["title", "brief", "options"]
            }),
        }
    }

    fn propose(&self, call: &ToolCall, context: &ToolContext) -> Result<ToolResult, ToolFailure> {
        let args: ProposeArgs = serde_json::from_str(&call.arguments).map_err(|error| {
            ToolFailure::plain(format!("arguments must be a JSON object: {error}"))
        })?;
        let title = args.title.trim();
        let brief = args.brief.trim();
        if title.is_empty() || brief.is_empty() {
            return Err(ToolFailure::plain("Write a title and a brief."));
        }
        if args.options.is_empty() {
            return Err(ToolFailure::plain("Give the card at least one option."));
        }
        let mut ids = BTreeSet::new();
        for option in &args.options {
            if option.id.trim().is_empty() || !ids.insert(option.id.as_str()) {
                return Err(ToolFailure::plain("Option ids must be unique."));
            }
        }
        let events = context.session_events();
        let turn = current_turn(&events);
        let loaded = skills_loaded_in_turn(&events, turn);
        if earlier_card_in_turn(&events, turn, &call.id) {
            return Err(ToolFailure::plain(
                "One card per turn; the earlier card stands.",
            ));
        }
        let env = ActionEnv {
            data_dir: &self.data_dir,
            identity: self.identity.as_deref(),
            events: &events,
        };
        let registered: Vec<String> = self.actions.names();
        let mut options = Vec::with_capacity(args.options.len());
        for option in &args.options {
            let Some(kind) = self.actions.get(&option.action) else {
                return Err(ToolFailure::plain(format!(
                    "Unknown action \"{}\". Registered actions: {}.",
                    option.action,
                    if registered.is_empty() {
                        "none".to_owned()
                    } else {
                        registered.join(", ")
                    }
                )));
            };
            if !loaded.contains(kind.skill()) {
                return Err(ToolFailure::plain(format!(
                    "Load skill {} first, then call propose_action.",
                    kind.skill()
                )));
            }
            let details = kind
                .details(&option.details)
                .map_err(|reason| ToolFailure::plain(format!("Invalid details: {reason}")))?;
            let facts = kind
                .describe(&details, &env)
                .map_err(|reason| ToolFailure::plain(format!("Cannot offer this option: {reason}")))?;
            options.push(CardMetaOption {
                id: option.id.clone(),
                label: option.label.trim().to_owned(),
                description: option
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned),
                facts,
            });
        }
        Ok(ToolResult::success_with_meta(
            call.id.clone(),
            ACCEPTED_RESULT,
            cards::card_meta(title, brief, options),
        ))
    }
}

impl Tool for ProposeActionTool {
    fn name(&self) -> &str {
        "propose_action"
    }

    fn description(&self) -> &str {
        "Show the user one optional action card under this answer. Call only as the skill that names the actions instructs, once per turn."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkill(self.actions.skills())
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            if !context.allows("propose_action") {
                return ToolResult::error(call.id, ToolFailure::plain("unknown tool"));
            }
            match self.propose(&call, context) {
                Ok(result) => result,
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// 当前是哪一轮：日志里最后一条 turn/start。
fn current_turn(events: &[session::Event]) -> u64 {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::TurnStart { turn } => Some(*turn),
            _ => None,
        })
        .next_back()
        .unwrap_or(0)
}

/// 这一轮里成功加载过的 skill。
fn skills_loaded_in_turn(events: &[session::Event], turn: u64) -> BTreeSet<String> {
    let mut requested = std::collections::BTreeMap::new();
    for event in events {
        if let session::EventKind::ToolCall {
            turn: call_turn,
            call_id,
            name,
            arguments,
            ..
        } = &event.kind
            && *call_turn == turn
            && name == "skill"
            && let Ok(value) = serde_json::from_str::<Value>(arguments)
            && let Some(skill) = value.get("name").and_then(Value::as_str)
        {
            requested.insert(call_id.as_str(), skill.to_owned());
        }
    }
    let mut loaded = BTreeSet::new();
    for event in events {
        let session::EventKind::ToolResult {
            turn: result_turn,
            message,
            error,
            ..
        } = &event.kind
        else {
            continue;
        };
        if *result_turn != turn || message.is_error == Some(true) || error.is_some() {
            continue;
        }
        if let Some(call_id) = message.tool_call_id.as_deref()
            && let Some(skill) = requested.get(call_id)
        {
            loaded.insert(skill.clone());
        }
    }
    loaded
}

/// 这一轮是不是已经有一张被接受的卡（同一步里先发出的调用还没结果，也算）。
fn earlier_card_in_turn(events: &[session::Event], turn: u64, current: &str) -> bool {
    for event in events {
        let session::EventKind::ToolCall {
            turn: call_turn,
            call_id,
            name,
            ..
        } = &event.kind
        else {
            continue;
        };
        if *call_turn != turn || name != "propose_action" || call_id == current {
            continue;
        }
        let failed = events.iter().any(|event| {
            let session::EventKind::ToolResult {
                message, error, ..
            } = &event.kind
            else {
                return false;
            };
            message.tool_call_id.as_deref() == Some(call_id)
                && (message.is_error == Some(true) || error.is_some())
        });
        if !failed {
            return true;
        }
    }
    false
}
