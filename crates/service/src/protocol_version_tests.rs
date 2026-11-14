use super::*;

#[test]
fn protocol_version_is_12() {
    assert_eq!(
        PROTOCOL_VERSION, 13,
        "work（工作页/「现在」共用工作快照）与组接受同批出，只增方法，协议升到 12"
    );
}
