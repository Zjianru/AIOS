use crate::events::page_events;

fn events(count: u64) -> Vec<session::Event> {
    (0..count)
        .map(|seq| {
            session::Event::new(
                seq,
                1_000 + seq,
                session::EventKind::UserMessage {
                    turn: seq / 2 + 1,
                    step: 1,
                    message: model::Message::text(model::Role::User, "m"),
                },
            )
        })
        .collect()
}

fn seqs(page: &[&session::Event]) -> Vec<u64> {
    page.iter().map(|event| event.seq).collect()
}

#[test]
fn before_seq_is_exclusive_upper_bound() {
    let all = events(10);
    let (page, has_more) = page_events(&all, Some(5), 100, None, None);
    assert_eq!(seqs(&page), vec![0, 1, 2, 3, 4], "beforeSeq 不含本条");
    assert!(!has_more);
    let (page, _) = page_events(&all, Some(0), 100, None, None);
    assert!(page.is_empty(), "beforeSeq=0 没有任何更早的事件");
    let (page, _) = page_events(&all, Some(11), 100, None, None);
    assert_eq!(
        seqs(&page),
        (0..10).collect::<Vec<_>>(),
        "beforeSeq=N+1 等于全部"
    );
    let (page, _) = page_events(&all, None, 100, None, None);
    assert_eq!(
        seqs(&page),
        (0..10).collect::<Vec<_>>(),
        "省略 beforeSeq 等于全部"
    );
}

#[test]
fn paging_backwards_yields_each_seq_exactly_once() {
    let all = events(10);
    for page_size in [2_usize, 3] {
        let mut cursor: Option<u64> = None;
        let mut collected: Vec<u64> = Vec::new();
        for _ in 0..100 {
            let (page, has_more) = page_events(&all, cursor, page_size, None, None);
            if page.is_empty() {
                break;
            }
            collected.extend(seqs(&page));
            cursor = page.first().map(|event| event.seq);
            if !has_more {
                break;
            }
        }
        collected.sort_unstable();
        assert_eq!(
            collected,
            (0..10).collect::<Vec<_>>(),
            "page_size={page_size}：每条正好一次、不缺"
        );
    }
}
