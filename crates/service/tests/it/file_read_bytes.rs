//! 只读文件字节（path / attachmentId / 校验）。

use base64::Engine as _;
use service::{Call, FileReadBytesRequest, Response, ServiceError};
use tempfile::tempdir;

#[tokio::test]
async fn file_read_bytes_returns_path_bytes_and_version() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), b"hello").unwrap();
    let host = crate::common::open(crate::common::options(dir.path())).await;
    let response = host
        .dispatch(Call::FileReadBytes(FileReadBytesRequest {
            path: Some("hello.txt".to_owned()),
            attachment_id: None,
        }))
        .await
        .expect("读路径");
    let Response::FileReadBytes(bytes) = response else {
        panic!("应回 file_read_bytes");
    };
    assert_eq!(bytes.mime, "text/plain");
    assert_eq!(bytes.size, 5);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&bytes.bytes_base64)
            .unwrap(),
        b"hello"
    );
    assert!(!bytes.content_version.is_empty());
    assert!(bytes.text.is_none(), "文本文件不附 docx/pptx 文字");
}

#[tokio::test]
async fn file_read_bytes_reads_attachment_and_rejects_bad_input() {
    let dir = tempdir().unwrap();
    let stored = tools::attachments::store(dir.path(), "sample.xlsx", b"PK\x03\x04fake").unwrap();
    let host = crate::common::open(crate::common::options(dir.path())).await;
    let response = host
        .dispatch(Call::FileReadBytes(FileReadBytesRequest {
            path: None,
            attachment_id: Some(stored.id.clone()),
        }))
        .await
        .expect("读附件");
    let Response::FileReadBytes(bytes) = response else {
        panic!("应回 file_read_bytes");
    };
    assert_eq!(bytes.size, 8);
    assert_eq!(bytes.content_version, stored.id);
    assert!(bytes.mime.contains("spreadsheetml"));

    for (path, attachment) in [
        (Some("a".to_owned()), Some(stored.id.clone())),
        (None, None),
        (Some("missing.txt".to_owned()), None),
    ] {
        let error = host
            .dispatch(Call::FileReadBytes(FileReadBytesRequest {
                path,
                attachment_id: attachment,
            }))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ServiceError::InvalidPath(_)),
            "参数错误按 InvalidPath：{error}"
        );
    }

    let error = host
        .dispatch(Call::FileReadBytes(FileReadBytesRequest {
            path: None,
            attachment_id: Some("sha256:bad".to_owned()),
        }))
        .await
        .unwrap_err();
    assert!(matches!(error, ServiceError::AttachmentInvalid(_)));
}
