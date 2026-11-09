//! `aios_query`：Kind 查 AIOS 现在的设置、连接、模型、用量和产品信息。
//!
//! 只读服务里**已经存下的**数据（D71）：设置读 `settings.json`、连接读 Keyring 的视图、
//! 模型读目录和存下的厂商列表、用量读会话日志、产品信息是编进二进制的常数。不现拉厂商
//! 列表、不现测连接（那两件是设置页的按钮）；输出里没有 key 正文，也不带连接的地址和
//! 鉴权头——连接只给名字、提供方、有没有 key、上次验证结果。
//!
//! 挂在 skill `aios-settings` 后面：本会话加载过它以后，下一步才开出这个工具（T014.2）。

use std::path::PathBuf;

use serde_json::{Value, json};
use tools::{Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 查询的五个主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Topic {
    Settings,
    Connections,
    Models,
    Usage,
    Product,
}

impl Topic {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "settings" => Some(Self::Settings),
            "connections" => Some(Self::Connections),
            "models" => Some(Self::Models),
            "usage" => Some(Self::Usage),
            "product" => Some(Self::Product),
            _ => None,
        }
    }
}

/// 只读查询工具。
pub struct QueryTool {
    data_dir: PathBuf,
    /// Keyring 的钥匙文件位置；None = 用数据目录里的配置/默认位置。
    identity: Option<PathBuf>,
    parameters: Value,
}

impl QueryTool {
    pub fn new(data_dir: impl Into<PathBuf>, identity: Option<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            identity,
            parameters: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "enum": ["settings", "connections", "models", "usage", "product"],
                        "description": "settings: this conversation's model and effort, the selection new conversations use, and the currency. connections: connections already stored, with whether a key is saved and the last verification result. models: the models each connection can pick, with masking rules. usage: this conversation's turns, steps, tokens and cost. product: what AIOS can and cannot do."
                    }
                },
                "required": ["topic"]
            }),
        }
    }

    fn read(&self, topic: Topic, context: &ToolContext) -> Result<Value, ToolFailure> {
        match topic {
            Topic::Settings => self.settings(context),
            Topic::Connections => self.connections(),
            Topic::Models => self.models(),
            Topic::Usage => self.usage(context),
            Topic::Product => Ok(product()),
        }
    }

    fn keyring(&self) -> Result<credentials::Keyring, ToolFailure> {
        credentials::Keyring::open(&self.data_dir, self.identity.as_deref())
            .map_err(|error| ToolFailure::plain(format!("读不出 Keyring 的视图：{error}")))
    }

    fn settings(&self, context: &ToolContext) -> Result<Value, ToolFailure> {
        let settings = crate::settings::load(&self.data_dir);
        let current = crate::settings::current_selection(&self.data_dir, &context.session_events())
            .ok_or_else(|| ToolFailure::plain("这条对话还没有选过模型，也没有上次的选择可读"))?;
        let next = settings.last_selection.map_or(Value::Null, |selection| {
            json!({
                "connection": selection.connection,
                "model": selection.model,
                "effort": selection.effort_wanted,
            })
        });
        Ok(json!({
            "session": {
                "connection": current.connection,
                "model": current.model,
                "effort": current.effort_wanted,
            },
            "newConversations": next,
            "currency": settings.currency,
        }))
    }

    fn connections(&self) -> Result<Value, ToolFailure> {
        let view = self.keyring()?.view();
        let rows: Vec<Value> = view
            .connections
            .iter()
            .map(|connection| {
                json!({
                    "id": connection.id,
                    "name": connection.name,
                    "provider": connection.preset,
                    "hasKey": connection.key_saved,
                    "lastCheck": serde_json::to_value(&connection.check).unwrap_or(Value::Null),
                })
            })
            .collect();
        Ok(json!({ "connections": rows }))
    }

    fn models(&self) -> Result<Value, ToolFailure> {
        let view = self.keyring()?.view();
        let disabled = crate::disabled::table(&self.data_dir);
        let rows: Vec<Value> = view
            .connections
            .iter()
            .map(|connection| {
                let primary = crate::models::primary_model(&self.data_dir, connection);
                let models: Vec<Value> =
                    crate::models::selectable_models(&self.data_dir, connection)
                        .into_iter()
                        .map(|info| {
                            let masked = crate::disabled::match_disabled(
                                &disabled.models,
                                &connection.id,
                                &info.id,
                            );
                            json!({
                                "id": info.id,
                                "name": info.name,
                                "primary": info.id == primary,
                                "disabled": masked.is_some(),
                            })
                        })
                        .collect();
                json!({
                    "id": connection.id,
                    "name": connection.name,
                    "models": models,
                })
            })
            .collect();
        Ok(json!({
            "connections": rows,
            "disabled": disabled.models,
            "disabledUnknown": disabled.unknown,
        }))
    }

    fn usage(&self, context: &ToolContext) -> Result<Value, ToolFailure> {
        let events = context.session_events();
        if events.is_empty() {
            return Err(ToolFailure::plain("拿不到这条对话的会话日志"));
        }
        let view = self.keyring()?.view();
        let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
        let current = crate::settings::current_selection(&self.data_dir, &events)
            .ok_or_else(|| ToolFailure::plain("这条对话还没有选过模型，也没有上次的选择可读"))?;
        let stats =
            crate::stats::session_stats(&events, &view.connections, &catalog, &current, 0, 0);
        Ok(json!({
            "turns": stats.turns,
            "steps": stats.steps,
            "inputTokens": stats.input_tokens,
            "outputTokens": stats.output_tokens,
            "cacheReadTokens": stats.cache_read_tokens,
            "cacheWriteTokens": stats.cache_write_tokens,
            "costUsd": stats.cost_usd,
            "unpriced": stats.unpriced,
            "connection": stats.connection,
            "model": stats.model,
        }))
    }
}

/// 产品信息和能力边界（A04）：编进二进制的常数，不读磁盘。
fn product() -> Value {
    json!({
        "product": "AIOS",
        "about": "面向个人的桌面智能体工作台：把模型、工具和个人上下文组织成可执行的任务流。",
        "can": [
            "用你选的模型连接和你填的 key 对话；要换模型、换强度，我出确认卡，你点了才生效。",
            "读这条对话现在用的模型、推理强度、币种，以及一共花了多少。",
            "读你已经存下的连接、每个连接能选的模型、屏蔽了哪些模型。",
            "用文件读写、搜索、命令、后台任务、画布这些工具干活。",
            "在设置页填、换、删 Exa 的 key 以后搜网页、读网页（本机地址和内网地址会被拒）。"
        ],
        "cannot": [
            "自己建连接、填 key 或换 key：这些只在设置页做。",
            "现拉厂商的模型列表、现测连接：那是设置页上的按钮。",
            "从环境变量里找 key。",
            "碰任务、项目、日历这些还没做的域。",
            "绕过确认卡直接改设置：要改就得你在卡上点。"
        ]
    })
}

impl Tool for QueryTool {
    fn name(&self) -> &str {
        "aios_query"
    }

    fn description(&self) -> &str {
        "Read AIOS's own current settings, connections, models, this conversation's usage, or what AIOS can and cannot do. Read-only: it never changes anything and never returns API keys."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkill(vec!["aios-settings".to_owned()])
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let topic = match parse_topic(&call.arguments) {
                Ok(topic) => topic,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            match self.read(topic, context) {
                Ok(value) => ToolResult::success(
                    call.id,
                    serde_json::to_string_pretty(&value)
                        .unwrap_or_else(|error| format!("查询结果没法序列化成 JSON：{error}")),
                ),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn parse_topic(arguments: &str) -> Result<Topic, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let name = args
        .get("topic")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("invalid topic: expected a non-empty string"))?;
    Topic::parse(name).ok_or_else(|| {
        ToolFailure::plain(format!(
            "invalid topic {name:?}: expected settings, connections, models, usage or product"
        ))
    })
}
