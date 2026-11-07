//! T012.7：不是密钥的设置（`<数据目录>/settings.json`）。Keyring 只放连接和 key。
//!
//! 现有：上一次的模型选择（新会话默认用；story 2「新会话默认用我上一次选的」）。
//! T012.8 起同一个文件还放币种（只增字段）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 设置文件名（数据目录下）。
pub const SETTINGS_FILE: &str = "settings.json";

/// 不是密钥的设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 上一次的模型选择（T012.7）。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "lastSelection"
    )]
    pub last_selection: Option<session::ModelSelection>,
    /// 币种（T012.8）：`usd` 或 `cny`；默认 usd。
    #[serde(default = "default_currency")]
    pub currency: String,
    /// 屏蔽的模型规则（通配；空表 = 都不屏蔽）。改了立即生效。
    #[serde(default, rename = "disabledModels")]
    pub disabled_models: Vec<String>,
}

fn default_currency() -> String {
    "usd".to_owned()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_selection: None,
            currency: default_currency(),
            disabled_models: Vec::new(),
        }
    }
}

/// 设置文件路径。
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(SETTINGS_FILE)
}

/// 读设置：文件不存在/读不出/坏 JSON 都按默认值（设置丢了不阻塞启动；与根会话状态文件的
/// 严格报错不同——这里不是恢复链的硬事实）。
pub fn load(data_dir: &Path) -> Settings {
    std::fs::read(path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 原子写设置：同目录临时文件 → fsync → rename 覆盖（照根会话状态文件）。
pub fn save(data_dir: &Path, settings: &Settings) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec(settings).map_err(|error| error.to_string())?;
    let target = path(data_dir);
    let tmp = target.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp).map_err(|error| error.to_string())?;
        file.write_all(&json).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    std::fs::rename(&tmp, &target).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> session::ModelSelection {
        session::ModelSelection {
            connection: "minimax-cn".to_owned(),
            model: "MiniMax-M3.1-Flash".to_owned(),
            effort_wanted: "low".to_owned(),
        }
    }

    #[test]
    fn missing_settings_read_as_default() {
        let dir = tempfile::tempdir().expect("dir");
        assert_eq!(load(dir.path()), Settings::default());
    }

    #[test]
    fn last_selection_round_trips_atomically() {
        let dir = tempfile::tempdir().expect("dir");
        let settings = Settings {
            last_selection: Some(selection()),
            ..Settings::default()
        };
        save(dir.path(), &settings).expect("save");
        assert_eq!(load(dir.path()), settings);
        assert!(!path(dir.path()).with_extension("json.tmp").exists());
        // 坏文件按默认值读（不阻塞启动）。
        std::fs::write(path(dir.path()), b"{not json").expect("write");
        assert_eq!(load(dir.path()), Settings::default());
    }
}
