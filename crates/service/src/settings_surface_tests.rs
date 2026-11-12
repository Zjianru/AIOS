#[test]
fn auth_header_writeback_failure_names_connection_and_reason() {
    let text =
        crate::settings_surface::auth_header_writeback_failure("minimax-cn", "keyring/io: 磁盘满");
    assert_eq!(
        text,
        "连接 minimax-cn 的鉴权头没写回 Keyring：keyring/io: 磁盘满"
    );
    assert!(!text.contains('\n'), "日志只占一行：{text}");
}
