//! 发给模型前的图片投影（T100）。
//!
//! 会话日志里的 Image 块不动；这里只在请求副本上把主模型看不了的图换成会话旁注里
//! 冻结的文字。纯函数，方便逐字节测前缀稳定。

use std::collections::BTreeMap;

use crate::{ContentBlock, Message};

/// 把请求副本里的图片块投影成文字。
///
/// - supports_images 为真：原图直通，不做任何替换。
/// - 有旁注：逐字使用旁注；连续图片的旁注文字相同（omitted 合并行）只保留第一份。
/// - 缺旁注：连续缺旁注的图片合成一行 D120，附件 id 全部列出。
pub fn project_images(
    messages: &[Message],
    supports_images: bool,
    notes: &BTreeMap<String, String>,
) -> Vec<Message> {
    if supports_images {
        return messages.to_vec();
    }
    messages
        .iter()
        .map(|message| {
            let mut projected = Vec::with_capacity(message.content.len());
            let mut missing: Vec<String> = Vec::new();
            let mut last_projected_text: Option<String> = None;
            for block in &message.content {
                match block {
                    ContentBlock::Image { attachment_id, .. } => {
                        if let Some(note) = notes.get(attachment_id) {
                            if !missing.is_empty() {
                                let ids = std::mem::take(&mut missing);
                                let text = crate::image_omitted_text(&ids);
                                projected.push(ContentBlock::text(text));
                            }
                            if last_projected_text.as_deref() != Some(note.as_str()) {
                                projected.push(ContentBlock::text(note.clone()));
                                last_projected_text = Some(note.clone());
                            }
                        } else {
                            missing.push(attachment_id.clone());
                        }
                    }
                    other => {
                        if !missing.is_empty() {
                            let ids = std::mem::take(&mut missing);
                            let text = crate::image_omitted_text(&ids);
                            projected.push(ContentBlock::text(text));
                        }
                        projected.push(other.clone());
                        last_projected_text = None;
                    }
                }
            }
            if !missing.is_empty() {
                let text = crate::image_omitted_text(&missing);
                projected.push(ContentBlock::text(text));
            }
            let mut clone = message.clone();
            clone.content = projected;
            clone
        })
        .collect()
}
