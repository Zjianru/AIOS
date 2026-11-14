use super::*;

#[test]
fn protocol_version_is_11() {
    assert_eq!(
        PROTOCOL_VERSION, 11,
        "send 加 resultCard（结果卡进 Kind 输入框）、消息内容块加 result-card，只增字段/块，协议升到 11"
    );
}
