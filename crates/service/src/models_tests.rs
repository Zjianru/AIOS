use credentials::{CheckState, ConnectionInfo};

#[test]
fn 连接模型为空时默认模型退回目录第一个() {
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
        crate::models::primary_model(dir.path(), &connection),
        "MiniMax-M2",
        "没有显式模型、没有厂商列表、连接模型为空：退回目录第一个"
    );
}
