//! T008.F9：产品源码不再保留旧环境变量/静态凭证入口（review #9）。
//!
//! 删除契约用「读产品源码断言旧符号不存在」锁定：`DeepSeekConfig.api_key`、
//! `DeepSeek::new`、`StaticCredential` 与 Agent 组装里的「None 用 api_key」旧路径。
//! 骨架阶段先只读源码（行为不变）；红阶段填断言；修复阶段删旧路径后变绿。

use std::path::PathBuf;

fn repo_file(relative: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(relative)).expect("读产品源码")
}

#[test]
fn product_src_has_no_legacy_credential_path() {
    // 骨架：只确认能读产品源码；断言在红阶段补。
    assert!(repo_file("crates/model/src/deepseek/mod.rs").contains("pub struct DeepSeek"));
}
