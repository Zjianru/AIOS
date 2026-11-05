//! Keyring：加密保存模型凭证，并按请求签发（T008.1）。
//!
//! 对外只暴露「存/删/描述/申请签发/换钥匙位置/指回/重新开始/签发记录/变更通知」；
//! 描述和记录里没有 key 正文。存储是 age 密文，钥匙文件是 age 身份文件；
//! 每次操作重新从文件读取，不缓存。

mod keyring;
pub mod presets;

pub use keyring::{
    CheckResult, CheckState, ConnectionInfo, IssueRecord, IssuedSecret, KeyFileInfo, Keyring,
    KeyringError, KeyringView, NewConnection,
};
pub use presets::{PRESETS, Preset, preset, preset_for_endpoint};
