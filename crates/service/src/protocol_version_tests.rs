use super::*;

#[test]
fn protocol_version_is_16() {
    assert_eq!(
        PROTOCOL_VERSION, 16,
        "记忆页新 MemoryRead/MemoryAction 请求与 memory/changed 推送，协议升到 16"
    );
}
