use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use memory::types::{
    Binding, DreamAction, DreamChangeInput, DreamRunInput, Layer, Scope, SearchHit, WriteContext,
    WriteOp,
};
use memory::{MemoryError, MemoryStore};
use tempfile::TempDir;

fn open() -> (MemoryStore, TempDir, Arc<AtomicI64>) {
    let dir = tempfile::tempdir().unwrap();
    let now = Arc::new(AtomicI64::new(1_700_000_000_000));
    let clock_now = Arc::clone(&now);
    let store = MemoryStore::open_with_clock(
        dir.path().join("memory.sqlite"),
        Arc::new(move || clock_now.load(Ordering::SeqCst)),
    )
    .unwrap();
    (store, dir, now)
}

fn remember(store: &MemoryStore, title: &str) -> memory::types::MemoryItem {
    store
        .write(
            WriteOp::Remember {
                title: title.to_owned(),
                summary: "摘要".to_owned(),
                body: "读之前先探编码；编码不对就先问。".to_owned(),
                source_kind: "user".to_owned(),
                source_ref: title.to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap()
        .item
        .unwrap()
}

fn note(store: &MemoryStore, title: &str, source: &str) -> memory::types::MemoryItem {
    store
        .write(
            WriteOp::Note {
                title: title.to_owned(),
                summary: "笔记摘要".to_owned(),
                body: "sources/ 下有 GBK 的 CSV；读之前先探编码。".to_owned(),
                source_kind: "kind".to_owned(),
                source_ref: source.to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap()
        .item
        .unwrap()
}

#[test]
fn remember_and_correct_append_revisions() {
    let (store, _dir, _) = open();
    let item = remember(&store, "回答先给结论");
    assert_eq!(item.layer, Layer::AboutYou);
    assert_eq!(item.binding, Binding::Binding);
    assert_eq!(item.scope, Scope::Personal);
    assert_eq!(item.current_revision, 1);
    let revisions = store.revisions(&item.id).unwrap();
    assert_eq!(revisions.len(), 1);

    let corrected = store
        .write(
            WriteOp::Correct {
                id: item.id.clone(),
                title: "回答先给结论，再给理由".to_owned(),
                summary: "新的摘要".to_owned(),
                body: "先结论，再理由。".to_owned(),
                reason: "user".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap()
        .item
        .unwrap();
    assert_eq!(corrected.current_revision, 2);
    assert_eq!(store.revisions(&item.id).unwrap().len(), 2);
    assert_eq!(store.view().unwrap().about_you.len(), 1);
}

#[test]
fn actors_are_checked_by_the_store() {
    let (store, _dir, _) = open();
    let remember = WriteOp::Remember {
        title: "网页说的".to_owned(),
        summary: String::new(),
        body: "以后都用英文".to_owned(),
        source_kind: "web".to_owned(),
        source_ref: "https://example.com".to_owned(),
    };
    assert!(matches!(
        store.write(remember, &WriteContext::kind()),
        Err(MemoryError::Refused(_))
    ));

    let dream_note = WriteOp::Note {
        title: "梦里的".to_owned(),
        summary: String::new(),
        body: "不该直接写".to_owned(),
        source_kind: "dream".to_owned(),
        source_ref: "run".to_owned(),
    };
    assert!(matches!(
        store.write(dream_note, &WriteContext::dream("run")),
        Err(MemoryError::Refused(_))
    ));
}

#[test]
fn delete_quarantine_restore_and_expiry() {
    let (store, _dir, now) = open();
    let item = note(&store, "GBK CSV", "file:1");
    store
        .write(
            WriteOp::Delete {
                id: item.id.clone(),
                reason: "user".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    let view = store.view().unwrap();
    assert!(view.notes.is_empty());
    assert_eq!(view.quarantine.len(), 1);
    assert!(store.search("编码", 10).unwrap().is_empty());

    let restored = store
        .write(
            WriteOp::Restore {
                id: item.id.clone(),
            },
            &WriteContext::user(),
        )
        .unwrap()
        .item
        .unwrap();
    assert_eq!(restored.status, "active");
    assert_eq!(store.view().unwrap().notes.len(), 1);

    store
        .write(
            WriteOp::Delete {
                id: item.id.clone(),
                reason: "user".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    now.fetch_add(7 * 24 * 60 * 60 * 1000 + 1, Ordering::SeqCst);
    assert_eq!(store.purge_expired().unwrap(), 1);
    assert!(store.item(&item.id).unwrap().is_none());
    assert!(store.search("编码", 10).unwrap().is_empty());
}

#[test]
fn forget_clears_text_and_blocks_same_source() {
    let (store, _dir, _) = open();
    let item = note(&store, "GBK CSV", "file:2");
    store
        .write(
            WriteOp::Forget {
                id: item.id.clone(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    let forgotten = store.item(&item.id).unwrap().unwrap();
    assert_eq!(forgotten.status, "forgotten");
    assert!(forgotten.title.is_empty() && forgotten.body.is_empty());
    assert!(store.search("编码", 10).unwrap().is_empty());

    let same = store.write(
        WriteOp::Note {
            title: "再记一次".to_owned(),
            summary: String::new(),
            body: "GBK".to_owned(),
            source_kind: "kind".to_owned(),
            source_ref: "file:2".to_owned(),
        },
        &WriteContext::kind(),
    );
    assert!(matches!(same, Err(MemoryError::Refused(_))));
}

#[test]
fn chinese_short_and_trigram_search_both_hit() {
    let (store, _dir, _) = open();
    let item = note(&store, "GBK CSV", "file:3");
    let short = store.search("编码", 10).unwrap();
    assert!(matches!(short.first(), Some(SearchHit::Memory(hit)) if hit.id == item.id));
    let trigram = store.search("探编码", 10).unwrap();
    assert!(matches!(trigram.first(), Some(SearchHit::Memory(hit)) if hit.id == item.id));

    store
        .write(
            WriteOp::IndexConversation {
                conversation_id: "conv-1".to_owned(),
                seq: 7,
                turn: 2,
                text: "上次我们讨论方案时说过编码先看。".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    let hit = store.search("讨论方案", 10).unwrap();
    assert!(hit.iter().any(|hit| matches!(hit, SearchHit::Conversation { conversation_id, seq, .. } if conversation_id == "conv-1" && *seq == 7)));
    // 同一 seq 重放不产生第二条。
    store
        .write(
            WriteOp::IndexConversation {
                conversation_id: "conv-1".to_owned(),
                seq: 7,
                turn: 2,
                text: "上次我们讨论方案时说过编码先看。".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    let hits = store.search("讨论方案", 10).unwrap();
    assert_eq!(hits.iter().filter(|hit| matches!(hit, SearchHit::Conversation { conversation_id, .. } if conversation_id == "conv-1")).count(), 1);
}

#[test]
fn dream_can_apply_notes_and_undo_skips_user_edits() {
    let (store, _dir, _) = open();
    let item = note(&store, "GBK CSV", "file:4");
    let run = DreamRunInput {
        id: "run-1".to_owned(),
        model: "test".to_owned(),
        started_at: 1,
        ended_at: 2,
        outcome: "done".to_owned(),
        report_json: "{}".to_owned(),
        usage_json: "{}".to_owned(),
    };
    let applied = store
        .write(
            WriteOp::DreamApply {
                run: run.clone(),
                changes: vec![DreamChangeInput {
                    memory_id: Some(item.id.clone()),
                    title: "GBK CSV（已合）".to_owned(),
                    summary: "合并后的摘要".to_owned(),
                    body: "sources/ 下有 GBK 的 CSV；先看编码。".to_owned(),
                    action: DreamAction::Merge,
                    reason: "合并重复笔记".to_owned(),
                }],
            },
            &WriteContext::dream("run-1"),
        )
        .unwrap();
    assert_eq!(applied.dream_applied, 1);
    assert_eq!(store.item(&item.id).unwrap().unwrap().current_revision, 2);

    // 用户改过：整次撤销应跳过，不覆盖用户的话。
    store
        .write(
            WriteOp::Correct {
                id: item.id.clone(),
                title: "用户改过的".to_owned(),
                summary: "用户摘要".to_owned(),
                body: "用户改过的正文".to_owned(),
                reason: "user".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    let undo = store
        .write(
            WriteOp::DreamUndo {
                run_id: "run-1".to_owned(),
                only: None,
            },
            &WriteContext::dream("run-1"),
        )
        .unwrap();
    assert_eq!(undo.dream_applied, 0);
    assert_eq!(undo.dream_skipped, vec![item.id.clone()]);
    assert_eq!(store.item(&item.id).unwrap().unwrap().title, "用户改过的");

    // 没有用户改动时，撤销回到做梦前的正文。
    let second = note(&store, "另一条", "file:5");
    store
        .write(
            WriteOp::DreamApply {
                run: DreamRunInput {
                    id: "run-2".to_owned(),
                    ..run
                },
                changes: vec![DreamChangeInput {
                    memory_id: Some(second.id.clone()),
                    title: "改过".to_owned(),
                    summary: "改".to_owned(),
                    body: "改过正文".to_owned(),
                    action: DreamAction::Correct,
                    reason: "dream".to_owned(),
                }],
            },
            &WriteContext::dream("run-2"),
        )
        .unwrap();
    let undo = store
        .write(
            WriteOp::DreamUndo {
                run_id: "run-2".to_owned(),
                only: None,
            },
            &WriteContext::dream("run-2"),
        )
        .unwrap();
    assert_eq!(undo.dream_applied, 1);
    assert_eq!(
        store.item(&second.id).unwrap().unwrap().body,
        "sources/ 下有 GBK 的 CSV；读之前先探编码。"
    );
}

#[test]
fn diary_search_hits_short_and_trigram_queries() {
    let (store, _dir, _) = open();
    store
        .write(
            WriteOp::Diary {
                text: "今天整理了编码问题，读文件前先探编码。".to_owned(),
                source_ref: "diary:1".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap();
    let short = store.search("编码", 10).unwrap();
    assert!(
        short
            .iter()
            .any(|hit| matches!(hit, SearchHit::Diary(entry) if entry.text.contains("编码")))
    );
    let trigram = store.search("编码问", 10).unwrap();
    assert!(trigram.iter().any(|hit| matches!(hit, SearchHit::Diary(_))));
}

#[test]
fn pin_keeps_note_in_view_and_creates_binding_entry() {
    let (store, _dir, _) = open();
    let item = note(&store, "GBK CSV", "file:pin");
    let pinned = store
        .write(
            WriteOp::Pin {
                id: item.id.clone(),
            },
            &WriteContext::user(),
        )
        .unwrap()
        .item
        .unwrap();
    assert_eq!(pinned.layer, Layer::Entry);
    assert_eq!(pinned.binding, Binding::Binding);
    let view = store.view().unwrap();
    assert!(
        view.notes
            .iter()
            .any(|note| note.id == item.id && note.status == "pinned" && note.pinned)
    );
    assert!(view.entries.iter().any(|entry| entry.id == pinned.id));
}

#[test]
fn dream_cannot_touch_about_you_or_entry() {
    let (store, _dir, _) = open();
    let about = remember(&store, "关于你");
    let note = note(&store, "笔记", "file:d");
    let entry = store
        .write(
            WriteOp::Pin {
                id: note.id.clone(),
            },
            &WriteContext::user(),
        )
        .unwrap()
        .item
        .unwrap();
    let run = DreamRunInput {
        id: "run-guard".to_owned(),
        model: "test".to_owned(),
        started_at: 1,
        ended_at: 2,
        outcome: "done".to_owned(),
        report_json: "{}".to_owned(),
        usage_json: "{}".to_owned(),
    };
    for target in [&about.id, &entry.id] {
        let result = store.write(
            WriteOp::DreamApply {
                run: run.clone(),
                changes: vec![DreamChangeInput {
                    memory_id: Some(target.clone()),
                    title: "改".to_owned(),
                    summary: "改".to_owned(),
                    body: "改".to_owned(),
                    action: DreamAction::Correct,
                    reason: "dream".to_owned(),
                }],
            },
            &WriteContext::dream("run-guard"),
        );
        assert!(matches!(result, Err(MemoryError::Refused(_))));
    }
    assert_eq!(store.item(&about.id).unwrap().unwrap().title, "关于你");
    assert_eq!(store.item(&entry.id).unwrap().unwrap().title, "笔记");
}

#[test]
fn dream_run_id_and_note_targets_are_checked() {
    let (store, _dir, _) = open();
    let item = note(&store, "笔记", "file:run");
    let run = DreamRunInput {
        id: "run-check".to_owned(),
        model: "test".to_owned(),
        started_at: 1,
        ended_at: 2,
        outcome: "done".to_owned(),
        report_json: "{}".to_owned(),
        usage_json: "{}".to_owned(),
    };
    let mismatch = store.write(
        WriteOp::DreamApply {
            run: run.clone(),
            changes: vec![DreamChangeInput {
                memory_id: Some(item.id.clone()),
                title: "改".to_owned(),
                summary: "改".to_owned(),
                body: "改".to_owned(),
                action: DreamAction::Correct,
                reason: "dream".to_owned(),
            }],
        },
        &WriteContext::dream("other-run"),
    );
    assert!(matches!(mismatch, Err(MemoryError::Refused(_))));
    assert_eq!(store.item(&item.id).unwrap().unwrap().title, "笔记");

    let undo_mismatch = store.write(
        WriteOp::DreamUndo {
            run_id: "run-check".to_owned(),
            only: None,
        },
        &WriteContext::dream("other-run"),
    );
    assert!(matches!(undo_mismatch, Err(MemoryError::Refused(_))));
}

#[test]
fn schema_version_is_two_after_actor_column() {
    let (store, dir, _) = open();
    drop(store);
    let conn = rusqlite::Connection::open(dir.path().join("memory.sqlite")).unwrap();
    let value: String = conn
        .query_row(
            "SELECT value FROM kv WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, "2");
}

#[test]
fn kind_can_write_note_diary_and_index_and_dream_can_apply() {
    let (store, _dir, _) = open();
    note(&store, "Kind 笔记", "file:actor");
    store
        .write(
            WriteOp::Diary {
                text: "日记".to_owned(),
                source_ref: "diary:actor".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap();
    store
        .write(
            WriteOp::IndexConversation {
                conversation_id: "conv".to_owned(),
                seq: 1,
                turn: 1,
                text: "索引".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap();
    let item = store
        .view()
        .unwrap()
        .notes
        .into_iter()
        .find(|item| item.title == "Kind 笔记")
        .unwrap();
    let run = DreamRunInput {
        id: "run-apply".to_owned(),
        model: "test".to_owned(),
        started_at: 1,
        ended_at: 2,
        outcome: "done".to_owned(),
        report_json: "{}".to_owned(),
        usage_json: "{}".to_owned(),
    };
    let applied = store.write(
        WriteOp::DreamApply {
            run,
            changes: vec![DreamChangeInput {
                memory_id: Some(item.id),
                title: "改".to_owned(),
                summary: "改".to_owned(),
                body: "改".to_owned(),
                action: DreamAction::Correct,
                reason: "dream".to_owned(),
            }],
        },
        &WriteContext::dream("run-apply"),
    );
    assert!(applied.is_ok());
}

#[test]
fn dream_undo_only_one_change() {
    let (store, _dir, _) = open();
    let first = note(&store, "第一条", "file:u1");
    let second = note(&store, "第二条", "file:u2");
    let run = DreamRunInput {
        id: "run-only".to_owned(),
        model: "test".to_owned(),
        started_at: 1,
        ended_at: 2,
        outcome: "done".to_owned(),
        report_json: "{}".to_owned(),
        usage_json: "{}".to_owned(),
    };
    store
        .write(
            WriteOp::DreamApply {
                run,
                changes: vec![
                    DreamChangeInput {
                        memory_id: Some(first.id.clone()),
                        title: "改了第一条".to_owned(),
                        summary: "改".to_owned(),
                        body: "改".to_owned(),
                        action: DreamAction::Correct,
                        reason: "dream".to_owned(),
                    },
                    DreamChangeInput {
                        memory_id: Some(second.id.clone()),
                        title: "改了第二条".to_owned(),
                        summary: "改".to_owned(),
                        body: "改".to_owned(),
                        action: DreamAction::Correct,
                        reason: "dream".to_owned(),
                    },
                ],
            },
            &WriteContext::dream("run-only"),
        )
        .unwrap();
    let undo = store
        .write(
            WriteOp::DreamUndo {
                run_id: "run-only".to_owned(),
                only: Some(first.id.clone()),
            },
            &WriteContext::dream("run-only"),
        )
        .unwrap();
    assert_eq!(undo.dream_applied, 1);
    assert_eq!(store.item(&first.id).unwrap().unwrap().title, "第一条");
    assert_eq!(store.item(&second.id).unwrap().unwrap().title, "改了第二条");
}

#[test]
fn dream_settings_default_and_updates() {
    let (store, _dir, _) = open();
    let defaults = store.dream_settings().unwrap();
    assert!(defaults.enabled, "做梦默认开着");
    assert!(!defaults.permit, "观察许可默认没有");
    assert_eq!(defaults.interval_hours, 24);
    assert_eq!(defaults.min_events, 5);
    assert_eq!(defaults.last_success_at, 0);
    assert_eq!(defaults.last_read_seq, 0);

    let before = store.version().unwrap();
    let updated = store
        .update_dream_settings(&memory::types::DreamSettingsPatch {
            permit: Some(true),
            interval_hours: Some(12),
            last_success_at: Some(1234),
            ..Default::default()
        })
        .unwrap();
    assert!(updated.permit);
    assert_eq!(updated.interval_hours, 12);
    assert_eq!(updated.last_success_at, 1234);
    assert_eq!(updated.min_events, 5, "没给的项不动");
    assert!(store.version().unwrap() > before, "设置写入进版本");
    let again = store.dream_settings().unwrap();
    assert!(again.permit);
    assert_eq!(again.interval_hours, 12);
}

fn dream_apply_note(store: &MemoryStore, run_id: &str) -> memory::types::MemoryItem {
    store
        .write(
            WriteOp::DreamApply {
                run: DreamRunInput {
                    id: run_id.to_owned(),
                    model: "deepseek-flash".to_owned(),
                    started_at: 10,
                    ended_at: 20,
                    outcome: "running".to_owned(),
                    report_json: "{}".to_owned(),
                    usage_json: "{}".to_owned(),
                },
                changes: vec![DreamChangeInput {
                    memory_id: None,
                    title: "做梦新记".to_owned(),
                    summary: "整理".to_owned(),
                    body: "把两处笔记合并成这一条".to_owned(),
                    action: DreamAction::NewNote,
                    reason: "重复".to_owned(),
                }],
            },
            &WriteContext::dream(run_id),
        )
        .unwrap()
        .item
        .unwrap()
}

#[test]
fn dream_run_is_finished_and_read_back() {
    let (store, _dir, _) = open();
    let note = dream_apply_note(&store, "run-read");
    store
        .finish_dream_run(
            "run-read",
            30,
            "done",
            r#"{"applied":1,"summary":"合并了一处"}"#,
            r#"{"inputTokens":10,"outputTokens":2}"#,
            "",
        )
        .unwrap();
    let run = store.dream_run("run-read").unwrap().expect("能按 id 读");
    assert_eq!(run.outcome, "done");
    assert_eq!(run.ended_at, 30);
    assert!(run.report_json.contains("合并了一处"));
    assert!(run.usage_json.contains("outputTokens"));
    assert_eq!(
        store
            .latest_dream_run()
            .unwrap()
            .map(|run| run.id)
            .as_deref(),
        Some("run-read")
    );
    let changes = store.dream_changes("run-read").unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].memory_id, note.id);
    assert_eq!(changes[0].after_revision, 1);
    assert_eq!(changes[0].action, "new_note");
    assert!(
        store
            .finish_dream_run("missing", 1, "done", "{}", "{}", "")
            .is_err()
    );
}

#[test]
fn user_can_undo_dream_by_run_without_dream_context() {
    let (store, _dir, _) = open();
    let note = dream_apply_note(&store, "run-user-undo");
    let undo = store
        .write(
            WriteOp::DreamUndo {
                run_id: "run-user-undo".to_owned(),
                only: None,
            },
            &WriteContext::user(),
        )
        .unwrap();
    assert_eq!(undo.dream_applied, 1, "用户能从页面整次撤销");
    assert!(store.item(&note.id).unwrap().is_none(), "新记的笔记被撤掉");
}

#[test]
fn revisions_record_who_changed_them() {
    let (store, _dir, _) = open();
    let note = store
        .write(
            WriteOp::Note {
                title: "先探编码".to_owned(),
                summary: String::new(),
                body: "读之前先探编码".to_owned(),
                source_kind: "kind".to_owned(),
                source_ref: "kind:1".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap()
        .item
        .unwrap();
    assert_eq!(store.revisions(&note.id).unwrap()[0].actor, "kind");

    store
        .write(
            WriteOp::Correct {
                id: note.id.clone(),
                title: "先探编码".to_owned(),
                summary: String::new(),
                body: "读之前先探编码，GBK 也要先转".to_owned(),
                reason: "user".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    assert_eq!(
        store.revisions(&note.id).unwrap().last().unwrap().actor,
        "user"
    );

    store
        .write(
            WriteOp::DreamApply {
                run: DreamRunInput {
                    id: "run-actor".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    started_at: 1,
                    ended_at: 2,
                    outcome: "running".to_owned(),
                    report_json: "{}".to_owned(),
                    usage_json: "{}".to_owned(),
                },
                changes: vec![DreamChangeInput {
                    memory_id: Some(note.id.clone()),
                    title: "先探编码".to_owned(),
                    summary: String::new(),
                    body: "读之前先探编码，GBK 也要先转，再核一遍".to_owned(),
                    action: DreamAction::Correct,
                    reason: "dream".to_owned(),
                }],
            },
            &WriteContext::dream("run-actor"),
        )
        .unwrap();
    assert_eq!(
        store.revisions(&note.id).unwrap().last().unwrap().actor,
        "dream"
    );
}
