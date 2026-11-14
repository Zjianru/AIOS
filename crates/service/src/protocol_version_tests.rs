use super::*;

#[test]
fn protocol_version_is_14() {
    assert_eq!(
        PROTOCOL_VERSION, 14,
        "程序门（T057）与任务页检查事实只增字段，协议升到 14"
    );
}
