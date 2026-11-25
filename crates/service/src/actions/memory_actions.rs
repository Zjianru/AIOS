//! 记忆的低频写动作（属于 aios-memory）：删除/彻底删/恢复/更正/定下来/做梦设置/许可/两张判断卡。
//!
//! 事实行由服务端读当前记忆/卡片；执行全部走 memory::page::action 与页面同一个函数。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::protocol::{MemoryActionKind, MemoryActionRequest};
use crate::root_lifecycle::RootHost;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Delete,
    Forget,
    Restore,
    Correct,
    Pin,
    DreamSettings,
    Permit,
    ResolveReplace,
    ResolveAboutDraft,
}

pub(crate) struct MemoryAction {
    data_dir: PathBuf,
    kind: Kind,
}

impl MemoryAction {
    pub(crate) fn delete(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::Delete,
        }
    }
    pub(crate) fn forget(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::Forget,
        }
    }
    pub(crate) fn restore(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::Restore,
        }
    }
    pub(crate) fn correct(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::Correct,
        }
    }
    pub(crate) fn pin(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::Pin,
        }
    }
    pub(crate) fn dream_settings(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::DreamSettings,
        }
    }
    pub(crate) fn permit(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::Permit,
        }
    }
    pub(crate) fn resolve_replace(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::ResolveReplace,
        }
    }
    pub(crate) fn resolve_about_draft(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            kind: Kind::ResolveAboutDraft,
        }
    }

    fn store(&self) -> Result<memory::MemoryStore, String> {
        memory::MemoryStore::open(self.data_dir.join("memory.sqlite"))
            .map_err(|error| format!("记忆库读不出来：{error}"))
    }

    fn id(raw: &Value) -> Result<String, String> {
        raw.as_object()
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "details.id 必须是非空的记忆 id".to_owned())
    }

    fn opt_text(raw: &Value, key: &str) -> Result<Option<String>, String> {
        match raw.as_object().and_then(|object| object.get(key)) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.to_owned())),
            Some(_) => Err(format!("details.{key} 必须是字符串")),
        }
    }

    fn text(raw: &Value, key: &str) -> Result<String, String> {
        Self::opt_text(raw, key)?.ok_or_else(|| format!("details.{key} 必须是字符串"))
    }

    fn flag(raw: &Value, key: &str) -> Result<bool, String> {
        raw.as_object()
            .and_then(|object| object.get(key))
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("details.{key} 必须是布尔值"))
    }

    fn item(&self, id: &str) -> Result<memory::types::MemoryItem, String> {
        self.store()?
            .item(id)
            .map_err(|error| format!("记忆读不出来：{error}"))?
            .ok_or_else(|| format!("找不到这条记忆：{id}"))
    }

    fn judgment(name: String, object: String) -> Vec<CardFact> {
        vec![
            CardFact {
                name: "这是你的判断".to_owned(),
                value: object,
            },
            CardFact {
                name: "对象".to_owned(),
                value: name,
            },
        ]
    }

    fn corrected(
        &self,
        details: &Value,
    ) -> Result<(memory::types::MemoryItem, String, String, String, String), String> {
        let item = self.item(&Self::id(details)?)?;
        let title = Self::opt_text(details, "title")?.unwrap_or_else(|| item.title.clone());
        let summary = Self::opt_text(details, "summary")?.unwrap_or_else(|| item.summary.clone());
        let body = Self::opt_text(details, "body")?.unwrap_or_else(|| item.body.clone());
        let reason = Self::opt_text(details, "reason")?.unwrap_or_default();
        Ok((item, title, summary, body, reason))
    }

    fn check_user_words(
        env: &ActionEnv<'_>,
        values: &[(&str, &str)],
        current: &memory::types::MemoryItem,
    ) -> Result<(), String> {
        let conversation = env.conversation.ok_or_else(|| {
            "没有发起会话 id，核不了更正文字；对不上只能 memory_note，不能改原条。".to_owned()
        })?;
        let unchanged = [
            ("title", current.title.as_str()),
            ("summary", current.summary.as_str()),
            ("body", current.body.as_str()),
        ];
        for (name, value) in values {
            if value.trim().is_empty() {
                continue;
            }
            if let Some((_, old)) = unchanged.iter().find(|(key, _)| key == name)
                && *value == *old
            {
                continue;
            }
            if crate::ledger::find_user_words(env.data_dir, conversation, value)?.is_none() {
                return Err(format!(
                    "更正后的{name}必须逐字来自用户消息：『{value}』不在用户消息里；对不上只能 memory_note，不能改原条。"
                ));
            }
        }
        Ok(())
    }

    fn kind_action(&self, details: &Value) -> Result<MemoryActionKind, String> {
        Ok(match self.kind {
            Kind::Delete => MemoryActionKind::Delete {
                id: Self::id(details)?,
            },
            Kind::Forget => MemoryActionKind::Forget {
                id: Self::id(details)?,
            },
            Kind::Restore => MemoryActionKind::Restore {
                id: Self::id(details)?,
            },
            Kind::Correct => {
                let (_, title, summary, body, reason) = self.corrected(details)?;
                MemoryActionKind::Correct {
                    id: Self::id(details)?,
                    title,
                    summary,
                    body,
                    reason,
                }
            }
            Kind::Pin => MemoryActionKind::Pin {
                id: Self::id(details)?,
            },
            Kind::DreamSettings => {
                return Err("做梦设置一次只改一个开关".to_owned());
            }
            Kind::Permit => MemoryActionKind::SetPermit {
                allow: Self::flag(details, "allow")?,
            },
            Kind::ResolveReplace => MemoryActionKind::ResolveReplace {
                card: Self::text(details, "card")?,
                replace: Self::flag(details, "replace")?,
            },
            Kind::ResolveAboutDraft => MemoryActionKind::ResolveAboutDraft {
                card: Self::text(details, "card")?,
                save: Self::flag(details, "save")?,
            },
        })
    }
}

impl ActionKind for MemoryAction {
    fn name(&self) -> &str {
        match self.kind {
            Kind::Delete => "memory_delete",
            Kind::Forget => "memory_forget",
            Kind::Restore => "memory_restore",
            Kind::Correct => "memory_correct",
            Kind::Pin => "memory_pin",
            Kind::DreamSettings => "memory_dream_settings",
            Kind::Permit => "memory_permit",
            Kind::ResolveReplace => "memory_resolve_replace",
            Kind::ResolveAboutDraft => "memory_resolve_about_draft",
        }
    }

    fn skill(&self) -> &str {
        "aios-memory"
    }

    fn details(&self, raw: &Value) -> Result<Value, String> {
        Ok(match self.kind {
            Kind::Delete | Kind::Forget | Kind::Restore | Kind::Pin => {
                json!({ "id": Self::id(raw)? })
            }
            Kind::Correct => json!({
                "id": Self::id(raw)?,
                "title": Self::opt_text(raw, "title")?,
                "summary": Self::opt_text(raw, "summary")?,
                "body": Self::opt_text(raw, "body")?,
                "reason": Self::opt_text(raw, "reason")?,
            }),
            Kind::DreamSettings => {
                let enabled = raw
                    .as_object()
                    .and_then(|object| object.get("enabled"))
                    .and_then(Value::as_bool);
                let interval =
                    raw.as_object()
                        .and_then(|object| object.get("interval_hours"))
                        .map(|value| {
                            value.as_u64().filter(|hours| *hours > 0).ok_or_else(|| {
                                "details.interval_hours 必须是大于 0 的整数".to_owned()
                            })
                        })
                        .transpose()?;
                if enabled.is_none() && interval.is_none() {
                    return Err("details 至少要带 enabled 或 interval_hours".to_owned());
                }
                json!({ "enabled": enabled, "interval_hours": interval })
            }
            Kind::Permit => json!({ "allow": Self::flag(raw, "allow")? }),
            Kind::ResolveReplace => json!({
                "card": Self::text(raw, "card")?,
                "replace": Self::flag(raw, "replace")?,
            }),
            Kind::ResolveAboutDraft => json!({
                "card": Self::text(raw, "card")?,
                "save": Self::flag(raw, "save")?,
            }),
        })
    }

    fn describe(&self, details: &Value, env: &ActionEnv<'_>) -> Result<Vec<CardFact>, String> {
        match self.kind {
            Kind::Delete => {
                let item = self.item(&Self::id(details)?)?;
                Ok(vec![
                    CardFact {
                        name: "标题".to_owned(),
                        value: item.title,
                    },
                    CardFact {
                        name: "动作".to_owned(),
                        value: "移到隔离区（7 天内可恢复）".to_owned(),
                    },
                ])
            }
            Kind::Forget => {
                let item = self.item(&Self::id(details)?)?;
                let conversation = env
                    .conversation
                    .ok_or_else(|| "没有发起会话 id，核不了彻底删".to_owned())?;
                if crate::ledger::find_user_words(env.data_dir, conversation, "彻底删")?.is_none()
                    && crate::ledger::find_user_words(env.data_dir, conversation, "永久删")?
                        .is_none()
                {
                    return Err(
                        "只有用户明说「彻底删」或「永久删」才彻底删；否则用 memory_delete（移隔离区，7 天可恢复）。"
                            .to_owned(),
                    );
                }
                Ok(vec![
                    CardFact {
                        name: "彻底删，不能恢复".to_owned(),
                        value: item.title,
                    },
                    CardFact {
                        name: "记忆".to_owned(),
                        value: item.id,
                    },
                ])
            }
            Kind::Restore => {
                let item = self.item(&Self::id(details)?)?;
                Ok(vec![
                    CardFact {
                        name: "标题".to_owned(),
                        value: item.title,
                    },
                    CardFact {
                        name: "动作".to_owned(),
                        value: "从隔离区恢复".to_owned(),
                    },
                ])
            }
            Kind::Correct => {
                let (current, title, summary, body, reason) = self.corrected(details)?;
                if title == current.title
                    && summary == current.summary
                    && body == current.body
                    && reason.is_empty()
                {
                    return Err("没有改任何文字".to_owned());
                }
                Self::check_user_words(
                    env,
                    &[
                        ("title", &title),
                        ("summary", &summary),
                        ("body", &body),
                        ("reason", &reason),
                    ],
                    &current,
                )?;
                if title == current.title && summary == current.summary && body == current.body {
                    return Err("没有改任何文字".to_owned());
                }
                let mut facts = Vec::new();
                if title != current.title {
                    facts.push(CardFact {
                        name: "标题".to_owned(),
                        value: format!("{} → {title}", current.title),
                    });
                }
                if summary != current.summary {
                    facts.push(CardFact {
                        name: "摘要".to_owned(),
                        value: format!("{} → {summary}", current.summary),
                    });
                }
                if body != current.body {
                    facts.push(CardFact {
                        name: "正文".to_owned(),
                        value: format!("{} → {body}", current.body),
                    });
                }
                if !reason.is_empty() {
                    facts.push(CardFact {
                        name: "理由（用户原话）".to_owned(),
                        value: reason,
                    });
                }
                Ok(facts)
            }
            Kind::Pin => {
                let item = self.item(&Self::id(details)?)?;
                let mut facts = Self::judgment(
                    item.title.clone(),
                    format!("定下「{}」这条笔记，新条目有约束力", item.title),
                );
                facts.push(CardFact {
                    name: "摘要".to_owned(),
                    value: item.summary,
                });
                Ok(facts)
            }
            Kind::DreamSettings => {
                let settings = self
                    .store()?
                    .dream_settings()
                    .map_err(|error| format!("做梦设置读不出来：{error}"))?;
                let mut facts = Vec::new();
                if let Some(enabled) = details.get("enabled").and_then(Value::as_bool) {
                    facts.push(CardFact {
                        name: "做梦".to_owned(),
                        value: format!(
                            "{} → {}",
                            if settings.enabled { "开" } else { "关" },
                            if enabled { "开" } else { "关" }
                        ),
                    });
                }
                if let Some(hours) = details.get("interval_hours").and_then(Value::as_u64) {
                    facts.push(CardFact {
                        name: "间隔".to_owned(),
                        value: format!("{} 小时 → {hours} 小时", settings.interval_hours),
                    });
                }
                Ok(facts)
            }
            Kind::Permit => {
                let settings = self
                    .store()?
                    .dream_settings()
                    .map_err(|error| format!("做梦设置读不出来：{error}"))?;
                let allow = Self::flag(details, "allow")?;
                Ok(vec![CardFact {
                    name: "观察许可".to_owned(),
                    value: format!(
                        "{} → {}",
                        if settings.permit {
                            "允许"
                        } else {
                            "暂不允许"
                        },
                        if allow { "允许" } else { "暂不允许" }
                    ),
                }])
            }
            Kind::ResolveReplace => {
                let card = Self::text(details, "card")?;
                let record = self
                    .store()?
                    .card(&card)
                    .map_err(|error| format!("卡片读不出来：{error}"))?
                    .ok_or_else(|| format!("找不到这张卡：{card}"))?;
                let replace = Self::flag(details, "replace")?;
                let mut facts = Self::judgment(
                    card.clone(),
                    format!(
                        "替换卡「{card}」：{}",
                        if replace {
                            "替换旧的，留新的"
                        } else {
                            "两条都留，旧的照旧"
                        }
                    ),
                );
                facts.push(CardFact {
                    name: "卡片状态".to_owned(),
                    value: record.status,
                });
                Ok(facts)
            }
            Kind::ResolveAboutDraft => {
                let card = Self::text(details, "card")?;
                let record = self
                    .store()?
                    .card(&card)
                    .map_err(|error| format!("卡片读不出来：{error}"))?
                    .ok_or_else(|| format!("找不到这张卡：{card}"))?;
                let save = Self::flag(details, "save")?;
                let mut facts = Self::judgment(
                    card.clone(),
                    format!(
                        "关于你草稿「{card}」：{}",
                        if save {
                            "保存，进「关于你」"
                        } else {
                            "不用，关于你没动"
                        }
                    ),
                );
                facts.push(CardFact {
                    name: "卡片状态".to_owned(),
                    value: record.status,
                });
                Ok(facts)
            }
        }
    }

    fn act<'a>(
        &'a self,
        details: &'a Value,
        host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move {
            let Some(memory) = host.memory_runtime() else {
                return Err("记忆库没开".to_owned());
            };
            let dream = host.dream_runtime();
            let mut messages = Vec::new();
            let requests = match self.kind {
                Kind::DreamSettings => {
                    let mut requests = Vec::new();
                    if let Some(enabled) = details.get("enabled").and_then(Value::as_bool) {
                        requests.push(MemoryActionKind::SetDreamEnabled { enabled });
                    }
                    if let Some(hours) = details.get("interval_hours").and_then(Value::as_u64) {
                        requests.push(MemoryActionKind::SetDreamInterval { hours });
                    }
                    requests
                }
                _ => vec![self.kind_action(details)?],
            };
            for action in requests {
                let result = crate::memory::page::action(
                    Some(memory),
                    dream,
                    &MemoryActionRequest { action },
                )
                .map_err(|error| error.to_string())?;
                host.emit_memory_changed(result.version);
                if result.trigger_dream {
                    host.dream_check();
                }
                messages.push(result.message);
            }
            Ok(ActionOutcome {
                summary: messages.join("；"),
                data: json!({}),
            })
        })
    }
}
