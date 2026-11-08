//! monitor 集成测试共用：把监控挂在会话写入钩子上。

use monitor::Monitor;
use session::SessionStore;

pub fn wire_ledger(monitor: &Monitor, store: &mut SessionStore) {
    let factory = monitor.append_hook_factory();
    store.set_append_hook(Some(factory(store.id())));
}
