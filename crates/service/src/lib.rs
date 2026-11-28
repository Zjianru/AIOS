//! 与载体无关的服务宿主：把调用翻译成 Agent 操作，把会话日志事件投影成服务事件。
//!
//! 协议形状、错误码、事件映射、根会话生命周期、设置面和输入解析各在自己的模块里；
//! 本文件只声明模块并 re-export 对外面，载体（web/cli）只从 crate 根取名字。

pub mod actions;
/// 旧 e2e 回放兼容总开关（只由假模型 e2e 进程设置；产品/真 key 不设）。
/// 编译期隔离：release 里连读取环境变量的代码都不编进来。
#[cfg(debug_assertions)]
pub(crate) fn e2e_legacy_mode() -> bool {
    std::env::var_os("AIOS_E2E_LEGACY").is_some_and(|value| value == "1")
}

#[cfg(not(debug_assertions))]
pub(crate) fn e2e_legacy_mode() -> bool {
    false
}

/// 旧 e2e 回放脚本兼容：只在显式测试环境变量下把相对写入范围当根会话目录。
#[cfg(debug_assertions)]
pub(crate) fn e2e_legacy_write_scope() -> bool {
    e2e_legacy_mode()
        || std::env::var_os("AIOS_E2E_LEGACY_WRITE_SCOPE").is_some_and(|value| value == "1")
}

#[cfg(not(debug_assertions))]
pub(crate) fn e2e_legacy_write_scope() -> bool {
    false
}

#[cfg(all(test, not(debug_assertions)))]
mod release_legacy_env_tests {
    /// release 构建即使被设了兼容环境变量，也不能放出旧相对写范围/旧相对读。
    #[test]
    fn legacy_env_variables_are_ignored_in_release_builds() {
        let exe = std::env::current_exe().expect("测试二进制");
        let output = std::process::Command::new(exe)
            .arg("--exact")
            .arg("release_legacy_env_tests::legacy_env_probe")
            .arg("--test-threads=1")
            .env("AIOS_RELEASE_LEGACY_PROBE", "1")
            .env("AIOS_E2E_LEGACY", "1")
            .env("AIOS_E2E_LEGACY_WRITE_SCOPE", "1")
            .output()
            .expect("跑 release 子进程");
        assert!(
            output.status.success(),
            "release 子进程失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "release 子进程没跑到探针：{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    /// 只在子进程里检查：父进程把兼容变量放进了环境。
    #[test]
    fn legacy_env_probe() {
        if std::env::var_os("AIOS_RELEASE_LEGACY_PROBE").is_none() {
            return;
        }
        assert!(!super::e2e_legacy_mode());
        assert!(!super::e2e_legacy_write_scope());
    }
}

pub mod aios_query;
pub(crate) mod appointments;
pub(crate) mod calendar_trigger;
mod cards;
mod date_facts;
mod delivery;
pub mod disabled;
mod error;
mod events;
mod exa;
mod executor;
mod file_bytes;
mod input;
pub mod installer;
pub(crate) mod kind_output;
mod ledger;
mod plugins;
mod projects;
mod propose;
mod protocol;
mod result_cards;
mod rewind;
mod root_lifecycle;
mod routing;
mod session_host;
mod session_select;
mod settings_surface;
mod status;
mod vision_runtime;
mod vision_tools;
mod web;
mod web_search;
pub mod work;
mod worktree;
mod write_scope;

pub mod commands;
pub mod memory;
pub mod models;
pub(crate) mod monitor_facts;
pub(crate) mod program_messages;
pub mod quota;
mod root;
pub mod settings;
pub mod stats;

/// 服务事件流容量（订阅者落后时用 Lagged）。
pub(crate) const SERVICE_EVENT_CHANNEL_CAPACITY: usize = 1024;

pub use actions::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, ActionRegistry, CardFact};
pub use delivery::{DeliveryRecord, delivery_for};
pub use error::*;
pub use exa::EXA_DEFAULT_BASE_URL;
pub use executor::{
    AppointmentCancelTool, MonitorLive, MonitorLiveTask, TASKS_DB_FILE, THIN_TOOLS,
    THIN_WRITE_BASH_TOOLS, THIN_WRITE_TOOLS, TaskLive, TaskRuntime, TaskStopTool, TaskStream,
};
pub use protocol::*;
pub use root::{
    KeyringChanged, ModelFactory, ROOT_STATE_FILE, RootError, RootSession, RootSessionOptions,
    RootStatus, open_root_session, session_model_selection,
};
pub use root_lifecycle::RootHost;
pub use session_host::ServiceHost;
#[doc(hidden)]
pub use vision_tools::set_test_screen_permission;

#[cfg(test)]
#[path = "disabled_tests.rs"]
mod disabled_tests;
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
#[path = "result_cards_tests.rs"]
mod result_cards_tests;
#[cfg(test)]
#[path = "root_lifecycle_tests.rs"]
mod root_lifecycle_tests;
#[cfg(test)]
#[path = "settings_path_tests.rs"]
mod settings_path_tests;
#[cfg(test)]
#[path = "web_fetch_tests.rs"]
mod web_fetch_tests;
#[cfg(test)]
#[path = "web_tests.rs"]
mod web_tests;

#[doc(hidden)]
pub use vision_runtime::{TestVisionOutcome, set_test_vision_outcomes};
