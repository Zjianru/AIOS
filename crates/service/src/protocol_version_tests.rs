use super::*;

#[test]
fn protocol_version_is_20() {
    assert_eq!(PROTOCOL_VERSION, 20, "日历区间/触发任务事实，协议升到 20");
}
