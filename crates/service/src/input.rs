//! 请求输入：把发消息/附件/@ 文件候选翻译成模型输入或本地文件。

use model::UserMessage;

use crate::error::ServiceError;
use crate::protocol::{
    AttachmentGetRequest, AttachmentGetResponse, AttachmentPutRequest, AttachmentPutResponse,
    FileReferenceEntry, FileReferenceObject, FileReferencesListRequest, FileReferencesListResponse,
    SendRequest,
};
use crate::root_lifecycle::RootHost;

/// 只有这四种图片进 Image 块（Anthropic 支持的格式；svg 等按文字附件走）。
pub(crate) fn image_format_supported(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

/// 把附件错误映射成服务错误码。
pub(crate) fn attachment_error(error: tools::attachments::AttachmentError) -> ServiceError {
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
/// 规范 base64 编码（不引新依赖；给图片二进制用）。
fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
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
        // T054.2：结果卡「要求修改」带进输入框的卡排在用户文字前面（模型看到一行文字）。
        if let Some(card) = &request.result_card {
            blocks.push(model::ContentBlock::from_result_card(
                card.task_id.clone(),
                card.title.clone(),
                card.version,
            ));
        }
        // 画布按钮放进来的那一句排在最前（照原型 composeForModel 的顺序）；
        // 块上带画布身份，页面据此画「来自画布」标记，模型看到的也是这句话。
        if let Some(canvas) = &request.canvas {
            blocks.push(model::ContentBlock::from_canvas(model::CanvasSource::new(
                canvas.id.clone(),
                canvas.title.clone(),
            )));
        }
        // （N2）：一次放回多条结果卡/画布标记，按页面上的顺序追加（旧单字段保留兼容）。
        for card in &request.result_cards {
            blocks.push(model::ContentBlock::from_result_card(
                card.task_id.clone(),
                card.title.clone(),
                card.version,
            ));
        }
        for canvas in &request.canvases {
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
        for object in &request.object_refs {
            blocks.push(model::ContentBlock::ObjectRef {
                kind: object.kind.clone(),
                id: object.id.clone(),
                title: object.title.clone(),
                version: object.version,
                task_id: object.task_id.clone(),
            });
        }
        for attachment in &request.attachments {
            let stored = tools::attachments::read(&self.options.data_dir, &attachment.id)
                .map_err(attachment_error)?;
            // 名字/字节数用请求里的值：同内容不同名时，卡片上要显示用户拖进来的那个名字。
            let mime = attachment.mime.clone().or_else(|| stored.mime.clone());
            let path = Some(stored.path.to_string_lossy().into_owned());
            if mime.as_deref().is_some_and(image_format_supported) {
                blocks.push(model::ContentBlock::Image {
                    attachment_id: attachment.id.clone(),
                    name: attachment.name.clone(),
                    bytes: attachment.bytes,
                    mime,
                    path,
                });
            } else {
                blocks.push(model::ContentBlock::File {
                    attachment_id: attachment.id.clone(),
                    name: attachment.name.clone(),
                    bytes: attachment.bytes,
                    path,
                    mime,
                });
            }
        }
        if blocks.is_empty() {
            return Err(ServiceError::MessageEmpty);
        }
        Ok(UserMessage::with_request_id(
            blocks,
            request.request_id.clone(),
        ))
    }

    /// 候选整表预算 20：先文件后对象，文件占满就不再给对象。
    fn trim_candidates(
        references: &mut Vec<FileReferenceEntry>,
        objects: &mut Vec<FileReferenceObject>,
    ) {
        references.truncate(tools::file_references::DEFAULT_MAX_RESULTS);
        let remaining =
            tools::file_references::DEFAULT_MAX_RESULTS.saturating_sub(references.len());
        objects.truncate(remaining);
    }

    /// 列 @ 文件候选：项目默认目录 + 资料库的文字文件，返回绝对路径。
    ///
    /// Kind 没有工作目录；这里不再以进程/根会话 cwd 为根。项目默认目录不存在时跳过该项目。
    pub(crate) async fn file_references_list(
        &self,
        request: FileReferencesListRequest,
    ) -> Result<FileReferencesListResponse, ServiceError> {
        let mut roots = Vec::new();
        if let Some(projects) = &self.options.projects {
            let projects = projects
                .list()
                .map_err(|error| ServiceError::InvalidPath(error.to_string()))?;
            for project in projects {
                if let Some(dir) = project.dir {
                    roots.push(dir);
                }
            }
        }
        // 资料库：现有资料/成果库目录；没有就只是没有候选。
        let library = self.options.data_dir.join("library");
        if library.is_dir() {
            roots.push(library);
        }
        let mut references = Vec::new();
        for root in roots {
            let Ok(items) = tools::file_references::list(&root, &request.query) else {
                continue;
            };
            for item in items {
                references.push(FileReferenceEntry {
                    path: root.join(item.path).to_string_lossy().into_owned(),
                    bytes: item.bytes,
                });
            }
        }
        // 保留每个根自己的排序（同名/前缀/子序列），跨项目只按项目顺序拼接。
        let mut seen = std::collections::BTreeSet::new();
        references.retain(|entry| seen.insert(entry.path.clone()));

        // 同一份候选接口只增对象分组；数据来自已有任务/项目事实，不加新查询协议。
        let needle = request.query.to_lowercase();
        let matched = |text: &str| needle.is_empty() || text.to_lowercase().contains(&needle);
        let mut task_objects = Vec::new();
        let mut report_objects = Vec::new();
        let mut project_objects = Vec::new();
        if let Some(tasks) = self
            .task_runtime()
            .map(|runtime| runtime.store().all_tasks())
            .and_then(Result::ok)
        {
            for task in tasks {
                if matched(task.id.as_str()) || matched(&task.title) {
                    task_objects.push(FileReferenceObject {
                        kind: "task".to_owned(),
                        id: task.id.as_str().to_owned(),
                        title: task.title.clone(),
                        version: Some(task.requirement_version()),
                        task_id: None,
                        goal: None,
                    });
                }
                if let Some(report) = task.latest_report()
                    && (matched(report.id.as_str()) || matched(&task.title))
                {
                    report_objects.push(FileReferenceObject {
                        kind: "report".to_owned(),
                        id: report.id.as_str().to_owned(),
                        title: task.title.clone(),
                        version: Some(report.version),
                        task_id: Some(task.id.as_str().to_owned()),
                        goal: None,
                    });
                }
            }
        }
        if let Some(projects) = &self.options.projects
            && let Ok(projects) = projects.list()
        {
            for project in projects {
                if matched(&project.id) || matched(&project.name) || matched(&project.goal) {
                    project_objects.push(FileReferenceObject {
                        kind: "project".to_owned(),
                        id: project.id.clone(),
                        title: project.name.clone(),
                        version: None,
                        task_id: None,
                        goal: Some(project.goal.clone()),
                    });
                }
            }
        }
        let mut objects = task_objects;
        objects.extend(report_objects);
        objects.extend(project_objects);
        Self::trim_candidates(&mut references, &mut objects);
        Ok(FileReferencesListResponse {
            references,
            objects,
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

    /// 按 id 读回附件（卡片「点开看全文」/图片预览）。
    pub(crate) async fn attachment_get(
        &self,
        request: AttachmentGetRequest,
    ) -> Result<AttachmentGetResponse, ServiceError> {
        let stored = tools::attachments::read(&self.options.data_dir, &request.id)
            .map_err(attachment_error)?;
        let bytes = std::fs::read(&stored.path)
            .map_err(|error| ServiceError::AttachmentInvalid(format!("附件读不回来：{error}")))?;
        let mime = stored.mime.clone();
        // 文字文件给全文；非文字（含 NUL 的合法 UTF-8 二进制）只给二进制 base64。
        let text = if tools::attachments::is_text(&bytes) {
            String::from_utf8(bytes.clone()).ok()
        } else {
            None
        };
        let data = if text.is_none() {
            Some(encode_base64(&bytes))
        } else {
            None
        };
        Ok(AttachmentGetResponse {
            name: stored.name,
            bytes: stored.bytes as u64,
            mime,
            text,
            data,
        })
    }
}

#[cfg(test)]
mod image_format_tests {
    use super::image_format_supported;

    #[test]
    fn only_png_jpeg_gif_webp_are_image_blocks() {
        for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            assert!(image_format_supported(mime), "{mime} 要进图片块");
        }
        for mime in [
            "image/svg+xml",
            "image/bmp",
            "image/tiff",
            "text/plain",
            "application/pdf",
        ] {
            assert!(!image_format_supported(mime), "{mime} 不能进图片块");
        }
    }

    fn reference(index: usize) -> super::FileReferenceEntry {
        super::FileReferenceEntry {
            path: format!("/tmp/file-{index}.txt"),
            bytes: 1,
        }
    }

    fn object(index: usize) -> super::FileReferenceObject {
        super::FileReferenceObject {
            kind: "task".to_owned(),
            id: format!("task-{index}"),
            title: format!("任务 {index}"),
            version: Some(1),
            task_id: None,
            goal: None,
        }
    }

    #[test]
    fn candidates_share_one_budget_of_twenty() {
        // 文件 >20：只剩 20 个文件，对象一个不给。
        let mut references: Vec<_> = (0..25).map(reference).collect();
        let mut objects: Vec<_> = (0..5).map(object).collect();
        super::RootHost::trim_candidates(&mut references, &mut objects);
        assert_eq!(references.len(), 20);
        assert!(objects.is_empty());

        // 文件+对象 >20：文件 15，对象只留 5。
        let mut references: Vec<_> = (0..15).map(reference).collect();
        let mut objects: Vec<_> = (0..10).map(object).collect();
        super::RootHost::trim_candidates(&mut references, &mut objects);
        assert_eq!(references.len(), 15);
        assert_eq!(objects.len(), 5);
        assert_eq!(references.len() + objects.len(), 20);

        // 文件+对象 <20：原样保留。
        let mut references: Vec<_> = (0..5).map(reference).collect();
        let mut objects: Vec<_> = (0..5).map(object).collect();
        super::RootHost::trim_candidates(&mut references, &mut objects);
        assert_eq!(references.len(), 5);
        assert_eq!(objects.len(), 5);
    }
}
