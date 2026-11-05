//! T013.4：send 的 pastes/attachments（清单 8、10 的服务面）。

use std::path::Path;
use std::time::Duration;

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{
    AttachmentPutRequest, Call, RootHost, RootSessionOptions, SendAttachment, SendRequest,
};
use tempfile::tempdir;

struct TestCredential;

impl model::CredentialSource for TestCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok("test-key".to_owned())
    }
}

fn options(data_dir: &Path, identity: &Path, cwd: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        model: DeepSeekConfig {
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::new("high"),
        },
        config: AgentConfig::default(),
        credentials: Some(std::sync::Arc::new(TestCredential)),
        identity: Some(identity.to_path_buf()),
        model_factory: None,
        append_hook_factory: None,
        keyring_changed: None,
    }
}

async fn host(dir: &Path, cwd: &Path) -> RootHost {
    RootHost::open(
        options(dir, &dir.join("identity.txt"), cwd),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await
}

fn request(content: &str) -> SendRequest {
    SendRequest {
        request_id: "req-t013-4".to_owned(),
        content: content.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
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
            session::EventKind::UserMessage { message, .. } => Some(message.clone()),
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
        matches!(&blocks[1], model::ContentBlock::Text { text, paste } if text == "第一段" && *paste),
        "第二块是粘贴：{:?}",
        blocks[1]
    );
    assert!(
        matches!(&blocks[2], model::ContentBlock::Text { text, paste } if text == "第二段" && *paste),
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
        matches!(&blocks[0], model::ContentBlock::Text { text, paste } if text == "只有粘贴" && *paste),
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
    }];
    let error = host
        .dispatch(Call::Send(send))
        .await
        .expect_err("坏附件要拒收");
    assert_eq!(error.code().as_str(), "attachment/invalid");
}
