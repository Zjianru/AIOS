//! 不是密钥的设置（`<数据目录>/settings.json`）。Keyring 只放连接和 key。
//!
//! 现有：上一次的模型选择（新会话默认用；story 2「新会话默认用我上一次选的」）。
//! 起同一个文件还放币种（只增字段）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

/// 设置文件名（数据目录下）。
pub const SETTINGS_FILE: &str = "settings.json";

/// 不是密钥的设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 上一次的模型选择。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "lastSelection"
    )]
    pub last_selection: Option<session::ModelSelection>,
    /// 币种：`usd` 或 `cny`；默认 usd。
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

/// 这条对话现在用的连接/模型/档：会话日志里最后一条 session/model，
/// 没有就用 settings.json 的上次选择。查询工具和设置动作共用这一份口径。
pub(crate) fn current_selection(
    data_dir: &Path,
    events: &[session::Event],
) -> Option<session::ModelSelection> {
    session::last_model_selection(events).or_else(|| load(data_dir).last_selection)
}

/// 带检查的读法结果（屏蔽表和写入口用）。
pub(crate) enum Read {
    /// 读到：文件存在且能解析，或者文件不存在/空文件（按空设置算）。
    Known(Settings),
    /// 文件存在但读不出来；last 是这个进程上次读到的屏蔽表。
    Unreadable {
        last: Option<Vec<String>>,
        /// 原因（含 settings.json 路径）：只进失败块/诊断，不上页面。
        reason: String,
    },
}

/// 宽松读设置：文件不存在/读不出/坏 JSON 都按默认值（设置丢了不阻塞启动；与根会话状态
/// 文件的严格报错不同——这里不是恢复链的硬事实）。
///
/// 只给「上次选择/币种」这类不守门的字段用；屏蔽表和写入口用 read。
pub fn load(data_dir: &Path) -> Settings {
    read_file(data_dir).unwrap_or_default()
}

/// 带检查地读设置（照 Magpie LoadChecked）：文件不存在/空文件 = 空设置；
/// 存在但读不出来 = 错误，附带这个进程上次读到的屏蔽表。
///
/// 读成功（含文件不存在）时记下 disabledModels，坏文件时屏蔽表沿用它；
/// 从没读到过就是 last = None，调用方不能把它当成「什么都没屏蔽」。
pub(crate) fn read(data_dir: &Path) -> Read {
    match read_file(data_dir) {
        Ok(settings) => {
            remember_last(data_dir, &settings.disabled_models);
            Read::Known(settings)
        }
        Err(reason) => Read::Unreadable {
            last: last_disabled(data_dir),
            reason,
        },
    }
}

/// 写入口的读法：文件存在但读不出来就报原因（SETTINGS_UNREADABLE），不写。
pub(crate) fn load_for_write(data_dir: &Path) -> Result<Settings, String> {
    match read(data_dir) {
        Read::Known(settings) => Ok(settings),
        Read::Unreadable { reason, .. } => Err(reason),
    }
}

/// 上次读到的屏蔽表（进程级，按 settings.json 路径分开）。
///
/// 只在读到文件（含不存在/空文件）或写成功时更新；坏文件沿用它。按路径分开，
/// 换数据目录或一个进程里开多个宿主时不会串用别人的表。
static LAST_DISABLED: LazyLock<Mutex<HashMap<PathBuf, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn remember_last(data_dir: &Path, models: &[String]) {
    LAST_DISABLED
        .lock()
        .expect("上次读到的屏蔽表锁被毒化")
        .insert(path(data_dir), models.to_vec());
}

fn last_disabled(data_dir: &Path) -> Option<Vec<String>> {
    LAST_DISABLED
        .lock()
        .expect("上次读到的屏蔽表锁被毒化")
        .get(&path(data_dir))
        .cloned()
}

/// 读文件并解析：不存在/空文件是空设置；读错误/坏 JSON 是 Err(含路径的原因)。
fn read_file(data_dir: &Path) -> Result<Settings, String> {
    let target = path(data_dir);
    match std::fs::read(&target) {
        Ok(bytes) => {
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return Ok(Settings::default());
            }
            serde_json::from_slice(&bytes).map_err(|error| format!("{}：{error}", target.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(format!("{}：{error}", target.display())),
    }
}

/// 原子写设置：临时文件 + fsync + rename + 目录 fsync，失败向上返回（credentials::atomic）。
pub fn save(data_dir: &Path, settings: &Settings) -> Result<(), String> {
    let mut json = serde_json::to_vec(settings).map_err(|error| error.to_string())?;
    json.push(b'\n');
    credentials::atomic::write(
        &path(data_dir),
        &json,
        credentials::atomic::TargetKind::Shared,
    )
    .map_err(|error| error.to_string())?;
    // 写成功也是「读到了」：文件里就是这份内容，坏了时沿用它。
    remember_last(data_dir, &settings.disabled_models);
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
