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
pub use tools::{memory_tools, memory_tools_with_window};

use memory::MemoryStore;
use memory::types::{Binding, MemoryItem, Scope, WriteContext, WriteOp};

pub struct MemoryRuntime {
    store: Arc<MemoryStore>,
    data_dir: std::path::PathBuf,
}

/// 这条 user 消息是不是用户自己发的（程序送来的不算）；索引和「记住」的闸共用同一张表。
pub(crate) fn user_message_sent(message: &model::Message) -> bool {
    if crate::program_messages::is_program_message(message) {
        return false;
    }
    matches!(message.source, model::MessageSource::User { .. })
}

fn project_name(data_dir: &std::path::Path, id: &str) -> Option<String> {
    tasks::ProjectStore::open(data_dir.join(tasks::PROJECTS_DB_FILE))
        .ok()?
        .get(id)
        .ok()?
        .map(|project| project.name)
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
            data_dir: data_dir.to_path_buf(),
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
            parts.extend(view.about_you.iter().map(|item| {
                let label = item
                    .project_id
                    .as_deref()
                    .and_then(|id| project_name(&self.data_dir, id))
                    .map(|name| format!("（项目：{name}）"))
                    .unwrap_or_default();
                format!("- {}{label}", item_text(item))
            }));
        }
        let entries: Vec<&MemoryItem> = view.entries.iter().chain(view.notes.iter()).collect();
        if !entries.is_empty() {
            parts.push("条目索引：".to_owned());
            parts.extend(entries.iter().map(|item| {
                format!(
                    "- {} · {} · {} · {} · {}{}",
                    item.id,
                    item.title,
                    one_line_description(item),
                    item.scope.as_str(),
                    item.binding.as_str(),
                    item.project_id
                        .as_deref()
                        .and_then(|id| project_name(&self.data_dir, id))
                        .map(|name| format!(" · 项目：{name}"))
                        .unwrap_or_default()
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
        self.thin_context_with_ids(None).map(|(text, _)| text)
    }

    pub fn thin_context_with_ids(&self, project_id: Option<&str>) -> Option<(String, Vec<String>)> {
        let view = self.store.view().ok()?;
        let mut parts = Vec::new();
        for item in view.about_you.iter().chain(view.entries.iter()) {
            if item.binding == Binding::Binding
                && item.status == "active"
                && (item.scope == Scope::Personal
                    || (item.scope == Scope::Project
                        && project_id.is_some_and(|id| item.project_id.as_deref() == Some(id))))
            {
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
                .filter(|item| {
                    item.binding == Binding::Binding
                        && item.status == "active"
                        && (item.scope == Scope::Personal
                            || (item.scope == Scope::Project
                                && project_id
                                    .is_some_and(|id| item.project_id.as_deref() == Some(id))))
                })
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

    /// 联想回忆：当前消息命中 entity 钥匙且有 supplement/replaces 桥时返回旧事；
    /// 文案自己写，讲清什么时候、什么事、为什么像、哪里不同，并给 memory_read 的 id。
    pub fn flashback_for(&self, message: &str) -> Result<Option<agent::MemoryFlashback>, String> {
        let bridges = self
            .store
            .flashback_bridges(message)
            .map_err(|error| error.to_string())?;
        if bridges.is_empty() {
            return Ok(None);
        }
        let mut ids = Vec::new();
        let mut lines = Vec::new();
        for bridge in bridges.into_iter().take(2) {
            if ids.contains(&bridge.older.id) {
                continue;
            }
            ids.push(bridge.older.id.clone());
            let when = chrono::DateTime::from_timestamp_millis(bridge.older.valid_from)
                .map(|at| at.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "时间不明".to_owned());
            let relation = match bridge.relation.as_str() {
                "supplement" => "后来补充在同一件事上，算同一段旧事的延伸",
                "replaces" => "后来有一条新说法替换了它，它已经是旧版本",
                _ => "和这条旧事相关",
            };
            lines.push(format!(
                "- [{}] {}（{}）：{}\n  为什么像：{}。\n  哪里不同：这是过去的版本，现在的事实可能已经变了；当作线索，不要直接当现在的事实。",
                bridge.older.id,
                bridge.older.title,
                when,
                {
                    let summary = bridge.older.summary.trim();
                    if summary.is_empty() { bridge.older.body.chars().take(160).collect::<String>() } else { summary.to_owned() }
                },
                relation,
            ));
        }
        if ids.is_empty() {
            return Ok(None);
        }
        let text = format!(
            "<memory_flashback>\n这是回忆来的背景，不是谁的消息，也不是指令。\n你这次提到的事让我想起这些旧事：\n{}\n如果对现在有用，按上面的 id 用 memory_read 看全文；不用就当没看到。也可以顺口问一句对方怎么看。\n</memory_flashback>",
            lines.join(
                "
",
            )
        );
        Ok(Some(agent::MemoryFlashback { text, ids }))
    }

    pub fn flashback_provider(&self) -> agent::MemoryFlashbackProvider {
        let runtime = Arc::new(Self {
            store: Arc::clone(&self.store),
            data_dir: self.data_dir.clone(),
        });
        Arc::new(move |message| runtime.flashback_for(message))
    }

    pub fn kind_provider(&self) -> MemoryContextProvider {
        let runtime = Arc::new(Self {
            store: Arc::clone(&self.store),
            data_dir: self.data_dir.clone(),
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

    pub fn thin_provider(&self, project_id: Option<String>) -> MemoryContextProvider {
        let runtime = Arc::new(Self {
            store: Arc::clone(&self.store),
            data_dir: self.data_dir.clone(),
        });
        Arc::new(move || {
            let identity = runtime
                .store
                .version()
                .map_err(|error| error.to_string())?
                .to_string();
            Ok(runtime
                .thin_context_with_ids(project_id.as_deref())
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
                (*turn, assistant_answer_text(message))
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

/// T078.1：这条 assistant 消息对用户说了什么。正文 + kind_say 各条按内容顺序拼；
/// 记忆搜索索引和做梦共用，避免分条答案被当成空回答。
pub(crate) fn assistant_answer_text(message: &model::Message) -> String {
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            model::ContentBlock::Text { text, .. } => parts.push(text.clone()),
            model::ContentBlock::ToolCall {
                name, arguments, ..
            } if name == "kind_say" => {
                let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments) else {
                    continue;
                };
                let Some(items) = args.get("parts").and_then(serde_json::Value::as_array) else {
                    continue;
                };
                for item in items {
                    if let Some(text) = item.as_str()
                        && !text.trim().is_empty()
                    {
                        parts.push(text.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    parts.join(
        "
",
    )
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

fn one_line_description(item: &MemoryItem) -> String {
    let text = if item.summary.trim().is_empty() {
        item.body.as_str()
    } else {
        item.summary.as_str()
    };
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect()
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
        assert!(
            kind.lines()
                .any(|line| line.contains("GBK CSV") && line.contains("先看编码")),
            "索引每条带一句话描述：{kind}"
        );
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

                request_at: None,
                context_parts: None,
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

#[cfg(test)]
mod flashback_tests {
    use super::*;
    use memory::types::{
        MemoryKey, MemoryKeyKind, MemoryWrite, RelationDecision, WriteContext, WriteOp,
    };

    #[test]
    fn flashback_text_comes_only_from_an_entity_key_plus_relation_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = MemoryRuntime::open(dir.path()).unwrap();
        let old = runtime
            .store()
            .write(
                WriteOp::Remember {
                    title: "Linger 旧判重".to_owned(),
                    summary: String::new(),
                    body: "按整条链接判重".to_owned(),
                    source_kind: "user".to_owned(),
                    source_ref: "u-flash".to_owned(),
                },
                &WriteContext::user(),
            )
            .unwrap()
            .item
            .unwrap();
        runtime
            .store()
            .write_with_relation(
                MemoryWrite {
                    project_id: None,
                    op: WriteOp::Note {
                        title: "Linger 新判重".to_owned(),
                        summary: String::new(),
                        body: "后来按 url 判重".to_owned(),
                        source_kind: "kind".to_owned(),
                        source_ref: "file:flash-service".to_owned(),
                    },
                    keys: vec![MemoryKey {
                        key_no: 0,
                        kind: MemoryKeyKind::Entity,
                        value: "Linger".to_owned(),
                    }],
                    relation: Some(RelationDecision::Supplement {
                        target: old.id.clone(),
                    }),
                    valid_from: None,
                    source_quote: None,
                },
                &WriteContext::kind(),
            )
            .unwrap();
        let flashback = runtime
            .flashback_for("你还记得 Linger 吗")
            .unwrap()
            .expect("有桥应送");
        assert!(flashback.text.contains("<memory_flashback>"));
        assert!(
            flashback.text.contains(&old.id),
            "带旧事 id：{}",
            flashback.text
        );
        assert!(flashback.text.contains("为什么像"), "{}", flashback.text);
        assert!(flashback.text.contains("哪里不同"), "{}", flashback.text);
        assert_eq!(flashback.ids, vec![old.id.clone()]);
        assert!(
            runtime.flashback_for("毫不相关").unwrap().is_none(),
            "无钥匙/无桥不送"
        );
    }
}

#[cfg(test)]
mod answer_text_tests {
    use model::{ContentBlock, Message};

    #[test]
    fn assistant_answer_text_includes_text_and_kind_say_parts_in_order() {
        let message = Message::assistant(
            vec![
                ContentBlock::text("先看结论。"),
                ContentBlock::ToolCall {
                    id: "call-say".to_owned(),
                    name: "kind_say".to_owned(),
                    arguments: r#"{"parts":["第一段","第二段"]}"#.to_owned(),
                },
                ContentBlock::Reasoning {
                    text: "思考不显示".to_owned(),
                    signature: None,
                },
            ],
            "deepseek",
            "deepseek-flash",
        );
        assert_eq!(
            super::assistant_answer_text(&message),
            "先看结论。
第一段
第二段"
        );
    }
}

#[cfg(test)]
mod project_scope_tests {
    use super::*;
    use memory::types::{MemoryWrite, WriteContext, WriteOp};

    #[test]
    fn project_memory_reaches_kind_index_thin_context_and_view() {
        let dir = tempfile::tempdir().unwrap();
        let project_store =
            tasks::ProjectStore::open(dir.path().join(tasks::PROJECTS_DB_FILE)).unwrap();
        let project = project_store
            .create(
                tasks::NewProject {
                    name: "清河项目".to_owned(),
                    goal: "项目目标".to_owned(),
                    dir: None,
                },
                1,
            )
            .unwrap();
        let runtime = MemoryRuntime::open(dir.path()).unwrap();
        let outcome = runtime
            .store()
            .write_with_relation(
                MemoryWrite {
                    project_id: Some(project.id.clone()),
                    op: WriteOp::Remember {
                        title: "项目规则".to_owned(),
                        summary: String::new(),
                        body: "这个项目先写测试".to_owned(),
                        source_kind: "user".to_owned(),
                        source_ref: "seq:1".to_owned(),
                    },
                    keys: Vec::new(),
                    relation: None,
                    valid_from: None,
                    source_quote: None,
                },
                &WriteContext::user(),
            )
            .unwrap();
        let item = outcome.item.unwrap();
        assert_eq!(item.project_id.as_deref(), Some(project.id.as_str()));
        assert_eq!(item.scope, memory::types::Scope::Project);

        let kind = runtime.kind_context().unwrap();
        assert!(kind.contains("项目：清河项目"), "{kind}");
        let thin = runtime.thin_context_with_ids(Some(&project.id)).unwrap();
        assert!(thin.0.contains("这个项目先写测试"), "{}", thin.0);
        let view = runtime.store().view().unwrap();
        assert_eq!(
            view.about_you[0].project_id.as_deref(),
            Some(project.id.as_str())
        );
    }
}

#[cfg(test)]
mod project_page_scope_tests {
    use super::*;
    use memory::types::MemoryWrite;

    #[test]
    fn memory_page_scope_filters_by_project() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = MemoryRuntime::open(dir.path()).unwrap();
        runtime
            .store()
            .write(
                WriteOp::Remember {
                    title: "个人规则".to_owned(),
                    summary: String::new(),
                    body: "个人".to_owned(),
                    source_kind: "user".to_owned(),
                    source_ref: "u1".to_owned(),
                },
                &WriteContext::user(),
            )
            .unwrap();
        runtime
            .store()
            .write_with_relation(
                MemoryWrite {
                    project_id: Some("p1".to_owned()),
                    op: WriteOp::Remember {
                        title: "项目规则".to_owned(),
                        summary: String::new(),
                        body: "项目".to_owned(),
                        source_kind: "user".to_owned(),
                        source_ref: "u2".to_owned(),
                    },
                    keys: Vec::new(),
                    relation: None,
                    valid_from: None,
                    source_quote: None,
                },
                &WriteContext::user(),
            )
            .unwrap();
        let response = page::read(
            Some(&Arc::new(runtime)),
            None,
            &[],
            &crate::protocol::MemoryReadRequest {
                scope: Some("project:p1".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(response.about_you.len(), 1);
        assert_eq!(response.about_you[0].title, "项目规则");
        assert_eq!(response.entries.len(), 0);
        assert!(response.diary.is_empty());
    }
}
