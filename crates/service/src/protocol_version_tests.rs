use super::*;

#[test]
fn protocol_version_is_9() {
    assert_eq!(
        PROTOCOL_VERSION, 9,
        "status 带会话 cwd 把协议升到 9（ready 帧带 9；旧页忽略新增字段）"
    );
}
