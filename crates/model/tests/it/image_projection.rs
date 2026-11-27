use std::collections::BTreeMap;

use model::{ContentBlock, Message, Role, image_omitted_text, project_images};

fn image(attachment_id: &str) -> ContentBlock {
    ContentBlock::Image {
        attachment_id: attachment_id.to_owned(),
        name: "pic.png".to_owned(),
        bytes: 3,
        mime: Some("image/png".to_owned()),
        path: Some("/tmp/pic.png".to_owned()),
        sent_as: None,
    }
}

fn message(blocks: Vec<ContentBlock>) -> Message {
    Message::new(Role::User, blocks)
}

#[test]
fn supports_images_keeps_the_original_image_block() {
    let messages = vec![message(vec![image("sha256:a")])];
    let projected = project_images(&messages, true, &BTreeMap::new());
    assert!(matches!(
        projected[0].content[0],
        ContentBlock::Image { .. }
    ));
}

#[test]
fn note_replaces_image_and_keeps_bytes_stable() {
    let messages = vec![message(vec![image("sha256:a")])];
    let mut notes = BTreeMap::new();
    notes.insert("sha256:a".to_owned(), "冻结文字".to_owned());
    let first = project_images(&messages, false, &notes);
    let second = project_images(&messages, false, &notes);
    assert_eq!(first, second);
    assert_eq!(
        first[0].content,
        vec![ContentBlock::text("冻结文字".to_owned())]
    );
}

#[test]
fn missing_notes_group_consecutive_images_into_one_d120_line() {
    let messages = vec![message(vec![
        image("sha256:a"),
        image("sha256:b"),
        ContentBlock::text("中间"),
        image("sha256:c"),
    ])];
    let projected = project_images(&messages, false, &BTreeMap::new());
    let texts: Vec<&str> = projected[0]
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 3);
    assert_eq!(
        texts[0],
        image_omitted_text(&["sha256:a".to_owned(), "sha256:b".to_owned()])
    );
    assert!(texts[0].contains("attachments: sha256:a, sha256:b"));
    assert_eq!(texts[1], "中间");
    assert_eq!(texts[2], image_omitted_text(&["sha256:c".to_owned()]));
}

#[test]
fn same_omitted_note_for_consecutive_images_is_emitted_once() {
    let messages = vec![message(vec![image("sha256:a"), image("sha256:b")])];
    let line = image_omitted_text(&["sha256:a".to_owned(), "sha256:b".to_owned()]);
    let mut notes = BTreeMap::new();
    notes.insert("sha256:a".to_owned(), line.clone());
    notes.insert("sha256:b".to_owned(), line.clone());
    let projected = project_images(&messages, false, &notes);
    assert_eq!(projected[0].content, vec![ContentBlock::text(line)]);
}
