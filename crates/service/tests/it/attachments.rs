//! T013.2：附件仓库的服务 op（清单 10 的 attachment-put / attachment-get）。

use std::path::Path;

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{AttachmentGetRequest, AttachmentPutRequest, Call, RootHost, RootSessionOptions};
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

fn options(data_dir: &Path, identity: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: data_dir.to_path_buf(),
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

async fn host(dir: &Path) -> RootHost {
    RootHost::open(
        options(dir, &dir.join("identity.txt")),
        DEFAULT_CONTEXT_WINDOW,
    )
    .await
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

#[tokio::test]
async fn attachment_put_returns_id_and_bytes() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let response = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "notes.txt".to_owned(),
            data: base64("hello\n".as_bytes()),
        }))
        .await
        .expect("put 应成功");
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["method"], "attachment-put");
    assert!(
        json["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("sha256:")),
        "id 以 sha256: 开头：{json}"
    );
    assert_eq!(json["name"], "notes.txt");
    assert_eq!(json["bytes"], 6);
}

#[tokio::test]
async fn attachment_put_rejects_binary_with_not_text_code() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let error = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "bin.dat".to_owned(),
            data: base64(b"abc\0def"),
        }))
        .await
        .expect_err("二进制要拒收");
    assert_eq!(error.code().as_str(), "attachment/not-text");
}

#[tokio::test]
async fn attachment_put_rejects_bad_base64() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let error = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "a.txt".to_owned(),
            data: "!!!not base64!!!".to_owned(),
        }))
        .await
        .expect_err("坏 base64 要拒收");
    assert_eq!(error.code().as_str(), "attachment/invalid");
}

#[tokio::test]
async fn attachment_put_rejects_bad_name() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let error = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "a/b.txt".to_owned(),
            data: base64(b"hello"),
        }))
        .await
        .expect_err("坏名字要拒收");
    assert_eq!(error.code().as_str(), "attachment/invalid");
}

#[tokio::test]
async fn attachment_get_returns_the_text() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let put = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "notes.txt".to_owned(),
            data: base64("你好，世界\n".as_bytes()),
        }))
        .await
        .expect("put");
    let put_json = serde_json::to_value(&put).unwrap();
    let id = put_json["id"].as_str().unwrap().to_owned();
    let got = host
        .dispatch(Call::AttachmentGet(AttachmentGetRequest { id }))
        .await
        .expect("get");
    let json = serde_json::to_value(&got).unwrap();
    assert_eq!(json["method"], "attachment-get");
    assert_eq!(json["name"], "notes.txt");
    assert_eq!(json["text"], "你好，世界\n");
}

#[tokio::test]
async fn attachment_get_rejects_bad_id() {
    let dir = tempdir().unwrap();
    let host = host(dir.path()).await;
    let error = host
        .dispatch(Call::AttachmentGet(AttachmentGetRequest {
            id: "sha256:xx".to_owned(),
        }))
        .await
        .expect_err("坏 id 要拒收");
    assert_eq!(error.code().as_str(), "attachment/invalid");
}
