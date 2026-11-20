//! 记忆的服务层：打开库，给 Kind/ThinAgent 拼注入文本，压缩前把日记写进库。

use std::path::Path;
use std::sync::Arc;

use agent::{MemoryContext, MemoryContextProvider, PreCompactionDiary, PreCompactionHook};
mod dream;
pub(crate) mod page;
mod tools;
#[cfg(test)]
pub(crate) use dream::task_facts;
pub use dream::{
    DREAM_PERMIT_CARD_REF, DreamCheck, DreamReport, DreamRuntime, DreamTaskFact, DreamTrigger,
};
pub use tools::memory_tools;

use memory::MemoryStore;
use memory::types::{Binding, MemoryItem, WriteContext, WriteOp};

pub struct MemoryRuntime {
    store: Arc<MemoryStore>,
}

/// 程序送进会话的 user 消息前缀（中继卡、任务停止、抄送、重启说明、门判定、上游变更）。
/// 它们不是用户自己说的话，不能当「记住」的出处，也不进 Kind 对话索引。
const PROGRAM_MESSAGE_PREFIXES: [&str; 5] = [
    "task-",
    "restart:",
    "gate-verdict:",
    "upstream-changed:",
    "appointment-wake:",
];

/// 这条 user 消息是不是用户自己发的（程序送来的不算）；索引和「记住」的闸共用这一份。
pub(crate) fn user_message_sent(message: &model::Message) -> bool {
    let model::MessageSource::User { rpc_id } = &message.source else {
        return false;
    };
    rpc_id.as_deref().is_none_or(|id| {
        !PROGRAM_MESSAGE_PREFIXES
            .iter()
            .any(|prefix| id.starts_with(prefix))
    })
}

impl MemoryRuntime {
    pub fn open(data_dir: &Path) -> Result<Self, memory::MemoryError> {
        let store = MemoryStore::open(data_dir.join("memory.sqlite"))?;
        if let Some(report) = store.migration_report() {
            eprintln!(
                "记忆库迁移 v{}→v{}：备份 {}；缺原文 {} 行",
                report.from_version,
                report.to_version,
                report.backup.as_deref().unwrap_or("无"),
                report.missing_original
            );
        }
        Ok(Self {
            store: Arc::new(store),
        })
    }

    pub fn store(&self) -> &Arc<MemoryStore> {
        &self.store
    }

    /// Kind：关于你全文 + 条目索引；都没有就返回 None（不往空会话塞空记忆）。
    pub fn kind_context(&self) -> Option<String> {
        self.kind_context_with_ids().map(|(text, _)| text)
    }

    /// 同 kind_context，并带这次注入里的记忆 id（TurnEnd succeeded 落 delivery）。
    pub fn kind_context_with_ids(&self) -> Option<(String, Vec<String>)> {
        let view = self.store.view().ok()?;
        let mut parts = Vec::new();
        if !view.about_you.is_empty() {
            parts.push("关于你：".to_owned());
            parts.extend(
                view.about_you
                    .iter()
                    .map(|item| format!("- {}", item_text(item))),
            );
        }
        let entries: Vec<&MemoryItem> = view.entries.iter().chain(view.notes.iter()).collect();
        if !entries.is_empty() {
            parts.push("条目索引：".to_owned());
            parts.extend(entries.iter().map(|item| {
                format!(
                    "- {} · {} · {} · {}",
                    item.id,
                    item.title,
                    item.scope.as_str(),
                    item.binding.as_str()
                )
            }));
        }
        if parts.is_empty() {
            None
        } else {
            let ids = view
                .about_you
                .iter()
                .chain(view.entries.iter())
                .chain(view.notes.iter())
                .map(|item| item.id.clone())
                .collect();
            Some((
                parts.join(
                    "
",
                ),
                ids,
            ))
        }
    }

    /// ThinAgent：只给个人范围、有约束力的条目。
    pub fn thin_context(&self) -> Option<String> {
        self.thin_context_with_ids().map(|(text, _)| text)
    }

    pub fn thin_context_with_ids(&self) -> Option<(String, Vec<String>)> {
        let view = self.store.view().ok()?;
        let mut parts = Vec::new();
        for item in view.about_you.iter().chain(view.entries.iter()) {
            if item.binding == Binding::Binding && item.status == "active" {
                parts.push(format!("- {}", item_text(item)));
            }
        }
        if parts.is_empty() {
            None
        } else {
            let ids = view
                .about_you
                .iter()
                .chain(view.entries.iter())
                .filter(|item| item.binding == Binding::Binding && item.status == "active")
                .map(|item| item.id.clone())
                .collect();
            Some((
                "有约束力的记忆：
"
                .to_owned()
                    + &parts.join(
                        "
",
                    ),
                ids,
            ))
        }
    }

    /// TurnEnd succeeded 时由 agent 能力回调用：把这次注入的整份记忆记 sent。
    pub fn delivery_hook(&self) -> agent::MemoryDeliveryHook {
        let store = Arc::clone(&self.store);
        Arc::new(move |delivery: agent::MemoryDelivery| {
            if let Err(error) = store.record_delivery(
                &delivery.ids,
                delivery.conversation.as_str(),
                delivery.turn,
                "sent",
            ) {
                eprintln!("记忆送达落账失败：{error}");
            }
        })
    }

    pub fn kind_provider(&self) -> MemoryContextProvider {
        let runtime = Arc::new(Self {
            store: Arc::clone(&self.store),
        });
        Arc::new(move || {
            let identity = runtime
                .store
                .version()
                .map_err(|error| error.to_string())?
                .to_string();
            Ok(runtime
                .kind_context_with_ids()
                .map(|(text, ids)| MemoryContext {
                    identity,
                    text,
                    ids,
                }))
        })
    }

    pub fn thin_provider(&self) -> MemoryContextProvider {
        let runtime = Arc::new(Self {
            store: Arc::clone(&self.store),
        });
        Arc::new(move || {
            let identity = runtime
                .store
                .version()
                .map_err(|error| error.to_string())?
                .to_string();
            Ok(runtime
                .thin_context_with_ids()
                .map(|(text, ids)| MemoryContext {
                    identity,
                    text,
                    ids,
                }))
        })
    }

    /// 程序卡的状态按 ref 从库里读：观察许可、做梦 run、或记忆的当前状态。
    pub fn card_state(&self, reference: &str) -> Option<String> {
        if let Ok(Some(card)) = self.store.card(reference) {
            return Some(
                match card.status.as_str() {
                    "pending" => "active",
                    other => other,
                }
                .to_owned(),
            );
        }
        if reference == DREAM_PERMIT_CARD_REF {
            let settings = self.store.dream_settings().ok()?;
            return Some(
                if settings.permit {
                    "accepted"
                } else {
                    "active"
                }
                .to_owned(),
            );
        }
        if let Ok(Some(run)) = self.store.dream_run(reference) {
            return Some(
                match run.outcome.as_str() {
                    // 整理完但改动都撤掉了：卡片显示已撤销。
                    "done" if self.run_undone(&run.id) => "revoked",
                    "done" => "active",
                    "failed" => "failed",
                    _ => "running",
                }
                .to_owned(),
            );
        }
        if let Ok(Some(item)) = self.store.item(reference) {
            return Some(
                match item.status.as_str() {
                    "forgotten" => "revoked",
                    "quarantine" => "deleted",
                    "pinned" => "accepted",
                    _ => "active",
                }
                .to_owned(),
            );
        }
        None
    }

    /// 这次做梦的改动是不是都撤掉了：新记的笔记撤销后 change 行会删掉；
    /// 改正/合并/作废撤销后留一条 undoOf 指向这次写的修订。
    pub(crate) fn run_undone(&self, run_id: &str) -> bool {
        let Ok(Some(run)) = self.store.dream_run(run_id) else {
            return false;
        };
        let Ok(changes) = self.store.dream_changes(run_id) else {
            return false;
        };
        // 一处改动都没有：可能是这次本来就没改动（整理完了），也可能是新记的笔记撤销时
        // change 行被删了（已撤销）。用成功时记下的 applied 数分辨，不靠 changes 是否为空。
        if changes.is_empty() {
            let applied = serde_json::from_str::<serde_json::Value>(&run.report_json)
                .ok()
                .and_then(|report| report.get("applied").and_then(serde_json::Value::as_u64));
            return applied.is_some_and(|count| count > 0);
        }
        changes.iter().all(|change| {
            self.store
                .revisions(&change.memory_id)
                .map(|revisions| {
                    revisions
                        .iter()
                        .any(|revision| revision.undo_of == Some(change.after_revision))
                })
                .unwrap_or(false)
        })
    }

    /// 只索引用户自己的话和 Kind 回答正文；工具结果、推理、注入、记忆都不进索引。
    pub fn index_session_event(&self, conversation_id: &str, event: &session::Event) {
        let (turn, text) = match &event.kind {
            session::EventKind::UserMessage { turn, message, .. } => {
                // 中继卡/停任务/抄送/重启/门判定/上游变更也以 user 消息落盘，
                // 但它们不是用户自己的话，不进对话索引；backfill 走同一个函数。
                if !user_message_sent(message) {
                    return;
                }
                (*turn, message_text(message))
            }
            session::EventKind::AssistantMessage { turn, message, .. } => {
                if message.role != model::Role::Assistant {
                    return;
                }
                // 轮次用事件自带的 turn：搜索命中要能指回那一轮。
                (*turn, message_text(message))
            }
            _ => return,
        };
        if text.trim().is_empty() {
            return;
        }
        if let Err(error) = self.store.write(
            WriteOp::IndexConversation {
                conversation_id: conversation_id.to_owned(),
                seq: event.seq,
                turn,
                text,
            },
            &WriteContext::user(),
        ) {
            // 索引失败只记日志，不影响对话本身。
            eprintln!("会话 {conversation_id} 的对话索引没写进记忆库：{error}");
        }
    }

    /// 启动补齐：把会话日志里还没索引的 seq 补进去；失败只记日志。
    pub fn backfill(&self, data_dir: &Path, conversation_id: &str) {
        let Ok(id) = session::SessionId::parse(conversation_id) else {
            return;
        };
        let Ok(store) = session::SessionStore::open_read_only(data_dir, &id) else {
            return;
        };
        for event in store.events() {
            self.index_session_event(conversation_id, event);
        }
    }

    pub fn pre_compaction_hook(&self) -> PreCompactionHook {
        let store = Arc::clone(&self.store);
        Arc::new(move |diary: PreCompactionDiary| {
            let store = Arc::clone(&store);
            Box::pin(async move {
                if diary.text.trim().is_empty() {
                    return Ok(());
                }
                store
                    .write(
                        WriteOp::Diary {
                            text: diary.text,
                            source_ref: diary.compaction_id,
                        },
                        &WriteContext::kind(),
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        })
    }
}

fn message_text(message: &model::Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            model::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn item_text(item: &MemoryItem) -> String {
    let body = match (item.summary.is_empty(), item.body.is_empty()) {
        (true, true) => String::new(),
        (true, false) => item.body.clone(),
        (false, true) => item.summary.clone(),
        (false, false) => format!("{}：{}", item.summary, item.body),
    };
    if item.title.is_empty() {
        body
    } else if body.is_empty() {
        item.title.clone()
    } else {
        format!("{}：{}", item.title, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory::types::{WriteContext, WriteOp};

    #[tokio::test]
    async fn pre_compaction_hook_writes_diary_and_skips_empty() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = MemoryRuntime::open(dir.path()).unwrap();
        let hook = runtime.pre_compaction_hook();
        hook(agent::PreCompactionDiary {
            compaction_id: "compaction-test".to_owned(),
            conversation: session::SessionId::parse("00000000-0000-4000-8000-000000000001")
                .unwrap(),
            text: "   ".to_owned(),
        })
        .await
        .unwrap();
        assert!(runtime.store().view().unwrap().diary.is_empty());
        hook(agent::PreCompactionDiary {
            compaction_id: "compaction-test".to_owned(),
            conversation: session::SessionId::parse("00000000-0000-4000-8000-000000000001")
                .unwrap(),
            text: "用户偏好先给结论".to_owned(),
        })
        .await
        .unwrap();
        let view = runtime.store().view().unwrap();
        assert_eq!(view.diary.len(), 1);
        assert_eq!(view.diary[0].text, "用户偏好先给结论");
    }

    #[test]
    fn card_state_reads_current_memory_status() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = MemoryRuntime::open(dir.path()).unwrap();
        let item = runtime
            .store()
            .write(
                WriteOp::Note {
                    title: "卡片".to_owned(),
                    summary: String::new(),
                    body: "正文".to_owned(),
                    source_kind: "kind".to_owned(),
                    source_ref: "card".to_owned(),
                },
                &WriteContext::kind(),
            )
            .unwrap()
            .item
            .unwrap();
        assert_eq!(runtime.card_state(&item.id).as_deref(), Some("active"));
        runtime
            .store()
            .write(
                WriteOp::Delete {
                    id: item.id.clone(),
                    reason: "user".to_owned(),
                },
                &WriteContext::user(),
            )
            .unwrap();
        assert_eq!(runtime.card_state(&item.id).as_deref(), Some("deleted"));
        runtime
            .store()
            .write(
                WriteOp::Forget {
                    id: item.id.clone(),
                },
                &WriteContext::user(),
            )
            .unwrap();
        assert_eq!(runtime.card_state(&item.id).as_deref(), Some("revoked"));
        assert_eq!(runtime.card_state("missing"), None);
    }

    #[test]
    fn kind_thin_sections_differ_and_follow_store_changes() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = MemoryRuntime::open(dir.path()).unwrap();
        runtime
            .store()
            .write(
                WriteOp::Remember {
                    title: "回答先给结论".to_owned(),
                    summary: "先结论".to_owned(),
                    body: "再给理由".to_owned(),
                    source_kind: "user".to_owned(),
                    source_ref: "u1".to_owned(),
                },
                &WriteContext::user(),
            )
            .unwrap();
        runtime
            .store()
            .write(
                WriteOp::Note {
                    title: "GBK CSV".to_owned(),
                    summary: "先看编码".to_owned(),
                    body: "sources/ 下有 GBK".to_owned(),
                    source_kind: "kind".to_owned(),
                    source_ref: "k1".to_owned(),
                },
                &WriteContext::kind(),
            )
            .unwrap();
        let kind = runtime.kind_context().unwrap();
        assert!(kind.contains("关于你"));
        assert!(kind.contains("回答先给结论"));
        assert!(kind.contains("条目索引"));
        assert!(kind.contains("GBK CSV"));
        let thin = runtime.thin_context().unwrap();
        assert!(thin.contains("有约束力的记忆"));
        assert!(thin.contains("回答先给结论"));
        assert!(!thin.contains("GBK CSV"), "ThinAgent 只拿有约束力的条目");
        let before = runtime.kind_provider()().unwrap().unwrap().identity;
        runtime
            .store()
            .write(
                WriteOp::Note {
                    title: "新笔记".to_owned(),
                    summary: String::new(),
                    body: "新".to_owned(),
                    source_kind: "kind".to_owned(),
                    source_ref: "k2".to_owned(),
                },
                &WriteContext::kind(),
            )
            .unwrap();
        let after = runtime.kind_provider()().unwrap().unwrap().identity;
        assert_ne!(before, after);
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;
    use memory::types::SearchHit;

    #[test]
    fn conversation_index_is_filtered_and_idempotent_and_backfills() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let mut store = session::SessionStore::create(dir.path(), cwd.path()).unwrap();
        let id = store.id().clone();
        store
            .append(&[session::EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: model::Message::user_text("用户说了一句要紧的话"),
            }])
            .unwrap();
        store
            .append(&[session::EventKind::AssistantMessage {
                turn: 1,
                step: 2,
                message: model::Message::assistant(
                    vec![model::ContentBlock::text("Kind 的回答正文")],
                    "deepseek",
                    "deepseek-flash",
                ),
                stream: Vec::new(),
                usage: None,
                interrupted: None,
                effort: None,
                effort_wanted: None,
            }])
            .unwrap();
        let runtime = MemoryRuntime::open(dir.path()).unwrap();
        for event in store.events() {
            runtime.index_session_event(id.as_str(), event);
            runtime.index_session_event(id.as_str(), event);
        }
        let hits = runtime.store().search("用户说", 10).unwrap();
        assert!(hits.iter().any(|hit| matches!(hit, SearchHit::Conversation { conversation_id, .. } if conversation_id == id.as_str())));
        let hits = runtime.store().search("Kind 的回答", 10).unwrap();
        assert_eq!(
            hits.iter()
                .filter(|hit| matches!(hit, SearchHit::Conversation { .. }))
                .count(),
            1
        );
        drop(store);
        let backfilled = MemoryRuntime::open(dir.path()).unwrap();
        backfilled.backfill(dir.path(), id.as_str());
        let hits = backfilled.store().search("用户说", 10).unwrap();
        assert_eq!(
            hits.iter()
                .filter(|hit| matches!(hit, SearchHit::Conversation { .. }))
                .count(),
            1
        );
    }
}
