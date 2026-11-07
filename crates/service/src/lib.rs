//! 与载体无关的服务宿主：把调用翻译成 Agent 操作，把会话日志事件投影成服务事件。
//!
//! 协议形状、错误码、事件映射、根会话生命周期、设置面和输入解析各在自己的模块里；
//! 本文件只声明模块并 re-export 对外面，载体（web/cli）只从 crate 根取名字。

mod error;
mod events;
mod input;
mod protocol;
mod root_lifecycle;
mod routing;
mod session_host;
mod session_select;
mod settings_surface;
mod status;

pub mod fx;
pub mod quota;
mod root;
pub mod settings;
pub mod stats;

/// 服务事件流容量（订阅者落后时用 Lagged）。
pub(crate) const SERVICE_EVENT_CHANNEL_CAPACITY: usize = 1024;

pub use error::*;
pub use protocol::*;
pub use root::{
    KeyringChanged, ModelFactory, ROOT_STATE_FILE, RootError, RootSession, RootSessionOptions,
    RootStatus, open_root_session, session_model_selection,
};
pub use root_lifecycle::RootHost;
pub use session_host::ServiceHost;

#[cfg(test)]
#[path = "error_code_tests.rs"]
mod error_code_tests;
#[cfg(test)]
#[path = "event_projection_tests.rs"]
mod event_projection_tests;
#[cfg(test)]
#[path = "failure_cause_code_tests.rs"]
mod failure_cause_code_tests;
#[cfg(test)]
#[path = "history_pagination_tests.rs"]
mod history_pagination_tests;
#[cfg(test)]
#[path = "protocol_version_tests.rs"]
mod protocol_version_tests;
#[cfg(test)]
#[path = "settings_path_tests.rs"]
mod settings_path_tests;
