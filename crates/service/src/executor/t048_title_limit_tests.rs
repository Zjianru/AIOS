use super::TASK_ACCEPT_TITLE_DESCRIPTION;

#[test]
fn task_accept_title_description_states_the_card_title_limit() {
    assert!(
        TASK_ACCEPT_TITLE_DESCRIPTION.contains("at most 20 characters"),
        "title 说明要写明确上限 20 characters：{TASK_ACCEPT_TITLE_DESCRIPTION}"
    );
    assert!(
        TASK_ACCEPT_TITLE_DESCRIPTION.contains("Chinese characters count as one"),
        "中文按一字计：{TASK_ACCEPT_TITLE_DESCRIPTION}"
    );
    assert!(
        TASK_ACCEPT_TITLE_DESCRIPTION.contains("card title"),
        "说明这是卡片标题：{TASK_ACCEPT_TITLE_DESCRIPTION}"
    );
    assert!(
        TASK_ACCEPT_TITLE_DESCRIPTION.contains("truncated"),
        "超长会被截断：{TASK_ACCEPT_TITLE_DESCRIPTION}"
    );
}
