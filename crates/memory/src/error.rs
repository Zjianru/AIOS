use std::path::PathBuf;

/// 记忆库的错误；库内部只暴露「打不开、版本不对、被拒、没有这条」这几类。
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("记忆库的 sqlite 出了错：{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("记忆库的目录建不出来：{0}")]
    Io(#[from] std::io::Error),
    #[error("记忆库版本 {found} 比程序支持的 {supported} 新")]
    SchemaTooNew { found: u64, supported: u64 },
    #[error("记忆库版本 {found} 没有迁移到 {supported} 的路")]
    SchemaUnsupported { found: u64, supported: u64 },
    #[error("记忆库的配置不对：{0}")]
    StorageConfig(String),
    #[error("这个身份不能做这个记忆写入：{0}")]
    Refused(&'static str),
    #[error("没有这条记忆：{0}")]
    NotFound(String),
    #[error("记忆库设置在 {0} 上写失败")]
    Path(PathBuf),
}
