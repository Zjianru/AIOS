//! .3：只读文件字节（预览用；路径或附件 id 二选一）。
//!
//! 校验照 read 工具：路径相对根会话 cwd 解析、必须存在且是普通文件；附件走附件仓库。
//! 只读、不做内容解释；坏文件/超限报结构化错误，页面按错误显示。

use std::path::PathBuf;

use crate::error::ServiceError;
use crate::protocol::{FileReadBytesRequest, FileReadBytesResponse};
use crate::root_lifecycle::RootHost;
use base64::Engine as _;

/// 预览一次最多读多少字节；再大就先不做预览（不拿半份文件糊弄）。
const MAX_PREVIEW_BYTES: u64 = 50 * 1024 * 1024;

impl RootHost {
    pub(crate) async fn file_read_bytes(
        &self,
        request: FileReadBytesRequest,
    ) -> Result<FileReadBytesResponse, ServiceError> {
        match (request.path.as_deref(), request.attachment_id.as_deref()) {
            (Some(path), None) => self.read_path_bytes(path).await,
            (None, Some(id)) => self.read_attachment_bytes(id).await,
            (Some(_), Some(_)) => Err(ServiceError::InvalidPath(
                "path 和 attachmentId 只能给一个".to_owned(),
            )),
            (None, None) => Err(ServiceError::InvalidPath(
                "要给 path 或 attachmentId".to_owned(),
            )),
        }
    }

    async fn read_path_bytes(
        &self,
        requested: &str,
    ) -> Result<FileReadBytesResponse, ServiceError> {
        let path = PathBuf::from(requested);
        let path = if path.is_absolute() {
            path
        } else {
            self.options.cwd.join(path)
        };
        let canonical = std::fs::canonicalize(&path).map_err(|error| {
            ServiceError::InvalidPath(format!("读不到 {}：{error}", path.display()))
        })?;
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| ServiceError::InvalidPath(format!("读不到文件信息：{error}")))?;
        if !metadata.is_file() {
            return Err(ServiceError::InvalidPath(format!(
                "{} 不是普通文件",
                canonical.display()
            )));
        }
        let size = metadata.len();
        if size > MAX_PREVIEW_BYTES {
            return Err(ServiceError::InvalidPath(format!(
                "文件太大（{} 字节，预览上限 {} 字节）",
                size, MAX_PREVIEW_BYTES
            )));
        }
        let bytes = std::fs::read(&canonical)
            .map_err(|error| ServiceError::InvalidPath(format!("读不了文件：{error}")))?;
        let mime = mime_for(&canonical);
        let content_version = format!(
            "{}:{}",
            size,
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        );
        Ok(FileReadBytesResponse {
            mime,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            size,
            content_version,
        })
    }

    async fn read_attachment_bytes(&self, id: &str) -> Result<FileReadBytesResponse, ServiceError> {
        let stored = tools::attachments::read(&self.options.data_dir, id)
            .map_err(|error| ServiceError::AttachmentInvalid(error.to_string()))?;
        let size = stored.bytes as u64;
        if size > MAX_PREVIEW_BYTES {
            return Err(ServiceError::AttachmentInvalid(format!(
                "附件太大（{size} 字节，预览上限 {MAX_PREVIEW_BYTES} 字节）"
            )));
        }
        let bytes = std::fs::read(&stored.path)
            .map_err(|error| ServiceError::AttachmentInvalid(format!("读不了附件：{error}")))?;
        Ok(FileReadBytesResponse {
            mime: stored
                .mime
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            size,
            content_version: stored.id,
        })
    }
}

fn mime_for(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(tools::attachments::mime_for_name)
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}
