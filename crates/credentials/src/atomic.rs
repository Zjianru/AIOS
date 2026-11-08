//! 原子写文件的唯一出处：根状态、设置、汇率缓存、Keyring 的私有文件都走这里。
//!
//! 语义固定为「临时文件写 + fsync 文件 + rename + fsync 目录」：rename 之前目标文件不会被
//! 破坏；目录 fsync 保证 rename 的持久性。任何一步失败都向上返回，临时文件顺手删掉。
//!
//! 放在 credentials：service 与 credentials 都要原子写，而 credentials 不依赖其他内部 crate、
//! service 已经依赖它——这里是不加新 crate 的最低共用位置。

use std::path::Path;

/// 写到哪种目标（决定权限与临时文件命名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// 状态/缓存文件：按默认 umask。
    Shared,
    /// 私有文件（Keyring 的钥匙与密文）：父目录 0700、文件 0600。
    Private,
}

/// 原子写：临时文件写 + fsync 文件 + rename + fsync 目录。失败向上返回。
pub fn write(_path: &Path, _bytes: &[u8], _kind: TargetKind) -> std::io::Result<()> {
    // 骨架：签名先落地，行为下一笔实现。
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目录里的所有条目（用来确认没留下临时文件）。
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("列目录")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn write_lands_the_bytes_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("settings.json");
        write(&target, b"{\"currency\":\"cny\"}\n", TargetKind::Shared).expect("原子写");
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"currency\":\"cny\"}\n");
        assert_eq!(entries(dir.path()), ["settings.json"], "临时文件不能留下");
    }

    #[cfg(unix)]
    #[test]
    fn private_target_is_0600_in_0700_dir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("keys").join("identity.txt");
        write(&target, b"secret", TargetKind::Private).expect("原子写私有文件");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "私有文件只属主可读写");
        let dir_mode = std::fs::metadata(target.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "私有目录只属主可进");
    }

    #[test]
    fn write_failure_is_returned_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"x").unwrap();
        let target = blocked.join("settings.json");
        let error = write(&target, b"{}", TargetKind::Shared).expect_err("父路径是文件，必须报错");
        assert!(
            error.kind() == std::io::ErrorKind::NotADirectory
                || error.kind() == std::io::ErrorKind::AlreadyExists,
            "要把底层原因带上来：{error}"
        );
        assert_eq!(entries(dir.path()), ["blocked"], "失败不能留下半成品");
    }
}
