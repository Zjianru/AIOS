//! T013.4：send 的 pastes/attachments（清单 10 的服务面）。

use std::path::Path;

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

fn request(content: &str) -> SendRequest {
    SendRequest {
        request_id: "req-t013-4".to_owned(),
        content: content.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
    }
}

#[tokio::test]
async fn send_with_pastes_and_attachments_is_accepted() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = RootHost::open(
        options(data.path(), &data.path().join("identity.txt"), cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
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
    send.pastes = vec!["第一段".to_owned(), "第二段".to_owned()];
    send.attachments = vec![SendAttachment {
        id,
        name: "notes.txt".to_owned(),
        bytes: 5,
    }];
    let response = host.dispatch(Call::Send(send)).await.expect("send 应受理");
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["accepted"], true);
}

#[tokio::test]
async fn empty_message_is_rejected() {
    let data = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let host = RootHost::open(
        options(data.path(), &data.path().join("identity.txt"), cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
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
    let host = RootHost::open(
        options(data.path(), &data.path().join("identity.txt"), cwd.path()),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await;
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
