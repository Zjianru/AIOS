//! 原子写文件的唯一出处：根状态、设置、汇率缓存、Keyring 的私有文件都走这里。
//!
//! 语义固定为「临时文件写 + fsync 文件 + rename + fsync 目录」：rename 之前目标文件不会被
//! 破坏；目录 fsync 保证 rename 的持久性。任何一步失败都向上返回，临时文件顺手删掉。
//!
//! 放在 credentials：service 与 credentials 都要原子写，而 credentials 不依赖其他内部 crate、
//! service 已经依赖它——这里是不加新 crate 的最低共用位置。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 写到哪种目标（决定权限与临时文件命名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// 状态/缓存文件：按默认 umask；临时文件沿用现码的 `<路径>.tmp`。
    Shared,
    /// 私有文件（Keyring 的钥匙与密文）：父目录 0700、文件 0600；临时文件带进程与序号
    /// ——两个进程可能同时写同一份私有文件，固定名会互相踩。
    Private,
}

/// 私有文件临时名的序号（同一进程里连写同名文件也不撞）。
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 原子写：临时文件写 + fsync 文件 + rename + fsync 目录。失败向上返回。
pub fn write(path: &Path, bytes: &[u8], kind: TargetKind) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} 没有父目录", path.display()),
        )
    })?;
    match kind {
        TargetKind::Shared => fs::create_dir_all(parent)?,
        TargetKind::Private => create_private_dir(parent)?,
    }
    let temp = temp_path(path, kind);
    match publish(&temp, path, parent, bytes, kind) {
        Ok(()) => Ok(()),
        Err(error) => {
            // 半成品不能留着（下一次写会撞名，读的人也会误以为它是目标文件）。
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// 建私有目录：缺失的逐级建，已存在的把权限修正成 0700（升级/续接的旧数据目录）。
pub(crate) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// 写临时文件、fsync、rename、fsync 目录。任何一步失败原样返回。
fn publish(
    temp: &Path,
    path: &Path,
    parent: &Path,
    bytes: &[u8],
    kind: TargetKind,
) -> std::io::Result<()> {
    if kind == TargetKind::Shared {
        // 固定临时名：上一次崩在 rename 之前留下的文件当陈旧文件清掉；清不掉就让 create_new 报错。
        let _ = fs::remove_file(temp);
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if kind == TargetKind::Private {
            options.mode(0o600);
        }
    }
    let mut file = options.open(temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    if kind == TargetKind::Private {
        use std::os::unix::fs::PermissionsExt;
        // 创建时的 mode 也要过 umask；这里再钉一次，保证最终就是 0600。
        fs::set_permissions(temp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(temp, path)?;
    // rename 的持久性靠目录 fsync；失败就是失败，不再 best effort。
    fs::File::open(parent)?.sync_all()
}

fn temp_path(path: &Path, kind: TargetKind) -> PathBuf {
    let suffix = match kind {
        // 状态/缓存文件沿用现码的临时名。
        TargetKind::Shared => String::from(".tmp"),
        TargetKind::Private => {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            format!(".tmp-{}-{sequence}", std::process::id())
        }
    };
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
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
