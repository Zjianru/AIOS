use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use memory::types::{
    Binding, DreamAction, DreamChangeInput, DreamRunInput, Layer, Scope, SearchHit, WriteContext,
    WriteOp,
};
use memory::{MemoryError, MemoryStore};
use rusqlite::OptionalExtension;
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
fn schema_version_is_three_after_schema_migration() {
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
    assert_eq!(value, "3");
    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(user_version, 3);
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

fn v2_database(dir: &TempDir) -> std::path::PathBuf {
    let path = dir.path().join("memory.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE memory (
             id TEXT PRIMARY KEY, layer TEXT NOT NULL, binding TEXT NOT NULL,
             scope TEXT NOT NULL, project_id TEXT, role_id TEXT,
             title TEXT NOT NULL, summary TEXT NOT NULL, body TEXT NOT NULL,
             source_kind TEXT NOT NULL, source_ref TEXT NOT NULL,
             created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
             deleted_at INTEGER, purge_after INTEGER, status TEXT NOT NULL,
             current_revision INTEGER NOT NULL, pinned INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE memory_revision (
             id INTEGER PRIMARY KEY AUTOINCREMENT, memory_id TEXT NOT NULL,
             revision INTEGER NOT NULL, title TEXT NOT NULL, summary TEXT NOT NULL,
             body TEXT NOT NULL, source_kind TEXT NOT NULL, source_ref TEXT NOT NULL,
             reason TEXT NOT NULL, dream_run_id TEXT, undo_of INTEGER,
             actor TEXT NOT NULL DEFAULT 'system', created_at INTEGER NOT NULL,
             UNIQUE(memory_id, revision)
         );
         CREATE TABLE diary_entry (
             id TEXT PRIMARY KEY, day TEXT NOT NULL, seq INTEGER NOT NULL,
             text TEXT NOT NULL, source_ref TEXT NOT NULL, created_at INTEGER NOT NULL
         );
         CREATE VIRTUAL TABLE memory_fts USING fts5(
             kind, memory_id UNINDEXED, title, body, source_json UNINDEXED,
             tokenize='trigram'
         );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kv (key, value) VALUES ('schema_version', '2'), ('version', '0')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory (id, layer, binding, scope, title, summary, body, source_kind, source_ref, created_at, updated_at, status, current_revision, pinned)
         VALUES ('mem-old', 'about_you', 'binding', 'personal', '旧标题', '旧摘要', '旧正文 old-body', 'user', 'old:1', 111, 111, 'active', 1, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_revision (memory_id, revision, title, summary, body, source_kind, source_ref, reason, dream_run_id, actor, created_at)
         VALUES ('mem-old', 1, '旧标题', '旧摘要', '旧正文 old-body', 'user', 'old:1', 'created', NULL, 'system', 111)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO diary_entry (id, day, seq, text, source_ref, created_at)
         VALUES ('diary-old', '1970-01-01', 1, '旧日记', 'diary:old', 111)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_fts (kind, memory_id, title, body, source_json)
         VALUES ('about_you', 'mem-old', '旧标题', '旧正文 old-body', '{}')",
        [],
    )
    .unwrap();
    path
}

fn table_columns(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn v2_migrates_columns_sources_and_fts_with_backup_report() {
    let dir = tempfile::tempdir().unwrap();
    let path = v2_database(&dir);
    let store = MemoryStore::open(&path).unwrap();
    let report = store.migration_report().expect("迁移报告");
    assert_eq!((report.from_version, report.to_version), (2, 3));
    assert_eq!(report.missing_original, 1, "旧行没有原文，报告要写行数");
    let backup = report.backup.clone().expect("备份路径");
    assert!(backup.ends_with("memory.db.v2.bak"), "{backup}");
    assert!(std::path::Path::new(&backup).exists(), "迁移前备份要在");
    assert!(store.item("mem-old").unwrap().is_some(), "旧条目还在");
    assert!(store.keys("mem-old").unwrap().is_empty(), "旧数据没有钥匙");
    assert!(
        store.sources("mem-old").unwrap().is_empty(),
        "旧数据不插 source 行"
    );
    assert!(
        store
            .search("旧标题", 10)
            .unwrap()
            .iter()
            .any(|hit| matches!(hit, SearchHit::Memory(found) if found.id == "mem-old")),
        "FTS 重建后旧标题还能搜到"
    );
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(user_version, 3);
    for column in [
        "valid_from",
        "valid_until",
        "invalid_reason",
        "superseded_by",
        "possibly_stale",
    ] {
        assert!(
            table_columns(&conn, "memory").contains(&column.to_owned()),
            "{column}"
        );
        assert!(
            table_columns(&conn, "memory_revision").contains(&column.to_owned()),
            "{column} revision"
        );
    }
    for table in [
        "memory_key",
        "memory_source",
        "memory_relation",
        "memory_delivery",
        "memory_card",
    ] {
        let exists: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert!(exists.is_some(), "{table} 要建出来");
    }
    assert!(table_columns(&conn, "memory_fts").contains(&"keys".to_owned()));
    assert_eq!(
        table_columns(&conn, "memory_card"),
        vec!["card_ref", "kind", "status", "payload_json"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );
    let valid_from: i64 = conn
        .query_row(
            "SELECT valid_from FROM memory WHERE id = 'mem-old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(valid_from, 111, "valid_from=created_at");
    let revision_valid_from: i64 = conn
        .query_row(
            "SELECT valid_from FROM memory_revision WHERE memory_id = 'mem-old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revision_valid_from, 111);
    let nulls: (Option<i64>, Option<String>, Option<String>, i64) = conn
        .query_row(
            "SELECT valid_until, invalid_reason, superseded_by, possibly_stale FROM memory WHERE id = 'mem-old'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(nulls, (None, None, None, 0));
    let key_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_key", [], |row| row.get(0))
        .unwrap();
    let source_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_source", [], |row| row.get(0))
        .unwrap();
    assert_eq!((key_rows, source_rows), (0, 0));

    // 二次迁移幂等：不重复写、不再出报告、备份不丢。
    let again = MemoryStore::open(&path).unwrap();
    assert!(again.migration_report().is_none());
    assert!(std::path::Path::new(&backup).exists());
    assert_eq!(again.item("mem-old").unwrap().unwrap().title, "旧标题");
    assert!(again.keys("mem-old").unwrap().is_empty());
}

#[test]
fn failed_migration_rolls_back_to_v2_and_leaves_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = v2_database(&dir);
    let result = MemoryStore::open_with_failed_migration_for_test(&path);
    assert!(
        matches!(result, Err(MemoryError::Migration(_))),
        "应该报迁移失败"
    );
    let conn = rusqlite::Connection::open(&path).unwrap();
    let schema: String = conn
        .query_row(
            "SELECT value FROM kv WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(schema, "2", "失败整事务回滚，库仍 v2");
    assert!(!table_columns(&conn, "memory").contains(&"valid_from".to_owned()));
    assert!(
        dir.path().join("memory.db.v2.bak").exists(),
        "失败也留着备份"
    );
    drop(conn);
    // 不自动恢复：下一次正常打开走真实迁移，而不是拿备份覆盖。
    let store = MemoryStore::open(&path).unwrap();
    assert_eq!(store.migration_report().unwrap().from_version, 2);
    assert_eq!(store.item("mem-old").unwrap().unwrap().title, "旧标题");
}

#[test]
fn keys_are_written_removed_and_searched_from_fts() {
    let (store, _dir, _) = open();
    let item = remember(&store, "回答先给结论");
    assert!(store.keys(&item.id).unwrap().is_empty(), "不从标题造钥匙");
    store
        .set_keys(
            &item.id,
            &[(
                memory::types::MemoryKeyKind::Keyword,
                "zzunique-key".to_owned(),
            )],
        )
        .unwrap();
    let keys = store.keys(&item.id).unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].value, "zzunique-key");
    let item_id = item.id.clone();
    let hits = store.search("zzunique-key", 10).unwrap();
    assert!(
        hits.iter()
            .any(|hit| matches!(hit, SearchHit::Memory(found) if found.id == item_id)),
        "{hits:?}"
    );
    store
        .set_keys(
            &item.id,
            &[(
                memory::types::MemoryKeyKind::Keyword,
                "other-key".to_owned(),
            )],
        )
        .unwrap();
    assert!(
        store.search("zzunique-key", 10).unwrap().is_empty(),
        "旧钥匙同事务删掉"
    );
    assert!(
        store
            .search("other-key", 10)
            .unwrap()
            .iter()
            .any(|hit| matches!(hit, SearchHit::Memory(found) if found.id == item.id)),
        "新钥匙要能搜到"
    );
    store.set_keys(&item.id, &[]).unwrap();
    assert!(store.keys(&item.id).unwrap().is_empty());
    assert!(store.search("other-key", 10).unwrap().is_empty());
}

#[test]
fn sources_are_explicit_and_title_does_not_create_keys() {
    let (store, _dir, _) = open();
    let item = remember(&store, "回答先给结论");
    assert!(
        store.search("回答先给结论", 10).unwrap().is_empty()
            || !store
                .keys(&item.id)
                .unwrap()
                .iter()
                .any(|key| key.value.contains("回答")),
        "不从标题造钥匙"
    );
    store
        .add_source(&item.id, "message", "root:3", Some("原话：回答先给结论"))
        .unwrap();
    let sources = store.sources(&item.id).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_ref, "root:3");
    assert_eq!(sources[0].quote.as_deref(), Some("原话：回答先给结论"));
}
