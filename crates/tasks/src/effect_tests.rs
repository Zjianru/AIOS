//! 效果表：同键合并、代次、Later、退避和健康错误；时间全部注入。

use super::Store;
use crate::{EffectClaim, EffectHealth, EffectKind, EffectPayload, EffectState, ReportId, TaskId};

const T0: i64 = 1_700_000_000_000;

fn open_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn launch(id: &str) -> EffectPayload {
    EffectPayload::Launch {
        task_id: TaskId::from(id),
    }
}

fn claim(store: &Store, kind: EffectKind, now: i64) -> EffectClaim {
    store
        .claim_effect(kind, now)
        .unwrap()
        .expect("效果应该可以领")
}

fn ready_health(waiting: u32) -> EffectHealth {
    EffectHealth {
        state: EffectState::Ready,
        waiting,
        attempts: 0,
        since: if waiting == 0 { None } else { Some(T0) },
        last_error: None,
    }
}

#[test]
fn merges_the_same_key_and_keeps_its_queue_position() {
    let (_directory, store) = open_store();
    store.enqueue_effect(launch("a"), T0).unwrap();
    store.enqueue_effect(launch("b"), T0).unwrap();
    store.enqueue_effect(launch("a"), T0 + 1).unwrap();

    let first = claim(&store, EffectKind::Launch, T0 + 1);
    assert_eq!(first.key, "a");
    assert_eq!(first.seq, 1, "同一个键重排要保留原来的队列位置");
    assert_eq!(first.generation, 1);
    assert_eq!(first.since, T0, "since 是第一次入队的时刻");
    assert!(store.complete_effect(&first).unwrap());

    let second = claim(&store, EffectKind::Launch, T0 + 1);
    assert_eq!(second.key, "b");
}

#[test]
fn a_new_payload_resets_failures_and_blocks_the_old_claim() {
    let (_directory, store) = open_store();
    store.enqueue_effect(launch("a"), T0).unwrap();
    let stale = claim(&store, EffectKind::Launch, T0);
    assert!(store.fail_effect(&stale, "第一次失败", T0).unwrap());
    store.enqueue_effect(launch("a"), T0 + 1).unwrap();

    assert!(!store.complete_effect(&stale).unwrap());
    assert!(!store.fail_effect(&stale, "迟到的失败", T0 + 2).unwrap());
    assert!(!store.defer_effect(&stale, T0 + 2).unwrap());

    let fresh = claim(&store, EffectKind::Launch, T0 + 1);
    assert_eq!(fresh.generation, 1);
    assert_eq!(fresh.attempts, 0, "新载荷把连续失败清零");
    assert_eq!(fresh.payload, launch("a"));
    assert!(store.complete_effect(&fresh).unwrap());
    assert!(!store.effect_pending(EffectKind::Launch, "a").unwrap());
}

#[test]
fn later_reschedules_without_counting_a_failure() {
    let (_directory, store) = open_store();
    store.enqueue_effect(launch("a"), T0).unwrap();
    let first = claim(&store, EffectKind::Launch, T0);

    assert!(store.defer_effect(&first, T0).unwrap());
    assert_eq!(
        store.effect_health(EffectKind::Launch).unwrap(),
        ready_health(1)
    );
    assert_eq!(
        store.claim_effect(EffectKind::Launch, T0 + 4_999).unwrap(),
        None
    );

    let next = claim(&store, EffectKind::Launch, T0 + 5_000);
    assert_eq!(next.attempts, 0, "Later 不计失败");
    assert_eq!(next.generation, 0);
    assert_eq!(next.due, T0 + 5_000);
    assert!(store.complete_effect(&next).unwrap());
}

#[test]
fn three_failures_report_an_effect_error() {
    let (_directory, store) = open_store();
    store.enqueue_effect(launch("a"), T0).unwrap();
    let mut now = T0;
    for attempt in 0..3 {
        let claim = claim(&store, EffectKind::Launch, now);
        assert!(store.fail_effect(&claim, "boom", now).unwrap());
        now += 5_000 * (1_i64 << attempt);
    }

    assert_eq!(
        store.effect_health(EffectKind::Launch).unwrap(),
        EffectHealth {
            state: EffectState::Error,
            waiting: 1,
            attempts: 3,
            since: Some(T0),
            last_error: Some("boom".to_owned()),
        }
    );

    let claim = claim(&store, EffectKind::Launch, now);
    assert!(store.complete_effect(&claim).unwrap());
    assert_eq!(
        store.effect_health(EffectKind::Launch).unwrap(),
        ready_health(0)
    );
}

#[test]
fn relay_uses_its_own_first_retry_delay() {
    let (_directory, store) = open_store();
    store
        .enqueue_effect(
            EffectPayload::Relay {
                task_id: TaskId::from("task-1"),
                report_id: ReportId::from("report-1"),
            },
            T0,
        )
        .unwrap();
    let first = claim(&store, EffectKind::Relay, T0);
    assert!(store.fail_effect(&first, "boom", T0).unwrap());

    assert_eq!(
        store.claim_effect(EffectKind::Relay, T0 + 1_999).unwrap(),
        None
    );
    let retry = claim(&store, EffectKind::Relay, T0 + 2_000);
    assert_eq!(retry.attempts, 1);
    assert_eq!(retry.due, T0 + 2_000);
}

#[test]
fn backoff_doubles_and_caps_at_the_kind_maximum() {
    let (_directory, store) = open_store();
    store.enqueue_effect(launch("a"), T0).unwrap();
    let delays = [
        5_000_i64, 10_000, 20_000, 40_000, 80_000, 160_000, 300_000, 300_000,
    ];
    let mut now = T0;
    for delay in delays {
        let claim = claim(&store, EffectKind::Launch, now);
        assert!(store.fail_effect(&claim, "boom", now).unwrap());
        assert_eq!(
            store
                .claim_effect(EffectKind::Launch, now + delay - 1)
                .unwrap(),
            None,
            "退避没到不能领"
        );
        now += delay;
        assert!(
            store
                .claim_effect(EffectKind::Launch, now)
                .unwrap()
                .is_some()
        );
    }
    assert_eq!(
        store.effect_health(EffectKind::Launch).unwrap().attempts,
        delays.len() as u32
    );
}

#[test]
fn queued_effects_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    {
        let store = Store::open(&path).unwrap();
        store
            .enqueue_effect(
                EffectPayload::Relay {
                    task_id: TaskId::from("task-1"),
                    report_id: ReportId::from("report-1"),
                },
                T0,
            )
            .unwrap();
    }

    let store = Store::open(&path).unwrap();
    let claim = claim(&store, EffectKind::Relay, T0);
    assert_eq!(claim.key, "report-1");
    assert!(store.complete_effect(&claim).unwrap());
    assert!(!store.effect_pending(EffectKind::Relay, "report-1").unwrap());
}

#[test]
fn health_is_ready_with_nothing_waiting() {
    let (_directory, store) = open_store();
    assert_eq!(
        store.effect_health(EffectKind::Relay).unwrap(),
        ready_health(0)
    );
}
