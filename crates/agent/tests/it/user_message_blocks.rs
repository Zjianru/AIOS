//! T013.4：用户消息的块（清单 8）——日志块顺序/标记与投给模型的请求体。

use agent::Delivery;
use model::{ContentBlock, UserMessage};
use session::{EventKind, SessionStore};
use tools::ToolSet;

use crate::common::{FINAL_TEXT_SSE, ScriptedSseServer, SseScript, setup_agent_custom};

const ATTACHMENT_ID: &str = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn user_message_blocks_keep_order_and_request_body_uses_file_handle_text() {
    let server = ScriptedSseServer::start(vec![SseScript {
        frames: FINAL_TEXT_SSE.to_owned(),
        hold: false,
    }]);
    let cwd = tempfile::tempdir().unwrap();
    let (agent, data_dir, id) =
        setup_agent_custom(&server.base_url(), ToolSet::empty(), cwd.path(), Vec::new()).await;
    // 附件副本（模拟服务解析出来的只读路径）。
    let copy = cwd
        .path()
        .join("attachments/v1/files/ab")
        .join(ATTACHMENT_ID.trim_start_matches("sha256:"))
        .join("notes.txt");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, "hello").unwrap();
    let message = UserMessage::with_request_id(
        vec![
            ContentBlock::text("看这两个"),
            ContentBlock::paste("第一段粘贴"),
            ContentBlock::paste("第二段粘贴"),
            ContentBlock::File {
                attachment_id: ATTACHMENT_ID.to_owned(),
                name: "notes.txt".to_owned(),
                bytes: 5,
                path: Some(copy.to_string_lossy().into_owned()),
            },
        ],
        "req-t013-4",
    );
    agent.send(message, Delivery::FollowUp).await.unwrap();
    agent.idle().await;

    // 1) 日志 user/message 的块顺序 content→粘贴→文件，paste 标记对。
    let store = SessionStore::open_read_only(data_dir.path(), &id).expect("读日志");
    let user_message = store
        .events()
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::UserMessage { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("日志里要有 user/message");
    let blocks = &user_message.content;
    assert_eq!(blocks.len(), 4, "content + 两段粘贴 + 一个文件：{blocks:?}");
    assert_eq!(blocks[0].as_text(), Some("看这两个"), "第一块是 content");
    assert!(
        matches!(&blocks[1], ContentBlock::Text { text, paste } if text == "第一段粘贴" && *paste),
        "第二块是 paste=true 的粘贴：{:?}",
        blocks[1]
    );
    assert!(
        matches!(&blocks[2], ContentBlock::Text { text, paste } if text == "第二段粘贴" && *paste),
        "第三块是第二段粘贴：{:?}",
        blocks[2]
    );
    match &blocks[3] {
        ContentBlock::File {
            attachment_id,
            name,
            bytes,
            path,
        } => {
            assert_eq!(attachment_id, ATTACHMENT_ID);
            assert_eq!(name, "notes.txt");
            assert_eq!(*bytes, 5);
            assert_eq!(path.as_deref(), Some(copy.to_string_lossy().as_ref()));
        }
        other => panic!("第四块应是 File：{other:?}"),
    }

    // 2) 请求体：粘贴照原文；文件块换成 fileHandleText；文件内容不进请求。
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 1, "一轮一条模型请求");
    let body = String::from_utf8_lossy(&bodies[0]).to_string();
    assert!(body.contains("看这两个"), "content 在请求里");
    assert!(body.contains("第一段粘贴"), "粘贴原文（段一）");
    assert!(body.contains("第二段粘贴"), "粘贴原文（段二）");
    assert!(
        body.contains("(5 bytes, sha256:abcdef01): verbatim read-only copy saved at"),
        "文件块换成 fileHandleText：{body}"
    );
    assert!(
        body.contains(&copy.to_string_lossy().to_string()),
        "fileHandleText 的路径是副本绝对路径"
    );
    assert!(
        !body.contains("hello"),
        "文件内容不塞进请求（只给存在哪）：{body}"
    );
}
