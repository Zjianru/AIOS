//! T008.F9：产品源码不再保留旧环境变量/静态凭证入口（review #9）。
//!
//! 删除契约用「读产品源码断言旧符号不存在」锁定：`DeepSeekConfig.api_key`、
//! `DeepSeek::new`、`StaticCredential` 与 Agent 组装里的「None 用 api_key」旧路径。

use std::path::PathBuf;

fn repo_file(relative: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(relative)).expect("读产品源码")
}

#[test]
fn product_src_has_no_legacy_credential_path() {
    let model_mod = repo_file("crates/model/src/deepseek/mod.rs");
    assert!(
        !model_mod.contains("StaticCredential"),
        "DeepSeek 适配器不再有 StaticCredential 旧路径"
    );
    assert!(
        !model_mod.contains("fn new(config: DeepSeekConfig)"),
        "DeepSeek::new(config) 旧构造入口要删掉"
    );

    let config = repo_file("crates/model/src/deepseek/config.rs");
    assert!(
        !config.contains("pub api_key: String"),
        "DeepSeekConfig 不再有 api_key 旧字段"
    );

    let credential = repo_file("crates/model/src/credential.rs");
    assert!(
        !credential.contains("StaticCredential"),
        "凭证模块的 StaticCredential 旧实现要删掉"
    );

    let assembly = repo_file("crates/agent/src/assembly.rs");
    assert!(
        !assembly.contains("DeepSeek::new("),
        "Agent 组装不再回退到 DeepSeek::new 旧路径"
    );
    assert!(
        !assembly.contains("None 用 DeepSeekConfig.api_key"),
        "组装层不再保留旧路径说明"
    );

    let cli = repo_file("crates/cli/src/main.rs");
    assert!(
        !cli.contains("api_key: String,"),
        "CLI 的 config_from_args 不再带 api_key 参数"
    );
}
