//! T013.2：附件仓库（文字文件的只读副本）。
//!
//! 照 DSH `fs/tool-fs/src/file-store.ts` + `admission.ts`：
//! - `<数据目录>/attachments/v1/` 下按 sha256 存一份内容（file-objects/<前2>/<sha>），再按文件名给只读副本（files/<前2>/<sha>/<name>）；
//! - 文字判断照 git：前 8000 字节里有 NUL 就不是文字，拒收（错误码 not-text）；
//! - 不设大小/个数上限，不做清理（日志里还引用着）。

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use ring::digest::{SHA256, digest};

/// 文字判断的探测长度（照 git `xdiff-interface.c` 的 FIRST_FEW_BYTES）。
pub const TEXT_PROBE_BYTES: usize = 8000;

/// 拒收二进制文件的错误码（服务/页面按码处理）。
pub const NOT_TEXT_CODE: &str = "not-text";

/// 附件仓库根（数据目录下；照 DSH file-store 的 attachments/v1）。
fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("attachments").join("v1")
}

/// 存好的一份附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttachment {
    /// `sha256:<64 位小写 hex>`。
    pub id: String,
    pub name: String,
    /// 内容字节数。
    pub bytes: usize,
    /// 只读副本的绝对路径（模型层投递 fileHandleText 用）。
    pub path: PathBuf,
}

/// 附件仓库的错误。
#[derive(Debug)]
pub enum AttachmentError {
    /// 前 8000 字节里出现 NUL：不是文字文件。
    NotText,
    /// 文件名不合法（空、含路径分隔符、. / ..、超长）。
    BadName(String),
    /// id 不合法（只认 sha256:<64 位小写 hex>）。
    BadId(String),
    Io(std::io::Error),
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotText => formatter.write_str("附件不是文字文件"),
            Self::BadName(name) => write!(formatter, "附件名不合法：{name}"),
            Self::BadId(id) => write!(formatter, "附件 id 不合法：{id}"),
            Self::Io(error) => write!(formatter, "附件读写失败：{error}"),
        }
    }
}

impl std::error::Error for AttachmentError {}

impl From<std::io::Error> for AttachmentError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// 前 8000 字节里有没有 NUL（照 git：有就不是文字）。
pub fn is_text(data: &[u8]) -> bool {
    let probe = &data[..data.len().min(TEXT_PROBE_BYTES)];
    !probe.contains(&0)
}

fn validate_name(name: &str) -> Result<(), AttachmentError> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0');
    if bad {
        return Err(AttachmentError::BadName(name.to_owned()));
    }
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = digest(&SHA256, data);
    let mut hex = String::with_capacity(64);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").expect("写入 String 不会失败");
    }
    hex
}

/// 写一个只读文件（内容对象与命名副本都是 0444）。
fn write_read_only(path: &Path, data: &[u8]) -> Result<(), AttachmentError> {
    let mut file = fs::File::create(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_mode(0o444);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

/// 存一份附件：内容按 sha256 存一份，再给只读命名副本。返回 id/name/bytes/路径。
pub fn store(
    data_dir: &Path,
    name: &str,
    data: &[u8],
) -> Result<StoredAttachment, AttachmentError> {
    validate_name(name)?;
    if !is_text(data) {
        return Err(AttachmentError::NotText);
    }
    let hex = sha256_hex(data);
    let root = root(data_dir);
    let object = root.join("file-objects").join(&hex[..2]).join(&hex);
    if !object.exists() {
        if let Some(parent) = object.parent() {
            fs::create_dir_all(parent)?;
        }
        write_read_only(&object, data)?;
    }
    let alias_dir = root.join("files").join(&hex[..2]).join(&hex);
    fs::create_dir_all(&alias_dir)?;
    let alias = alias_dir.join(name);
    if !alias.exists() {
        write_read_only(&alias, data)?;
    }
    Ok(StoredAttachment {
        id: format!("sha256:{hex}"),
        name: name.to_owned(),
        bytes: data.len(),
        path: alias,
    })
}

/// 读回一个附件（按 id；名字取只读副本目录里的文件名）。
pub fn read(data_dir: &Path, id: &str) -> Result<StoredAttachment, AttachmentError> {
    let Some(hex) = id.strip_prefix("sha256:") else {
        return Err(AttachmentError::BadId(id.to_owned()));
    };
    let valid = hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return Err(AttachmentError::BadId(id.to_owned()));
    }
    let root = root(data_dir);
    let object = root.join("file-objects").join(&hex[..2]).join(hex);
    if !object.exists() {
        return Err(AttachmentError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("附件不存在：{id}"),
        )));
    }
    let alias_dir = root.join("files").join(&hex[..2]).join(hex);
    let mut name = String::new();
    let mut path = object.clone();
    if let Ok(mut entries) = fs::read_dir(&alias_dir)
        && let Some(entry) = entries.next().transpose()?
    {
        name = entry.file_name().to_string_lossy().into_owned();
        path = entry.path();
    }
    let bytes = fs::metadata(&object)?.len() as usize;
    Ok(StoredAttachment {
        id: id.to_owned(),
        name,
        bytes,
        path,
    })
}
