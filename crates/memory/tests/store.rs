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
