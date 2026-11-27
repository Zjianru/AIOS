//! T013.4：send 的 pastes/attachments（清单 8、10 的服务面）。

use std::path::Path;
use std::time::Duration;

use service::{
    AttachmentPutRequest, Call, RootHost, SendAttachment, SendCanvas, SendObjectRef, SendQuote,
    SendRequest, SendResultCard,
};
use tempfile::tempdir;

async fn host(dir: &Path, cwd: &Path) -> RootHost {
    let mut options = crate::common::options(dir);
    options.cwd = cwd.to_path_buf();
    crate::common::open(options).await
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn request(content: &str) -> SendRequest {
    SendRequest {
        request_id: "req-t013-4".to_owned(),
        content: content.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        object_refs: Vec::new(),
        quotes: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
    }
}

/// 等日志里出现 user/message 并返回它的块（send 后 step 开始时写）。
async fn wait_user_blocks(host: &RootHost, data_dir: &Path) -> Vec<model::ContentBlock> {
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let status = host
            .dispatch(Call::Status(service::StatusRequest {}))
            .await
            .expect("status");
        let json = serde_json::to_value(&status).unwrap();
        let Some(session_id) = json["object"]["sessionId"].as_str() else {
            continue;
        };
        let Ok(id) = session::SessionId::parse(session_id) else {
            continue;
        };
        let Ok(store) = session::SessionStore::open_read_only(data_dir, &id) else {
            continue;
        };
        if let Some(message) = store.events().iter().find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. }
                if matches!(
                    &message.source,
                    model::MessageSource::User { rpc_id: Some(rpc_id) }
                        if rpc_id.starts_with("req-")
                ) =>
            {
                Some(message.clone())
            }
            _ => None,
        }) {
            return message.content;
        }
    }
    panic!("等不到 user/message");
}

#[tokio::test]
async fn send_builds_blocks_in_order_content_then_pastes_then_files() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let put = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "notes.txt".to_owned(),
            data: "aGVsbG8=".to_owned(),
        }))
        .await
        .expect("put");
    let id = serde_json::to_value(&put).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut send = request("看这个");
    send.pastes = vec!["第一段".to_owned(), "".to_owned(), "第二段".to_owned()];
    send.attachments = vec![SendAttachment {
        id: id.clone(),
        name: "notes.txt".to_owned(),
        bytes: 5,
        mime: None,
    }];
    host.dispatch(Call::Send(send)).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    assert_eq!(
        blocks.len(),
        4,
        "content + 两段非空粘贴 + 一个文件（空粘贴跳过）：{blocks:?}"
    );
    assert_eq!(blocks[0].as_text(), Some("看这个"));
    assert!(
        matches!(&blocks[1], model::ContentBlock::Text { text, paste, .. } if text == "第一段" && *paste),
        "第二块是粘贴：{:?}",
        blocks[1]
    );
    assert!(
        matches!(&blocks[2], model::ContentBlock::Text { text, paste, .. } if text == "第二段" && *paste),
        "第三块是第二段粘贴：{:?}",
        blocks[2]
    );
    match &blocks[3] {
        model::ContentBlock::File {
            attachment_id,
            name,
            bytes,
            path,
            ..
        } => {
            assert_eq!(attachment_id, &id);
            assert_eq!(name, "notes.txt");
            assert_eq!(*bytes, 5);
            assert!(
                path.as_deref()
                    .is_some_and(|path| path.contains("attachments/v1/files")),
                "路径是只读副本：{path:?}"
            );
        }
        other => panic!("第四块应是 File：{other:?}"),
    }
}

#[tokio::test]
async fn send_with_empty_content_starts_with_the_paste_block() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let mut send = request("");
    send.pastes = vec!["只有粘贴".to_owned()];
    host.dispatch(Call::Send(send)).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    assert_eq!(blocks.len(), 1, "content 空就不放那一块：{blocks:?}");
    assert!(
        matches!(&blocks[0], model::ContentBlock::Text { text, paste, .. } if text == "只有粘贴" && *paste),
        "{blocks:?}"
    );
}

#[tokio::test]
async fn attachment_name_and_bytes_use_the_request_values() {
    // 同内容异名：仓库里的副本叫 a.txt，请求里带的是用户拖进来的 b.txt —— 块里用请求值。
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let put = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "a.txt".to_owned(),
            data: "aGVsbG8=".to_owned(),
        }))
        .await
        .expect("put");
    let id = serde_json::to_value(&put).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut send = request("看文件");
    send.attachments = vec![SendAttachment {
        id,
        name: "b.txt".to_owned(),
        bytes: 99,
        mime: None,
    }];
    host.dispatch(Call::Send(send)).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    match blocks.last() {
        Some(model::ContentBlock::File { name, bytes, .. }) => {
            assert_eq!(name, "b.txt", "用请求里的名字（用户卡片名）");
            assert_eq!(*bytes, 99, "用请求里的字节数");
        }
        other => panic!("最后一块应是 File：{other:?}"),
    }
}

#[tokio::test]
async fn empty_message_is_rejected() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let error = host
        .dispatch(Call::Send(request("")))
        .await
        .expect_err("空消息要拒收");
    assert_eq!(error.code().as_str(), "request/invalid-message");
}

#[tokio::test]
async fn unknown_attachment_is_rejected() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let mut send = request("带个文件");
    send.attachments = vec![SendAttachment {
        id: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned(),
        name: "missing.txt".to_owned(),
        bytes: 1,
        mime: None,
    }];
    let error = host
        .dispatch(Call::Send(send))
        .await
        .expect_err("坏附件要拒收");
    assert_eq!(error.code().as_str(), "attachment/invalid");
}

/* ───────── T015.4：画布放进输入框的那一句（「来自画布」） ───────── */

/// 页面发 send 的真实形状：JSON 里带一个 canvas（画布 id + 标题）。
/// 冻结基上 SendRequest 还不认这个字段，测试照样跑得起来（serde 忽略多余字段），
/// 红就红在块上：没有标记块、没有 canvas、文本还是打的字。
fn canvas_send_request() -> Call {
    serde_json::from_value(serde_json::json!({
        "method": "send",
        "requestId": "req-t015-4",
        "content": "按这个存：每月 3000",
        "canvas": { "id": "cv_1", "title": "每月存钱算一算" },
    }))
    .expect("send 请求要读得出来")
}

/// 日志里用户消息的块，按 JSON 原样（要断言的就是日志里的形状）。
fn user_blocks_json(blocks: &[model::ContentBlock]) -> Vec<serde_json::Value> {
    blocks
        .iter()
        .map(|block| serde_json::to_value(block).expect("块要能序列化"))
        .collect()
}

#[tokio::test]
async fn send_with_canvas_adds_a_tagged_text_block_before_the_message() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    host.dispatch(canvas_send_request()).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    let json = user_blocks_json(&blocks);
    assert_eq!(json.len(), 2, "标记块 + 打的字：{json:?}");
    assert_eq!(json[0]["type"], "text", "标记那一块是文字块：{json:?}");
    assert_eq!(
        json[0]["text"], "（来自画布「每月存钱算一算」）",
        "{json:?}"
    );
    assert_eq!(
        json[0]["canvas"],
        serde_json::json!({ "id": "cv_1", "title": "每月存钱算一算" }),
        "块上的 canvas 恰好是 id/title（多一个字段就是契约变了）：{json:?}"
    );
    assert_eq!(json[1]["text"], "按这个存：每月 3000", "{json:?}");
    assert!(
        json[1].get("canvas").is_none(),
        "打的字那一块不带 canvas：{json:?}"
    );
}

/// 不带 canvas 的 send 和 T013.4 一样只有一块（移除标记后再发就是这个形状）。
#[tokio::test]
async fn send_without_canvas_keeps_a_single_text_block() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    host.dispatch(Call::Send(request("按这个存：每月 3000")))
        .await
        .expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    assert_eq!(
        user_blocks_json(&blocks),
        vec![serde_json::json!({ "type": "text", "text": "按这个存：每月 3000" })],
        "没有画布就不该多出标记块"
    );
}

/// （N2/N1）：多条结果卡/画布标记按顺序成块，附件原 MIME 跟着文件块落日志。
#[tokio::test]
async fn send_appends_multi_markers_in_order_and_keeps_attachment_mime() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let put = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "pic.png".to_owned(),
            data: "aGVsbG8=".to_owned(),
        }))
        .await
        .expect("put");
    let id = serde_json::to_value(&put).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut send = request("带标记");
    send.result_cards = vec![
        SendResultCard {
            task_id: "task-a".to_owned(),
            title: "结果甲".to_owned(),
            version: 1,
        },
        SendResultCard {
            task_id: "task-b".to_owned(),
            title: "结果乙".to_owned(),
            version: 2,
        },
    ];
    send.canvases = vec![
        SendCanvas {
            id: "cv-a".to_owned(),
            title: "画布甲".to_owned(),
        },
        SendCanvas {
            id: "cv-b".to_owned(),
            title: "画布乙".to_owned(),
        },
    ];
    send.attachments = vec![SendAttachment {
        id: id.clone(),
        name: "pic.png".to_owned(),
        bytes: 5,
        mime: Some("image/png".to_owned()),
    }];
    host.dispatch(Call::Send(send)).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    assert_eq!(
        blocks.len(),
        6,
        "两张卡 + 两个画布 + 正文 + 文件：{blocks:?}"
    );
    match (&blocks[0], &blocks[1]) {
        (
            model::ContentBlock::ResultCard { task_id, .. },
            model::ContentBlock::ResultCard { title, version, .. },
        ) => {
            assert_eq!(task_id, "task-a");
            assert_eq!(title, "结果乙");
            assert_eq!(*version, 2);
        }
        other => panic!("前两块应是结果卡：{other:?}"),
    }
    match (&blocks[2], &blocks[3]) {
        (
            model::ContentBlock::Text {
                canvas: Some(left), ..
            },
            model::ContentBlock::Text {
                canvas: Some(right),
                ..
            },
        ) => {
            assert_eq!((left.id.as_str(), left.title.as_str()), ("cv-a", "画布甲"));
            assert_eq!(
                (right.id.as_str(), right.title.as_str()),
                ("cv-b", "画布乙")
            );
        }
        other => panic!("第三、四块应是画布标记：{other:?}"),
    }
    assert_eq!(blocks[4].as_text(), Some("带标记"));
    match &blocks[5] {
        model::ContentBlock::Image {
            mime, name, path, ..
        } => {
            assert_eq!(name, "pic.png");
            assert_eq!(mime.as_deref(), Some("image/png"), "图片 MIME 要写进图片块");
            assert!(
                path.as_deref()
                    .is_some_and(|path| path.contains("attachments/v1/files")),
                "图片块带只读副本路径：{path:?}"
            );
        }
        other => panic!("最后一块应是图片：{other:?}"),
    }
}
#[tokio::test]
async fn send_svg_attachment_stays_a_file_block_in_the_log() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let bytes = b"<svg xmlns='http://www.w3.org/2000/svg'/>";
    let put = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "pic.svg".to_owned(),
            data: base64(bytes),
        }))
        .await
        .expect("put");
    let id = serde_json::to_value(&put).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut send = request("看这个图");
    send.attachments = vec![SendAttachment {
        id: id.clone(),
        name: "pic.svg".to_owned(),
        bytes: bytes.len() as u64,
        mime: None,
    }];
    host.dispatch(Call::Send(send)).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    match &blocks[1] {
        model::ContentBlock::File {
            attachment_id,
            name,
            mime,
            ..
        } => {
            assert_eq!(attachment_id, &id);
            assert_eq!(name, "pic.svg");
            assert_eq!(mime.as_deref(), Some("image/svg+xml"), "svg 仍是文字附件");
        }
        other => panic!("svg 不能写成 Image 块：{other:?}"),
    }
}

#[tokio::test]
async fn send_image_attachment_becomes_an_image_block_in_the_log() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let bytes = b"\x89PNG\r\n\x1a\n";
    let put = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "shot.png".to_owned(),
            data: base64(bytes),
        }))
        .await
        .expect("put");
    let id = serde_json::to_value(&put).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut send = request("看图");
    send.attachments = vec![SendAttachment {
        id: id.clone(),
        name: "shot.png".to_owned(),
        bytes: bytes.len() as u64,
        mime: None,
    }];
    host.dispatch(Call::Send(send)).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    match &blocks[1] {
        model::ContentBlock::Image {
            attachment_id,
            mime,
            bytes,
            ..
        } => {
            assert_eq!(attachment_id, &id);
            assert_eq!(mime.as_deref(), Some("image/png"), "按扩展名兜底 mime");
            assert_eq!(*bytes, 8);
        }
        other => panic!("图片附件要写成 Image 块：{other:?}"),
    }
}

#[tokio::test]
async fn send_object_refs_become_object_ref_blocks_in_the_log() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let mut send = request("看看这些");
    send.object_refs = vec![
        SendObjectRef {
            kind: "task".to_owned(),
            id: "task-1".to_owned(),
            title: "查重复链接".to_owned(),
            version: Some(2),
            task_id: None,
        },
        SendObjectRef {
            kind: "report".to_owned(),
            id: "report-1".to_owned(),
            title: "查重复链接".to_owned(),
            version: Some(2),
            task_id: Some("task-1".to_owned()),
        },
        SendObjectRef {
            kind: "project".to_owned(),
            id: "project-1".to_owned(),
            title: "新闻归档".to_owned(),
            version: None,
            task_id: None,
        },
    ];
    host.dispatch(Call::Send(send)).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    match &blocks[1] {
        model::ContentBlock::ObjectRef {
            kind,
            id,
            title,
            version,
            task_id,
        } => {
            assert_eq!(kind, "task");
            assert_eq!(id, "task-1");
            assert_eq!(title, "查重复链接");
            assert_eq!(*version, Some(2));
            assert_eq!(task_id, &None);
        }
        other => panic!("第一个对象引用要写成 ObjectRef 块：{other:?}"),
    }
    match &blocks[2] {
        model::ContentBlock::ObjectRef {
            kind,
            id,
            version,
            task_id,
            ..
        } => {
            assert_eq!(kind, "report");
            assert_eq!(id, "report-1");
            assert_eq!(*version, Some(2));
            assert_eq!(task_id.as_deref(), Some("task-1"));
        }
        other => panic!("第二个对象引用要写成 ObjectRef 块：{other:?}"),
    }
    assert!(matches!(&blocks[3], model::ContentBlock::ObjectRef { kind, .. } if kind == "project"));
}

#[tokio::test]
async fn send_quoted_selection_becomes_a_quote_block_in_the_log() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = host(data.path(), cwd.path()).await;
    let mut send = request("引用一下");
    send.quotes = vec![SendQuote {
        text: "判重用的是原始链接".to_owned(),
        source: Some(SendObjectRef {
            kind: "report".to_owned(),
            id: "report-1".to_owned(),
            title: "查重复链接".to_owned(),
            version: Some(2),
            task_id: Some("task-1".to_owned()),
        }),
    }];
    host.dispatch(Call::Send(send)).await.expect("send");
    let blocks = wait_user_blocks(&host, data.path()).await;
    match &blocks[1] {
        model::ContentBlock::Quote { text, source } => {
            assert_eq!(text, "判重用的是原始链接");
            let source = source.as_ref().expect("引用要带来源");
            assert_eq!(source.kind, "report");
            assert_eq!(source.id, "report-1");
            assert_eq!(source.title, "查重复链接");
            assert_eq!(source.version, Some(2));
            assert_eq!(source.task_id.as_deref(), Some("task-1"));
        }
        other => panic!("引用要写成 Quote 块：{other:?}"),
    }
}
