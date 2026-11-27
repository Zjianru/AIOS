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

#[tokio::test]
async fn file_read_bytes_rejects_escape_absolute_outside_and_symlink_outside() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("inside.txt"), b"inside").unwrap();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
    let host = crate::common::open(crate::common::options(&data)).await;

    // 绝对路径在 cwd 内：允许。
    let ok = host
        .dispatch(Call::FileReadBytes(FileReadBytesRequest {
            path: Some(data.join("inside.txt").display().to_string()),
            attachment_id: None,
        }))
        .await;
    assert!(ok.is_ok(), "cwd 内绝对路径允许：{ok:?}");

    // 相对 ../ 和绝对外部、cwd 内 symlink 指向外部：都拒绝。
    std::os::unix::fs::symlink(outside.path().join("secret.txt"), data.join("link.txt")).unwrap();
    for path in [
        "../secret.txt".to_owned(),
        outside.path().join("secret.txt").display().to_string(),
        data.join("link.txt").display().to_string(),
    ] {
        let error = host
            .dispatch(Call::FileReadBytes(FileReadBytesRequest {
                path: Some(path.clone()),
                attachment_id: None,
            }))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ServiceError::InvalidPath(_)),
            "{path} 应按 InvalidPath 拒绝：{error}"
        );
        if path.contains("link.txt") || path.contains("secret.txt") && !path.starts_with("..") {
            assert!(
                error.to_string().contains("会话工作目录"),
                "错误要可读：{error}"
            );
        }
    }
}

#[tokio::test]
async fn file_read_bytes_rejects_file_over_preview_limit() {
    let dir = tempdir().unwrap();
    let host = crate::common::open(crate::common::options(dir.path())).await;
    let huge = dir.path().join("huge.bin");
    let file = std::fs::File::create(&huge).unwrap();
    file.set_len(50 * 1024 * 1024 + 1).unwrap();
    let error = host
        .dispatch(Call::FileReadBytes(FileReadBytesRequest {
            path: Some("huge.bin".to_owned()),
            attachment_id: None,
        }))
        .await
        .unwrap_err();
    assert!(matches!(error, ServiceError::InvalidPath(_)));
    assert!(error.to_string().contains("太大"), "{error}");
}

#[tokio::test]
async fn file_read_bytes_allows_office_library_when_cwd_differs_from_data_dir() {
    let data = tempdir().unwrap();
    let work = tempdir().unwrap();
    std::fs::create_dir_all(data.path().join("library/office/task-1")).unwrap();
    let target = data.path().join("library/office/task-1/水果表.xlsx");
    std::fs::write(&target, b"PK\x03\x04fake").unwrap();
    let mut options = crate::common::options(data.path());
    options.cwd = work.path().to_path_buf();
    let host = crate::common::open(options).await;

    let response = host
        .dispatch(Call::FileReadBytes(FileReadBytesRequest {
            path: Some(target.display().to_string()),
            attachment_id: None,
        }))
        .await
        .expect("交付目录的绝对路径要允许");
    let Response::FileReadBytes(bytes) = response else {
        panic!("应回 file_read_bytes");
    };
    assert_eq!(bytes.size, 8);

    // 同一 data-dir 下、library/office 之外的文件仍拒绝。
    std::fs::write(data.path().join("secret.txt"), b"secret").unwrap();
    let error = host
        .dispatch(Call::FileReadBytes(FileReadBytesRequest {
            path: Some(data.path().join("secret.txt").display().to_string()),
            attachment_id: None,
        }))
        .await
        .unwrap_err();
    assert!(matches!(error, ServiceError::InvalidPath(_)));
    assert!(error.to_string().contains("Office 交付目录"));
}
