//! T011.5：agent 的警告出口。标准错误照旧输出；服务启动时装一个钩子，把同一条警告记成监控 ops。
//!
//! agent 不认识监控的类型：钩子是普通的 Fn(&str)，由服务/CLI 在装配时注入。

use std::sync::{Arc, OnceLock};

pub type WarningHook = Arc<dyn Fn(&str) + Send + Sync>;

static HOOK: OnceLock<WarningHook> = OnceLock::new();

/// 装警告钩子（只在进程启动时装一次；重复装直接忽略）。
pub fn set_warning_hook(hook: WarningHook) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "全局警告钩子只装一次；重复装配保留首个"
    )]
    let _ = HOOK.set(hook);
}

/// 一条警告：先照旧打到标准错误，再交给钩子（如果有）。
pub(crate) fn warn(message: impl Into<String>) {
    let message = message.into();
    eprintln!("{message}");
    if let Some(hook) = HOOK.get() {
        hook(&message);
    }
}

/// 会话 store 的警告出口：钩子降级这类会话侧信号接到 agent 的警告通道
/// （标准错误 + 服务装的监控钩子），和 agent 自己的警告走同一条路。
pub(crate) fn session_store_sink() -> session::WarningSink {
    Arc::new(|message: &str| warn(message.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn hook_receives_the_same_warning_as_stderr() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        set_warning_hook(Arc::new(move |message: &str| {
            sink.lock().expect("锁").push(message.to_owned());
        }));
        warn("[skill] 测试警告");
        assert_eq!(
            captured.lock().expect("锁").as_slice(),
            ["[skill] 测试警告"],
            "同一个消息交给钩子（标准错误由 eprintln 照旧输出）"
        );
    }
}
