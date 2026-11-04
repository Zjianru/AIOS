use model::AssistantStreamRecord;

/// 把紧凑流记录编码成 Messages SSE 帧（每个元素是一整帧，含 event/data 和空行）。
///
/// skeleton：先返回空（红测试后实现）。
pub fn encode_sse(_records: &[AssistantStreamRecord]) -> Vec<String> {
    Vec::new()
}
