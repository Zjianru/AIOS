//! 记忆页的服务端查询/变更：13 篇 4.1 的 M02/M05/M11 查询与 M01/M03/M04/M10/M12/M13/M14 变更。
//!
//! 页面只认这里的两个入口；库的 actor 校验、修订、FTS 都在 crates/memory。

use std::collections::HashSet;
use std::sync::Arc;

use memory::types::{
    Binding, DreamChange, Layer, MemoryItem, RevisionItem, SearchHit, WriteContext, WriteOp,
};

use super::{DreamRuntime, MemoryRuntime};
use crate::error::ServiceError;
use crate::protocol::{
    MemoryActionKind, MemoryActionRequest, MemoryCardStateView, MemoryDiaryView,
    MemoryDreamChangeView, MemoryDreamRunView, MemoryDreamView, MemoryHitView, MemoryItemView,
    MemoryReadRequest, MemoryReadResponse, MemoryRevisionView,
};

fn unavailable(error: impl std::fmt::Display) -> ServiceError {
    ServiceError::MemoryUnavailable(error.to_string())
}

fn item_view(item: &MemoryItem) -> MemoryItemView {
    MemoryItemView {
        id: item.id.clone(),
        layer: item.layer.as_str().to_owned(),
        binding: item.binding.as_str().to_owned(),
        scope: item.scope.as_str().to_owned(),
        title: item.title.clone(),
        summary: item.summary.clone(),
        body: item.body.clone(),
        source_kind: item.source_kind.clone(),
        source_ref: item.source_ref.clone(),
        status: item.status.clone(),
        revision: item.current_revision,
        updated_at: item.updated_at,
        pinned: item.pinned,
    }
}

fn revision_view(revision: &RevisionItem) -> MemoryRevisionView {
    MemoryRevisionView {
        revision: revision.revision,
        title: revision.title.clone(),
        body: revision.body.clone(),
        reason: revision.reason.clone(),
        actor: revision.actor.clone(),
        dream_run_id: revision.dream_run_id.clone(),
        created_at: revision.created_at,
    }
}

fn hit_view(hit: &SearchHit) -> MemoryHitView {
    match hit {
        SearchHit::Memory(item) => MemoryHitView {
            kind: "memory".to_owned(),
            id: item.id.clone(),
            title: item.title.clone(),
            snippet: [item.summary.as_str(), item.body.as_str()]
                .into_iter()
                .find(|text| !text.is_empty())
                .unwrap_or("")
                .to_owned(),
            conversation_id: None,
            seq: None,
            turn: None,
        },
        SearchHit::Diary(entry) => MemoryHitView {
            kind: "diary".to_owned(),
            id: entry.id.clone(),
            title: entry.day.clone(),
            snippet: entry.text.clone(),
            conversation_id: None,
            seq: None,
            turn: None,
        },
        SearchHit::Conversation {
            conversation_id,
            seq,
            turn,
            text,
        } => MemoryHitView {
            kind: "conversation".to_owned(),
            id: conversation_id.clone(),
            title: "Kind 对话".to_owned(),
            snippet: text.clone(),
            conversation_id: Some(conversation_id.clone()),
            seq: Some(*seq),
            turn: Some(*turn),
        },
    }
}

fn dream_view(memory: &MemoryRuntime, dream: Option<&Arc<DreamRuntime>>) -> MemoryDreamView {
    let store = memory.store();
    let mut runs = Vec::new();
    for run in store.dream_runs(5).unwrap_or_default() {
        let changes = store.dream_changes(&run.id).unwrap_or_default();
        let views = changes
            .iter()
            .map(|change| dream_change_view(store, change))
            .collect();
        runs.push(MemoryDreamRunView {
            id: run.id,
            started_at: run.started_at,
            ended_at: run.ended_at,
            outcome: run.outcome,
            error: run.error,
            model: run.model,
            report_json: run.report_json,
            changes: views,
        });
    }
    let settings = store.dream_settings().unwrap_or_default();
    let interval_ms =
        i64::try_from(settings.interval_hours.saturating_mul(3_600_000)).unwrap_or(i64::MAX);
    let last_mark = settings.last_success_at.max(settings.last_attempt_at);
    let next_at = if settings.enabled && settings.permit {
        Some(last_mark.saturating_add(interval_ms))
    } else {
        None
    };
    let running = dream.is_some_and(|dream| dream.is_running());
    runs.truncate(5);
    MemoryDreamView {
        enabled: settings.enabled,
        permit: settings.permit,
        interval_hours: settings.interval_hours,
        last_success_at: settings.last_success_at,
        next_at,
        running,
        runs,
    }
}

fn dream_change_view(
    store: &Arc<memory::MemoryStore>,
    change: &DreamChange,
) -> MemoryDreamChangeView {
    let current = store.item(&change.memory_id).ok().flatten();
    let undone = store
        .revisions(&change.memory_id)
        .map(|revisions| {
            revisions
                .iter()
                .any(|revision| revision.undo_of == Some(change.after_revision))
        })
        .unwrap_or(false);
    let reversible = !undone
        && current
            .as_ref()
            .is_some_and(|item| item.current_revision == change.after_revision);
    MemoryDreamChangeView {
        memory_id: change.memory_id.clone(),
        title: current.map(|item| item.title).unwrap_or_default(),
        action: change.action.clone(),
        reason: change.reason.clone(),
        before_revision: change.before_revision,
        after_revision: change.after_revision,
        reversible,
        undone,
    }
}

/// 程序卡（许可/整理完/没跑成）的状态：请求里的 ref 先算，再从根会话日志里补最近的卡。
fn card_states(
    memory: &MemoryRuntime,
    root_events: &[session::Event],
    refs: &[String],
) -> Vec<MemoryCardStateView> {
    let mut seen = HashSet::new();
    let mut cards = Vec::new();
    for reference in refs {
        if !seen.insert(reference.clone()) {
            continue;
        }
        if let Some(state) = memory.card_state(reference) {
            cards.push(MemoryCardStateView {
                reference: reference.clone(),
                state,
                error: card_error(memory, reference),
            });
        }
    }
    for event in root_events.iter().rev() {
        let session::EventKind::MemoryCard { reference, .. } = &event.kind else {
            continue;
        };
        let Some(reference) = reference else {
            continue;
        };
        if !seen.insert(reference.clone()) {
            continue;
        }
        let Some(state) = memory.card_state(reference) else {
            continue;
        };
        cards.push(MemoryCardStateView {
            reference: reference.clone(),
            state,
            error: card_error(memory, reference),
        });
        if cards.len() >= 100 {
            break;
        }
    }
    cards
}

/// 失败卡片的一行原因：按 ref 读到没跑成的做梦 run 的 error。
fn card_error(memory: &MemoryRuntime, reference: &str) -> Option<String> {
    let run = memory.store().dream_run(reference).ok().flatten()?;
    if run.outcome != "failed" {
        return None;
    }
    Some(if run.error.trim().is_empty() {
        "没跑成，没记下原因".to_owned()
    } else {
        run.error.trim().to_owned()
    })
}

/// 记忆页读取。
pub fn read(
    memory: Option<&Arc<MemoryRuntime>>,
    dream: Option<&Arc<DreamRuntime>>,
    root_events: &[session::Event],
    request: &MemoryReadRequest,
) -> Result<MemoryReadResponse, ServiceError> {
    let Some(memory) = memory else {
        return Err(unavailable("记忆库没打开"));
    };
    let store = memory.store();
    let view = store.view().map_err(unavailable)?;
    let mut search = Vec::new();
    if let Some(query) = request.query.as_deref()
        && !query.trim().is_empty()
    {
        search = store
            .search(query, 30)
            .map_err(unavailable)?
            .iter()
            .map(hit_view)
            .collect();
    }
    let revisions = match request.item_id.as_deref() {
        Some(id) => store
            .revisions(id)
            .map_err(unavailable)?
            .iter()
            .map(revision_view)
            .collect(),
        None => Vec::new(),
    };
    let version = store.version().map_err(unavailable)?;
    Ok(MemoryReadResponse {
        version,
        about_you: view.about_you.iter().map(item_view).collect(),
        entries: view.entries.iter().map(item_view).collect(),
        notes: view.notes.iter().map(item_view).collect(),
        diary: view
            .diary
            .iter()
            .map(|entry| MemoryDiaryView {
                id: entry.id.clone(),
                day: entry.day.clone(),
                text: entry.text.clone(),
                source_ref: entry.source_ref.clone(),
                created_at: entry.created_at,
            })
            .collect(),
        quarantine: view.quarantine.iter().map(item_view).collect(),
        search,
        cards: card_states(memory, root_events, &request.refs),
        dream: dream_view(memory, dream),
        item_revisions: revisions,
    })
}

/// 一次记忆变更的结果：新版本 + 一句话回执 + 是否该顺手查一次做梦。
pub struct ActionResult {
    pub version: u64,
    pub message: String,
    pub trigger_dream: bool,
}

/// 记忆页变更（一次只改一个对象/一处动作）。
pub fn action(
    memory: Option<&Arc<MemoryRuntime>>,
    dream: Option<&Arc<DreamRuntime>>,
    request: &MemoryActionRequest,
) -> Result<ActionResult, ServiceError> {
    let Some(memory) = memory else {
        return Err(unavailable("记忆库没打开"));
    };
    let store = memory.store();
    let outcome = |message: &str| ActionResult {
        version: store.version().unwrap_or_default(),
        message: message.to_owned(),
        trigger_dream: false,
    };
    match &request.action {
        MemoryActionKind::Correct {
            id,
            title,
            summary,
            body,
            reason,
        } => {
            store
                .write(
                    WriteOp::Correct {
                        id: id.clone(),
                        title: title.clone(),
                        summary: summary.clone(),
                        body: body.clone(),
                        reason: reason.clone(),
                    },
                    &WriteContext::user(),
                )
                .map_err(unavailable)?;
            Ok(outcome("改好了；这一版算你改的。"))
        }
        MemoryActionKind::Delete { id } => {
            store
                .write(
                    WriteOp::Delete {
                        id: id.clone(),
                        reason: "user".to_owned(),
                    },
                    &WriteContext::user(),
                )
                .map_err(unavailable)?;
            Ok(outcome("已移到隔离区，7 天内能恢复。"))
        }
        MemoryActionKind::Restore { id } => {
            store
                .write(WriteOp::Restore { id: id.clone() }, &WriteContext::user())
                .map_err(unavailable)?;
            Ok(outcome("恢复了。"))
        }
        MemoryActionKind::Forget { id } => {
            store
                .write(WriteOp::Forget { id: id.clone() }, &WriteContext::user())
                .map_err(unavailable)?;
            Ok(outcome("彻底删了，搜不到了。"))
        }
        MemoryActionKind::Pin { id } => {
            store
                .write(WriteOp::Pin { id: id.clone() }, &WriteContext::user())
                .map_err(unavailable)?;
            Ok(outcome("定下来了：新条目有约束力，原笔记标成已定下来。"))
        }
        MemoryActionKind::UndoDream { run_id, memory_id } => {
            let Some(dream) = dream else {
                return Err(unavailable("做梦没装配"));
            };
            let report = dream
                .undo_run(run_id.as_deref(), memory_id.as_deref())
                .map_err(unavailable)?;
            let mut message = format!("撤销了 {} 处", report.applied);
            if !report.skipped.is_empty() {
                message.push_str(&format!("；{} 处你后来改过，跳过", report.skipped.len()));
            }
            Ok(outcome(&message))
        }
        MemoryActionKind::SetDreamEnabled { enabled } => {
            if dream.is_none() {
                return Err(unavailable("做梦没装配"));
            }
            store
                .update_dream_settings(&memory::types::DreamSettingsPatch {
                    enabled: Some(*enabled),
                    ..Default::default()
                })
                .map_err(unavailable)?;
            let mut result = outcome(if *enabled {
                "做梦开了。"
            } else {
                "做梦关了；已经整理过的笔记不动。"
            });
            result.trigger_dream = *enabled;
            Ok(result)
        }
        MemoryActionKind::SetDreamInterval { hours } => {
            if *hours == 0 {
                return Err(unavailable("间隔至少 1 小时"));
            }
            store
                .update_dream_settings(&memory::types::DreamSettingsPatch {
                    interval_hours: Some(*hours),
                    ..Default::default()
                })
                .map_err(unavailable)?;
            Ok(outcome(&format!("间隔改成 {hours} 小时。")))
        }
        MemoryActionKind::SetPermit { allow } => {
            if dream.is_none() {
                return Err(unavailable("做梦没装配"));
            }
            store
                .update_dream_settings(&memory::types::DreamSettingsPatch {
                    permit: Some(*allow),
                    ..Default::default()
                })
                .map_err(unavailable)?;
            let mut result = outcome(if *allow {
                "允许了：空闲时 Kind 会整理它自己的笔记。"
            } else {
                "暂不允许：不读对话、不整理。"
            });
            result.trigger_dream = *allow;
            Ok(result)
        }
        MemoryActionKind::DreamNow => {
            let Some(dream) = dream else {
                return Err(unavailable("做梦没装配"));
            };
            let message = dream.request_now().map_err(unavailable)?;
            Ok(outcome(&message))
        }
    }
}

// 层/分量在视图映射里用 as_str；这里保留引用以免 clippy 认为导入未用。
#[allow(dead_code)]
fn _layers_used(layer: Layer, binding: Binding) -> (&'static str, &'static str) {
    (layer.as_str(), binding.as_str())
}
