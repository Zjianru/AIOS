use super::*;

#[test]
fn protocol_version_is_26() {
    assert_eq!(PROTOCOL_VERSION, 26, "T079.1 项目方法，协议升到 26");
}
