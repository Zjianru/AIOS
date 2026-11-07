use super::*;

#[test]
fn protocol_version_is_7() {
    assert_eq!(
        PROTOCOL_VERSION, 7,
        "T015.5：tool-call-delta 与 send.canvas 让协议升到 7"
    );
}
