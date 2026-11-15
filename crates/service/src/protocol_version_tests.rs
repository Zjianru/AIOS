use super::*;

#[test]
fn protocol_version_is_15() {
    assert_eq!(
        PROTOCOL_VERSION, 15,
        "交付记录（T058）新 TaskDelivery 请求，协议升到 15"
    );
}
