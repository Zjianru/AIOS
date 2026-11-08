//! cli 集成测试共用：被测二进制路径。

pub fn aios_dev() -> &'static str {
    env!("CARGO_BIN_EXE_aios-dev")
}
