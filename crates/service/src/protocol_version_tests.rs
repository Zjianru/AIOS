use super::*;

#[test]
fn protocol_version_is_24() {
    assert_eq!(PROTOCOL_VERSION, 24, "T077.3 取消事实，协议升到 24");
}
