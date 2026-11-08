//! 请求输入：把发消息/附件/@ 文件候选翻译成模型输入或本地文件。

use model::UserMessage;

use crate::error::ServiceError;
use crate::protocol::{
    AttachmentGetRequest, AttachmentGetResponse, AttachmentPutRequest, AttachmentPutResponse,
    FileReferenceEntry, FileReferencesListRequest, FileReferencesListResponse, SendRequest,
};
use crate::root_lifecycle::RootHost;

/// 把附件错误映射成服务错误码。
fn attachment_error(error: tools::attachments::AttachmentError) -> ServiceError {
    use tools::attachments::AttachmentError;
    match error {
        AttachmentError::NotText => ServiceError::AttachmentNotText,
        AttachmentError::BadName(_) | AttachmentError::BadId(_) => {
            ServiceError::AttachmentInvalid(error.to_string())
        }
        AttachmentError::Io(io) => ServiceError::AttachmentInvalid(format!("附件读写失败：{io}")),
    }
}

/// 严格 base64 解码（不引新依赖；不规范返回 None）。
///
/// 只收规范形式：长度是 4 的倍数、`=` 只允许在末尾（最多两个）、`=` 后不能有数据、
/// 尾部填充位的低位必须为 0（`AA==` 收，`AB==` 拒）。
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let total = bytes.len() / 4;
    let mut output = Vec::with_capacity(total * 3);
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index + 1 == total;
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return None;
        }
        let data = &chunk[..4 - padding];
        if data.contains(&b'=') {
            return None;
        }
        let mut values = [0u8; 4];
        for (position, byte) in data.iter().enumerate() {
            values[position] = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return None,
            };
        }
        // 非规范的尾部填充位拒收：`=` 占掉的 sextet 低位必须是 0。
        if padding == 1 && values[2] & 0b11 != 0 {
            return None;
        }
        if padding == 2 && values[1] & 0b1111 != 0 {
            return None;
        }
        let number = (u32::from(values[0]) << 18)
            | (u32::from(values[1]) << 12)
            | (u32::from(values[2]) << 6)
            | u32::from(values[3]);
        output.push((number >> 16) as u8);
        if padding < 2 {
            output.push((number >> 8) as u8);
        }
        if padding < 1 {
            output.push(number as u8);
        }
    }
    Some(output)
}

impl RootHost {
    /// 把 SendRequest 拼成块状用户消息（canvas→content→pastes→files；附件解析成副本路径）。
    pub(crate) fn user_message_from(
        &self,
        request: &SendRequest,
    ) -> Result<UserMessage, ServiceError> {
        let mut blocks = Vec::new();
        // 画布按钮放进来的那一句排在最前（照原型 composeForModel 的顺序）；
        // 块上带画布身份，页面据此画「来自画布」标记，模型看到的也是这句话。
        if let Some(canvas) = &request.canvas {
            blocks.push(model::ContentBlock::from_canvas(model::CanvasSource::new(
                canvas.id.clone(),
                canvas.title.clone(),
            )));
        }
        if !request.content.is_empty() {
            blocks.push(model::ContentBlock::text(request.content.clone()));
        }
        for paste in &request.pastes {
            if !paste.is_empty() {
                blocks.push(model::ContentBlock::paste(paste.clone()));
            }
        }
        for attachment in &request.attachments {
            let stored = tools::attachments::read(&self.options.data_dir, &attachment.id)
                .map_err(attachment_error)?;
            // 名字/字节数用请求里的值：同内容不同名时，卡片上要显示用户拖进来的那个名字。
            blocks.push(model::ContentBlock::File {
                attachment_id: attachment.id.clone(),
                name: attachment.name.clone(),
                bytes: attachment.bytes,
                path: Some(stored.path.to_string_lossy().into_owned()),
            });
        }
        if blocks.is_empty() {
            return Err(ServiceError::MessageEmpty);
        }
        Ok(UserMessage::with_request_id(
            blocks,
            request.request_id.clone(),
        ))
    }

    /// 按会话工作目录列 @ 文件候选。
    pub(crate) async fn file_references_list(
        &self,
        request: FileReferencesListRequest,
    ) -> Result<FileReferencesListResponse, ServiceError> {
        let references = tools::file_references::list(&self.options.cwd, &request.query)
            .map_err(|error| ServiceError::InvalidPath(error.to_string()))?;
        Ok(FileReferencesListResponse {
            references: references
                .into_iter()
                .map(|reference| FileReferenceEntry {
                    path: reference.path,
                    bytes: reference.bytes,
                })
                .collect(),
        })
    }

    /// 存一份文字附件（base64 → 只读副本，返回 id）。
    pub(crate) async fn attachment_put(
        &self,
        request: AttachmentPutRequest,
    ) -> Result<AttachmentPutResponse, ServiceError> {
        let data = decode_base64(&request.data)
            .ok_or_else(|| ServiceError::AttachmentInvalid("附件内容不是合法 base64".to_owned()))?;
        let stored = tools::attachments::store(&self.options.data_dir, &request.name, &data)
            .map_err(attachment_error)?;
        Ok(AttachmentPutResponse {
            id: stored.id,
            name: stored.name,
            bytes: stored.bytes as u64,
        })
    }

    /// 按 id 读回附件（卡片「点开看全文」）。
    pub(crate) async fn attachment_get(
        &self,
        request: AttachmentGetRequest,
    ) -> Result<AttachmentGetResponse, ServiceError> {
        let stored = tools::attachments::read(&self.options.data_dir, &request.id)
            .map_err(attachment_error)?;
        let bytes = std::fs::read(&stored.path)
            .map_err(|error| ServiceError::AttachmentInvalid(format!("附件读不回来：{error}")))?;
        // 仓库按 git 的「无 NUL 即文字」收；不是合法 UTF-8 时给 text=None（旧数据兼容口径）。
        let text = String::from_utf8(bytes).ok();
        Ok(AttachmentGetResponse {
            name: stored.name,
            bytes: stored.bytes as u64,
            text,
        })
    }
}
