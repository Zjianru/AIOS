use super::*;

#[test]
fn protocol_version_is_25() {
    assert_eq!(PROTOCOL_VERSION, 25, "T077.5 动作入口，协议升到 25");
}
