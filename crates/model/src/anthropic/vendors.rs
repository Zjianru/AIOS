//! T012.2：厂商之间的差别集中在这个文件。
//!
//! 只有一套 Anthropic Messages 实现；厂商差异按地址/模型名匹配后写在这里。
//! 本票只把现有的思考序列化差异搬进来（纯搬家，行为不变）；后续票再加
//! always_thinks / thinking_needed / budget_of / 余额和余量地址。

use crate::Effort;

/// 这次请求要不要发 thinking.enabled；关思考只发 disabled（现有 DeepSeek 行为）。
pub(crate) fn thinking_enabled(effort: &Effort) -> bool {
    !effort.is_off()
}
