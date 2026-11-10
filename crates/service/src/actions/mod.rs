//! 服务端动作登记：确认卡背后能做的事由这里登记，卡片和提议工具只认登记表。
//!
//! 每个动作说清四件事：名字、属于哪个 skill、details 的校验与当前事实（describe）、
//! 点确认之前核对事实还成不成立（reconcile）、以及真正执行（act）。具体域的动作只加
//! 登记，不碰卡片和提议工具的代码。

mod mask;
mod select_model;
mod set_paused;

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::root_lifecycle::RootHost;

/// 事实行：一行「名称：值」，由服务端按当前状态填，模型给不了。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardFact {
    pub name: String,
    pub value: String,
}

/// 动作读当前状态时能看到的只读面：设置、连接和这条对话的日志。
pub struct ActionEnv<'a> {
    pub data_dir: &'a Path,
    pub identity: Option<&'a Path>,
    pub events: &'a [session::Event],
}

/// 执行结果：给用户看的一句结果，加上落进 card/decision 的数据。
#[derive(Debug, Clone, PartialEq)]
pub struct ActionOutcome {
    pub summary: String,
    pub data: serde_json::Value,
}

/// 动作执行的异步返回（dyn 兼容；不用 async-trait）。
pub type ActionFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 一个登记的动作。
pub trait ActionKind: Send + Sync {
    /// details 里的 action 值；也是登记表的键。
    fn name(&self) -> &str;

    /// 提议这个动作的 skill：本轮加载过它，动作才能用。
    fn skill(&self) -> &str;

    /// 校验并规范化 details；不合法时返回给模型看的原因。
    fn details(&self, raw: &serde_json::Value) -> Result<serde_json::Value, String>;

    /// 读当前状态，给出这一项的事实行。做不到时（模型不存在、已经是要改成的样子）
    /// 返回原因，这次提议就被拒绝。
    fn describe(
        &self,
        details: &serde_json::Value,
        env: &ActionEnv<'_>,
    ) -> Result<Vec<CardFact>, String>;

    /// 点确认之前核事实还成不成立。默认重算 describe，和提议时的事实逐行比。
    fn reconcile(
        &self,
        details: &serde_json::Value,
        facts: &[CardFact],
        env: &ActionEnv<'_>,
    ) -> Result<(), String> {
        let current = self.describe(details, env)?;
        if current == facts {
            Ok(())
        } else {
            Err("卡片上的之前和之后已经变了".to_owned())
        }
    }

    /// 走正式的内部操作；失败返回原因（记 failed 回执，不再执行）。
    fn act<'a>(
        &'a self,
        details: &'a serde_json::Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>>;
}

/// 动作登记表：按名字查；skill 名单给提议工具的开出条件用。
#[derive(Default, Clone)]
pub struct ActionRegistry {
    kinds: BTreeMap<String, Arc<dyn ActionKind>>,
}

impl ActionRegistry {
    pub fn empty() -> Self {
        Self::default()
    }

    /// 产品设置域的动作。数据目录和钥匙位置在这里带上，动作每次自己读当前状态。
    pub fn settings(data_dir: PathBuf, identity: Option<PathBuf>) -> Self {
        let mut registry = Self::empty();
        registry.register(Arc::new(select_model::SelectModelAction::new(
            data_dir.clone(),
            identity.clone(),
        )));
        registry.register(Arc::new(mask::MaskAction::mask(
            data_dir.clone(),
            identity.clone(),
        )));
        registry.register(Arc::new(mask::MaskAction::unmask(
            data_dir.clone(),
            identity,
        )));
        registry.register(Arc::new(set_paused::SetPausedAction::new(data_dir)));
        registry
    }

    /// 登记一个动作；同一个名字登记两次是装配错误。
    pub fn register(&mut self, kind: Arc<dyn ActionKind>) {
        let name = kind.name().to_owned();
        assert!(!self.kinds.contains_key(&name), "动作被重复登记：{name}");
        self.kinds.insert(name, kind);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ActionKind>> {
        self.kinds.get(name)
    }

    /// 登记表里的动作名，升序（拒绝原因里列给模型看）。
    pub fn names(&self) -> Vec<String> {
        self.kinds.keys().cloned().collect()
    }

    /// 登记过的动作属于哪些 skill，升序去重。
    pub fn skills(&self) -> Vec<String> {
        let mut skills: Vec<String> = self
            .kinds
            .values()
            .map(|kind| kind.skill().to_owned())
            .collect();
        skills.sort();
        skills.dedup();
        skills
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}
