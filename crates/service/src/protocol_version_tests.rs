use super::*;

#[test]
fn protocol_version_is_19() {
    assert_eq!(
        PROTOCOL_VERSION, 19,
        "日历读取/动作、changed 与 sourceTurn，协议升到 19"
    );
}
