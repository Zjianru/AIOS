use super::*;

#[test]
fn protocol_version_is_23() {
    assert_eq!(PROTOCOL_VERSION, 23, "监控快照/任务诊断，协议升到 23");
}
