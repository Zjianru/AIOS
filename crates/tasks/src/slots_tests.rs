//! 席位计数：在跑集合 + 起跑预约、person 的额外 1 个、满了 Later。

use super::{Later, SlotKind, WorkerSlots};

fn running(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_owned()).collect()
}

#[test]
fn a_launch_gets_later_when_every_seat_is_taken() {
    let mut slots = WorkerSlots::new();
    let running = running(&["a"]);

    assert!(matches!(
        slots.reserve("b", SlotKind::Launch, &running, 1),
        Err(Later)
    ));
    assert_eq!(slots.used(&running), 1);
}

#[test]
fn a_repeated_reservation_keeps_its_kind_and_seat() {
    let mut slots = WorkerSlots::new();
    let none = running(&[]);

    slots.reserve("a", SlotKind::Launch, &none, 2).unwrap();
    slots.reserve("a", SlotKind::Wake, &none, 2).unwrap();

    assert_eq!(slots.used(&none), 1, "同一个键只占一个席位");
    assert_eq!(
        slots.launches(&none, 2),
        2,
        "重复预约不把 launch 改成 wake，容量不变"
    );
    slots.free("a");
    assert_eq!(slots.used(&none), 0);
}

#[test]
fn person_may_use_exactly_one_extra_seat() {
    let mut slots = WorkerSlots::new();
    let running = running(&["a"]);

    slots.reserve("b", SlotKind::Person, &running, 1).unwrap();
    assert_eq!(slots.used(&running), 2, "person 可以超出上限 1 个");
    assert!(matches!(
        slots.reserve("c", SlotKind::Person, &running, 1),
        Err(Later)
    ));

    slots.free("b");
    slots.reserve("c", SlotKind::Person, &running, 1).unwrap();
}

#[test]
fn launch_capacity_excludes_running_and_wakes_but_not_launches_in_flight() {
    let mut slots = WorkerSlots::new();
    let running = running(&["r"]);

    slots.reserve("w", SlotKind::Wake, &running, 3).unwrap();
    slots.reserve("l", SlotKind::Launch, &running, 3).unwrap();

    assert_eq!(slots.used(&running), 3);
    assert_eq!(
        slots.launches(&running, 3),
        1,
        "在跑和唤醒预约各扣一个，在飞的 launch 不重复扣"
    );
    assert!(matches!(
        slots.reserve("x", SlotKind::Launch, &running, 3),
        Err(Later)
    ));

    slots.free("w");
    assert_eq!(slots.launches(&running, 3), 2);
    slots.reserve("x", SlotKind::Launch, &running, 3).unwrap();
}

#[test]
fn a_released_seat_admits_the_next_reservation_in_call_order() {
    let mut slots = WorkerSlots::new();
    let none = running(&[]);

    slots.reserve("first", SlotKind::Launch, &none, 1).unwrap();
    assert!(matches!(
        slots.reserve("second", SlotKind::Launch, &none, 1),
        Err(Later)
    ));
    assert!(matches!(
        slots.reserve("third", SlotKind::Launch, &none, 1),
        Err(Later)
    ));

    slots.free("first");
    slots.reserve("second", SlotKind::Launch, &none, 1).unwrap();
    assert!(matches!(
        slots.reserve("third", SlotKind::Launch, &none, 1),
        Err(Later)
    ));

    slots.free("second");
    slots.reserve("third", SlotKind::Launch, &none, 1).unwrap();
    assert_eq!(slots.used(&none), 1);
}

#[test]
fn running_tasks_hold_seats_and_are_counted_once() {
    let slots = WorkerSlots::new();
    let running = running(&["a", "a", "b"]);

    assert_eq!(slots.used(&running), 2, "同一个任务只算一个席位");
    assert_eq!(slots.launches(&running, 3), 1);
    assert_eq!(
        slots.launches(&running, 1),
        0,
        "上限比在跑的少时按 0 算，不出现负数"
    );
}

#[test]
fn freeing_an_unknown_key_changes_nothing() {
    let mut slots = WorkerSlots::new();
    let none = running(&[]);
    slots.reserve("a", SlotKind::Launch, &none, 1).unwrap();

    slots.free("nobody");
    assert_eq!(slots.used(&none), 1);
    slots.free("a");
    assert_eq!(slots.used(&none), 0);
}
