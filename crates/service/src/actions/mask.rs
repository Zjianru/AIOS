//! 屏蔽、解除屏蔽一个模型（属于 aios-settings）：走设置里的屏蔽规则表。
//!
//! 事实是这个模型的状态之前、之后。模型不存在、已经屏蔽、解除时没有单独规则时不提议；
//! 点确认前重算一遍事实，设置页在中间改过就对不上，记 refused 不执行。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::root_lifecycle::RootHost;

/// 做哪个方向：屏蔽，还是解除。
#[derive(Clone, Copy)]
enum Direction {
    Mask,
    Unmask,
}

/// 设置域里的屏蔽动作。
pub(crate) struct MaskAction {
    data_dir: PathBuf,
    identity: Option<PathBuf>,
    direction: Direction,
}

struct Parts {
    connection: String,
    model: String,
}

impl MaskAction {
    pub(crate) fn mask(data_dir: PathBuf, identity: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            identity,
            direction: Direction::Mask,
        }
    }

    pub(crate) fn unmask(data_dir: PathBuf, identity: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            identity,
            direction: Direction::Unmask,
        }
    }

    fn parts(&self, raw: &Value) -> Result<Parts, String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
        let text = |key: &str| -> Result<String, String> {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("details.{key} 必须是非空字符串"))
        };
        Ok(Parts {
            connection: text("connection")?,
            model: text("model")?,
        })
    }

    fn keyring(&self) -> Result<credentials::Keyring, String> {
        credentials::Keyring::open(&self.data_dir, self.identity.as_deref())
            .map_err(|error| format!("读不出 Keyring：{error}"))
    }
}

impl ActionKind for MaskAction {
    fn name(&self) -> &str {
        match self.direction {
            Direction::Mask => "mask_model",
            Direction::Unmask => "unmask_model",
        }
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        let parts = self.parts(raw)?;
        Ok(json!({
            "connection": parts.connection,
            "model": parts.model,
        }))
    }

    fn describe(&self, _details: &Value, _env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        // 骨架：先把签名和登记立起来，事实与拒绝留到实现。
        let _ = self.keyring()?;
        Ok(Vec::new())
    }

    fn act<'a>(
        &'a self,
        _details: &'a Value,
        _host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            Ok(ActionOutcome {
                summary: "还没做".to_owned(),
                data: Value::Null,
            })
        })
    }
}
