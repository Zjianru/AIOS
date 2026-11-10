//! 全局暂停（属于 aios-tasks）：打开时新的 Attempt 排队，在跑的一轮跑完；
//! relay/notice 照送。关掉后新任务按排队顺序接着走（不取消、不清队）。
//!
//! 事实是设置文件里的之前、之后；已经是这个状态时不提议。点确认前重算一遍事实：
//! 中间在别处改过设置，就对不上，记 refused 不执行。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

/// 设置域里的全局暂停动作。
pub(crate) struct SetPausedAction {
    data_dir: PathBuf,
}

impl SetPausedAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// details 只有一个布尔字段；缺了或不是布尔都拒绝。
    fn paused(&self, raw: &Value) -> Result<bool, String> {
        raw.as_object()
            .and_then(|object| object.get("paused"))
            .and_then(Value::as_bool)
            .ok_or_else(|| "details.paused 必须是 true 或 false".to_owned())
    }

    /// 设置文件里现在的值；文件读不出来就拒绝（不猜）。
    fn current(&self) -> Result<bool, String> {
        crate::settings::load_for_write(&self.data_dir)
            .map(|settings| settings.exec.paused)
            .map_err(|reason| format!("设置文件读不出来：{reason}"))
    }
}

impl ActionKind for SetPausedAction {
    fn name(&self) -> &str {
        "set_paused"
    }

    fn skill(&self) -> &str {
        "aios-tasks"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(json!({ "paused": self.paused(raw)? }))
    }

    fn describe(&self, details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        self.paused(details)?;
        self.current()?;
        // 骨架：事实行还没接上（红阶段）。
        Ok(Vec::new())
    }

    fn act<'a>(
        &'a self,
        _details: &'a Value,
        _host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async { Err("全局暂停还没接上".to_owned()) })
    }
}
