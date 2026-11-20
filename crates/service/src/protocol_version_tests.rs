use super::*;

#[test]
fn protocol_version_is_22() {
    assert_eq!(PROTOCOL_VERSION, 22, "日历区间/触发任务事实，协议升到 22");
}
