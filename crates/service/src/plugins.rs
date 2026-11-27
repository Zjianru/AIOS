//! T094：插件开关的协议面（settings › 插件页用）。
//!
//! 官方插件只有 Office；开关持久化在 settings.plugins，运行时写进共享 PluginSwitches，
//! 下一 step 工具的 ToolRegistry 追加/SkillCatalog 更新自然发生（打开不发 SystemMessage）。

use crate::error::ServiceError;
use crate::protocol::{PluginListResponse, PluginSetRequest, PluginView};
use crate::root_lifecycle::RootHost;
use crate::settings;

/// 关掉时的固定 SystemMessage（打开时不发）。
pub(crate) const PLUGIN_CLOSED_NOTICE: &str =
    "【插件】Office 已关闭：Office 工具一律回「已关」；已生成的文件和数据保留。";

const OFFICE_ID: &str = "office";
const OFFICE_NAME: &str = "Office";
const OFFICE_DESCRIPTION: &str =
    "读写 xlsx/docx/pptx；第一刀可新建 xlsx，读只给结构与文字，不承诺版式。";
const OFFICE_SKILL_SUMMARY: &str =
    "读写 Office 文件（xlsx/docx/pptx）：xlsx 可生成；读只给结构与文字，不承诺版式。";

impl RootHost {
    /// 插件列表；enabled 从 settings 读（运行时开关同一份来源）。
    pub(crate) async fn plugin_list(&self) -> Result<PluginListResponse, ServiceError> {
        let settings = settings::load(&self.options.data_dir);
        Ok(PluginListResponse {
            plugins: vec![self.office_plugin_view(settings.plugins.office)],
            content_packs: Vec::new(),
        })
    }

    /// 打开/关掉一个插件：校验 → 写 settings → 写共享开关 → 关掉时追加固定通知 → 广播。
    pub(crate) async fn plugin_set(
        &self,
        request: PluginSetRequest,
    ) -> Result<PluginListResponse, ServiceError> {
        if request.id != OFFICE_ID {
            return Err(ServiceError::SettingsInvalid(format!(
                "未知插件：{}（只有 office）",
                request.id
            )));
        }
        let mut current = settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        let was_enabled = current.plugins.office;
        current.plugins.office = request.enabled;
        settings::save(&self.options.data_dir, &current).map_err(ServiceError::SettingsInvalid)?;
        // 共享开关：根会话与所有 ThinAgent 的下一 step 按它过滤工具表/skill 目录。
        let switches = tools::plugins::switches_for(&self.options.data_dir);
        if request.enabled {
            switches.enable(tools::PluginId::Office);
        } else {
            switches.disable(tools::PluginId::Office);
        }
        if was_enabled && !request.enabled {
            let guard = self.inner.lock().await;
            if let Some(host) = guard.as_ref() {
                host.agent()
                    .append_plugin_notice(PLUGIN_CLOSED_NOTICE)
                    .await
                    .map_err(|error| ServiceError::SettingsInvalid(error.to_string()))?;
            }
        }
        self.emit_settings_changed();
        self.plugin_list().await
    }

    fn office_plugin_view(&self, enabled: bool) -> PluginView {
        PluginView {
            id: OFFICE_ID.to_owned(),
            name: OFFICE_NAME.to_owned(),
            description: OFFICE_DESCRIPTION.to_owned(),
            enabled,
            skill_summary: OFFICE_SKILL_SUMMARY.to_owned(),
            data_dir: self
                .options
                .data_dir
                .join("plugins")
                .join("office")
                .display()
                .to_string(),
        }
    }
}
