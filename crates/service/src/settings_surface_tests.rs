use credentials::{CheckState, ConnectionInfo};

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

#[test]
fn 验证模型为空时退回目录第一个() {
    // 保存入口拒绝空 model；这条兜底路径给旧存储/外部写入的坏数据。
    model::catalog::set_global(model::catalog::snapshot());
    let dir = tempfile::tempdir().unwrap();
    let connection = ConnectionInfo {
        id: "minimax-cn".to_owned(),
        name: "MiniMax".to_owned(),
        model: String::new(),
        endpoint: "https://api.minimaxi.com/anthropic".to_owned(),
        saved_at: 1,
        check: CheckState::Never,
        key_saved: true,
        preset: "minimax-cn".to_owned(),
        models: Some(Vec::new()),
        auth_header: None,
    };
    assert_eq!(
        crate::settings_surface::verify_model(dir.path(), &connection),
        "MiniMax-M2",
        "连接没有选中模型时，验证沿用目录第一个"
    );
}
