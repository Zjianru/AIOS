//! 插件开关确认卡（D118）——卡与设置›插件页走同一 PluginSet 路径。

use std::path::PathBuf;

use serde_json::json;

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::protocol::PluginSetRequest;
use crate::root_lifecycle::RootHost;

const OFFICE_ID: &str = "office";
const IMPACT_OPEN: &str =
    "打开后 Kind/任务能读 xlsx/docx/pptx，写任务能新建 xlsx；不会跑进程，也不带运行时。";
const IMPACT_CLOSE: &str =
    "关掉后 Office 工具一律回「已关」，不能再读/生成；已生成的文件和数据保留。";

pub(crate) struct SetPluginAction {
    data_dir: PathBuf,
}

impl SetPluginAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

fn current_enabled(data_dir: &std::path::Path) -> bool {
    crate::settings::load(data_dir).plugins.office
}

impl ActionKind for SetPluginAction {
    fn name(&self) -> &str {
        "set_plugin"
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &serde_json::Value) -> Result<serde_json::Value, String> {
        let id = raw
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "缺少插件 id（只有 office）".to_owned())?;
        if id != OFFICE_ID {
            return Err(format!("未知插件：{id}（只有 office）"));
        }
        let enabled = raw
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "缺少 enabled（true/false）".to_owned())?;
        Ok(json!({ "id": OFFICE_ID, "enabled": enabled }))
    }

    fn describe(
        &self,
        details: &serde_json::Value,
        _env: &ActionEnv<'_>,
    ) -> Result<Vec<CardFact>, String> {
        let enabled = details
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "缺少 enabled".to_owned())?;
        let current = current_enabled(&self.data_dir);
        if current == enabled {
            return Err(if enabled {
                "Office 插件已经开着".to_owned()
            } else {
                "Office 插件已经关着".to_owned()
            });
        }
        Ok(vec![
            CardFact {
                name: "插件".to_owned(),
                value: "Office".to_owned(),
            },
            CardFact {
                name: "操作".to_owned(),
                value: if enabled { "打开" } else { "关掉" }.to_owned(),
            },
            CardFact {
                name: "影响".to_owned(),
                value: if enabled { IMPACT_OPEN } else { IMPACT_CLOSE }.to_owned(),
            },
            CardFact {
                name: "当前".to_owned(),
                value: if current { "已开启" } else { "已关闭" }.to_owned(),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a serde_json::Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let enabled = details
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| "缺少 enabled".to_owned())?;
            // 和设置页开关同一条 PluginSet：同一校验、同一持久化、同一广播。
            let response = host
                .plugin_set(PluginSetRequest {
                    id: OFFICE_ID.to_owned(),
                    enabled,
                })
                .await
                .map_err(|error| error.to_string())?;
            let summary = if enabled {
                "Office 插件已打开：下一轮起 Kind/任务能看到 Office 工具。"
            } else {
                "Office 插件已关掉：Office 工具一律回「已关」；已生成的文件和数据保留。"
            };
            Ok(ActionOutcome {
                summary: summary.to_owned(),
                data: serde_json::to_value(response).unwrap_or_else(|_| json!({})),
            })
        })
    }
}
