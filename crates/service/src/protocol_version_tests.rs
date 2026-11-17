use super::*;

#[test]
fn protocol_version_is_17() {
    assert_eq!(
        PROTOCOL_VERSION, 17,
        "工作快照加 graphs（服务算线），协议升到 17"
    );
}
