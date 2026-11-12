use super::*;

#[test]
fn protocol_version_is_10() {
    assert_eq!(
        PROTOCOL_VERSION, 10,
        "删了一个协议方法（只删不是只增），协议升到 10"
    );
}
