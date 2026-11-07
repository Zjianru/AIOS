use super::*;

#[test]
fn protocol_version_is_8() {
    assert_eq!(
        PROTOCOL_VERSION, 8,
        "模型屏蔽把协议升到 8（ready 帧带 8；旧页忽略新增字段）"
    );
}
