use super::*;

#[test]
fn protocol_version_is_18() {
    assert_eq!(
        PROTOCOL_VERSION, 18,
        "日历读取/动作与 calendar/changed，协议升到 18"
    );
}
