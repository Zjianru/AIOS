//! 文件系统支持：realpath target key、版本 token、原子写和观察记录。
//!
//! 语义照 DSH 的 packages/fs/fs-local（原子发布、版本、行尾）与
//! packages/fs/fs-observation-policy（编辑前先读）。只实现本机 POSIX 路径（D19 只 macOS）。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) mod observation;
pub(crate) mod patch;

use crate::fs::observation::WriteIntent;
use crate::session::FileVersion;
use crate::{ToolContext, ToolFailure};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 解析后的写/读目标：`key` 是 realpath，观察表和版本 token 都以它作身份。
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub key: PathBuf,
}

impl Target {
    pub(crate) fn display(&self) -> String {
        self.key.display().to_string()
    }
}

/// D3：存在路径 canonicalize；缺失路径用最近存在祖先 canonicalize + 缺失后缀。
/// 符号链接环、权限等 canonicalize 失败按 FS 错误返回，不静默退回绝对路径。
pub(crate) fn resolve(context: &ToolContext, requested: &str) -> Result<Target, ToolFailure> {
    let path = Path::new(requested);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        context.cwd.join(path)
    };
    match fs::canonicalize(&absolute) {
        Ok(real) => return Ok(Target { key: real }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => {
            return Err(ToolFailure::coded(
                "FS_NOT_FOUND",
                format!(
                    "cannot resolve \"{}\": a parent path segment is not a directory",
                    absolute.display()
                ),
            ));
        }
        Err(error) => {
            return Err(io_failure("cannot resolve", &absolute, &error));
        }
    }
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut current = absolute.clone();
    loop {
        match fs::canonicalize(&current) {
            Ok(real) => {
                let mut key = real;
                for part in missing.iter().rev() {
                    key.push(part);
                }
                return Ok(Target { key });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = current.file_name().map(std::ffi::OsStr::to_os_string) else {
                    return Err(ToolFailure::coded(
                        "FS_NOT_FOUND",
                        format!("cannot resolve \"{}\": not found", absolute.display()),
                    ));
                };
                missing.push(name);
                let Some(parent) = current.parent().map(Path::to_path_buf) else {
                    return Err(ToolFailure::coded(
                        "FS_NOT_FOUND",
                        format!("cannot resolve \"{}\": not found", absolute.display()),
                    ));
                };
                if parent == current {
                    return Err(ToolFailure::coded(
                        "FS_NOT_FOUND",
                        format!("cannot resolve \"{}\": not found", absolute.display()),
                    ));
                }
                current = parent;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => {
                return Err(ToolFailure::coded(
                    "FS_NOT_FOUND",
                    format!(
                        "cannot resolve \"{}\": a parent path segment is not a directory",
                        absolute.display()
                    ),
                ));
            }
            Err(error) => return Err(io_failure("cannot resolve", &current, &error)),
        }
    }
}

/// 一次 stat 的结果：版本 token + mode + 是否普通文件。
#[derive(Debug, Clone)]
pub(crate) struct FileInfo {
    pub version: FileVersion,
    pub mode: u32,
    pub is_file: bool,
}

/// 探测目标（跟随符号链接）；不存在返回 None。
pub(crate) fn probe(target: &Target) -> Result<Option<FileInfo>, ToolFailure> {
    match fs::metadata(&target.key) {
        Ok(metadata) => Ok(Some(FileInfo {
            version: version_of(&metadata),
            mode: mode_of(&metadata),
            is_file: metadata.is_file(),
        })),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.kind() == std::io::ErrorKind::NotADirectory =>
        {
            Ok(None)
        }
        Err(error) => Err(io_failure("cannot stat", &target.key, &error)),
    }
}

/// 登记一次「读过、且是这个版本」的观察（观察策略模块，T006.6）。
pub(crate) fn record_present(context: &ToolContext, target: &Target, version: FileVersion) {
    context
        .session()
        .observations()
        .observe_present(target.key.clone(), version);
}

/// 写策略：未见或已确认缺失 -> createIfAbsent；见过 present -> replaceIfVersion。
pub(crate) fn write_intent(context: &ToolContext, target: &Target) -> WriteIntent {
    context.session().observations().write_intent(&target.key)
}

/// edit/delete 的版本护栏：没见过 -> FS_NOT_OBSERVED；确认缺失 -> FS_NOT_FOUND。
pub(crate) fn edit_version(
    context: &ToolContext,
    target: &Target,
    verb: &str,
) -> Result<FileVersion, ToolFailure> {
    context
        .session()
        .observations()
        .edit_intent(&target.key, &target.display(), verb)
}

/// 登记一次「确认不存在」的观察（read 找不到时）。
pub(crate) fn record_absent(context: &ToolContext, target: &Target) {
    context
        .session()
        .observations()
        .observe_absent(target.key.clone());
}

/// DSH 的稳定模型可见文案：FS_NOT_OBSERVED 换成通用「先读」原因，
/// FS_STALE_VERSION 保留原原因并追加「重读后重试」。
pub(crate) fn remediate(failure: ToolFailure, target: &Target) -> ToolFailure {
    match failure.code.as_deref() {
        Some("FS_NOT_OBSERVED") => ToolFailure::coded(
            "FS_NOT_OBSERVED",
            format!(
                "cannot modify \"{}\": file has not been read — read the file, then retry",
                target.display()
            ),
        ),
        Some("FS_STALE_VERSION") => ToolFailure {
            code: Some("FS_STALE_VERSION".to_owned()),
            message: format!("{} — re-read the file, then retry", failure.message),
        },
        _ => failure,
    }
}

/// 整文件写入：先探测现有文件决定 create/replace，再走 D1 的原子发布。
/// 返回是否创建（create vs update，供信封用）。
fn write_file_raw(
    context: &ToolContext,
    target: &Target,
    content: &str,
) -> Result<bool, ToolFailure> {
    let intent = write_intent(context, target);
    let existing = probe(target)?;
    if let Some(info) = &existing
        && !info.is_file
    {
        return Err(ToolFailure::coded(
            "FS_NOT_REGULAR_FILE",
            format!("cannot write \"{}\": not a regular file", target.display()),
        ));
    }
    let (mode, create_if_absent) = match &intent {
        WriteIntent::ReplaceIfVersion(version) => {
            let Some(info) = &existing else {
                return Err(ToolFailure::coded(
                    "FS_STALE_VERSION",
                    format!(
                        "cannot write \"{}\": file no longer exists",
                        target.display()
                    ),
                ));
            };
            if &info.version != version {
                return Err(ToolFailure::coded(
                    "FS_STALE_VERSION",
                    format!(
                        "cannot write \"{}\": file changed since it was read",
                        target.display()
                    ),
                ));
            }
            (Some(info.mode), false)
        }
        WriteIntent::CreateIfAbsent => {
            if existing.is_some() {
                return Err(ToolFailure::coded(
                    "FS_NOT_OBSERVED",
                    format!(
                        "cannot overwrite existing \"{}\" without reading it first",
                        target.display()
                    ),
                ));
            }
            (None, true)
        }
    };
    let created = create_if_absent;
    atomic_write(&target.key, target, content, mode, create_if_absent)?;
    let after = probe(target)?.ok_or_else(|| {
        ToolFailure::coded(
            "FS_IO_ERROR",
            format!(
                "cannot write \"{}\": file missing after write",
                target.display()
            ),
        )
    })?;
    record_present(context, target, after.version);
    Ok(created)
}

/// D1 最小 POSIX 原子写：同目录 temp -> fsync -> rename；失败清理 temp。
fn atomic_write(
    key: &Path,
    target: &Target,
    content: &str,
    mode: Option<u32>,
    create_if_absent: bool,
) -> Result<(), ToolFailure> {
    let Some(parent) = key.parent() else {
        return Err(ToolFailure::coded(
            "FS_IO_ERROR",
            format!("cannot write \"{}\": no parent directory", target.display()),
        ));
    };
    if let Err(error) = fs::create_dir_all(parent) {
        return Err(io_failure("cannot create parent of", &target.key, &error));
    }
    let temp = parent.join(temp_name(key));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&temp) {
        Ok(file) => file,
        Err(error) => return Err(io_failure("cannot write", &target.key, &error)),
    };
    // temp 显式 0600（DSH fsio.ts:652-653 的语义；没有私有 staging 目录也保持只属主可读）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::set_permissions(&temp, fs::Permissions::from_mode(0o600)) {
            return Err(cleanup_failure(
                io_failure("cannot write", &target.key, &error),
                &temp,
            ));
        }
    }
    let written = (|| -> std::io::Result<()> {
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = written {
        return Err(cleanup_failure(
            io_failure("cannot write", &target.key, &error),
            &temp,
        ));
    }
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::set_permissions(&temp, fs::Permissions::from_mode(mode)) {
            return Err(cleanup_failure(
                io_failure("cannot write", &target.key, &error),
                &temp,
            ));
        }
    }
    let published = if create_if_absent {
        match fs::hard_link(&temp, key) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // 并发创建者赢了：区分目录/其他（不能写）与普通文件（未读不能覆盖）。
                let existing = probe(target)?;
                if existing.as_ref().is_some_and(|info| !info.is_file) {
                    Err(ToolFailure::coded(
                        "FS_NOT_REGULAR_FILE",
                        format!("cannot write \"{}\": not a regular file", target.display()),
                    ))
                } else {
                    Err(ToolFailure::coded(
                        "FS_NOT_OBSERVED",
                        format!(
                            "cannot overwrite existing \"{}\" without reading it first",
                            target.display()
                        ),
                    ))
                }
            }
            Err(error) => Err(io_failure("cannot write", &target.key, &error)),
        }
    } else {
        fs::rename(&temp, key).map_err(|error| io_failure("cannot write", &target.key, &error))
    };
    match published {
        Ok(()) => {
            let _ = fs::remove_file(&temp);
            Ok(())
        }
        Err(failure) => Err(cleanup_failure(failure, &temp)),
    }
}

fn temp_name(key: &Path) -> std::ffi::OsString {
    let name = key
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".{name}.aios-{}-{counter}.tmp", std::process::id()).into()
}

/// 失败路径清理 temp；清理也失败时把两件事都报出来，不吞掉。
fn cleanup_failure(failure: ToolFailure, temp: &Path) -> ToolFailure {
    match fs::remove_file(temp) {
        Ok(()) => failure,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => failure,
        Err(error) => ToolFailure {
            code: failure.code.clone(),
            message: format!(
                "{}（清理临时文件 {} 失败：{error}）",
                failure.message,
                temp.display()
            ),
        },
    }
}

pub(crate) fn io_failure(verb: &str, path: &Path, error: &std::io::Error) -> ToolFailure {
    let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
        "FS_PERMISSION_DENIED"
    } else {
        "FS_IO_ERROR"
    };
    ToolFailure::coded(code, format!("{verb} \"{}\": {error}", path.display()))
}

/// D7：版本 token = dev:ino:size:mtimeNs:ctimeNs（unix 元数据；非 unix 未实现，D19 只 macOS）。
fn version_of(metadata: &fs::Metadata) -> FileVersion {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mtime = metadata
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(metadata.mtime_nsec());
        let ctime = metadata
            .ctime()
            .saturating_mul(1_000_000_000)
            .saturating_add(metadata.ctime_nsec());
        FileVersion::new(format!(
            "{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            mtime,
            ctime
        ))
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        FileVersion::new(format!("{}:{modified}", metadata.len()))
    }
}

fn mode_of(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}
// --- 编辑 / 行尾 ---

/// 编辑用的文本：内容已做 LF 归一，行尾风格留待写回恢复。
pub(crate) struct TextFile {
    pub content: String,
    pub line_endings: LineEndings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineEndings {
    Lf,
    Crlf,
}

/// 读 + 解码编辑目标：二进制/非法 UTF-8 按 FS_NOT_TEXT 拒绝，内容 LF 归一。
pub(crate) fn read_for_edit(target: &Target) -> Result<TextFile, ToolFailure> {
    read_text_for_verb(target, "edit")
}

fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n")
}

/// 行尾风格按前 4096 字符里 CRLF 与 LF 的数量取多数（照 DSH detectLineEndings）。
fn detect_line_endings(raw: &str) -> LineEndings {
    let sample: String = raw.chars().take(4096).collect();
    let crlf = sample.matches("\r\n").count();
    let lf = sample.matches('\n').count() - crlf;
    if crlf > lf {
        LineEndings::Crlf
    } else {
        LineEndings::Lf
    }
}

fn restore_line_endings(content: &str, style: LineEndings) -> String {
    match style {
        LineEndings::Lf => content.to_owned(),
        LineEndings::Crlf => normalize_line_endings(content).replace('\n', "\r\n"),
    }
}

/// 字面替换（LF 归一后匹配）：0 次 FS_EDIT_NOT_FOUND，多次且非 replace_all FS_AMBIGUOUS_EDIT。
pub(crate) fn apply_literal_edit(
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    display: &str,
) -> Result<String, ToolFailure> {
    let old = normalize_line_endings(old_string);
    if old.is_empty() {
        return Err(ToolFailure::coded(
            "FS_EDIT_NOT_FOUND",
            "old_string must be a non-empty string",
        ));
    }
    let new = normalize_line_endings(new_string);
    let matched = content.matches(old.as_str()).count();
    if matched == 0 {
        return Err(ToolFailure::coded(
            "FS_EDIT_NOT_FOUND",
            format!("old_string was not found in \"{display}\""),
        ));
    }
    if !replace_all && matched > 1 {
        return Err(ToolFailure::coded(
            "FS_AMBIGUOUS_EDIT",
            format!(
                "old_string matched {matched} times in \"{display}\"; provide a more specific old_string or set replace_all to true"
            ),
        ));
    }
    Ok(content.replace(old.as_str(), new.as_str()))
}

/// edit 的完整流程：版本护栏 -> 读文件 -> 字面替换 -> 恢复原行尾原子写回。
pub(crate) fn edit_file(
    context: &ToolContext,
    target: &Target,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<(), ToolFailure> {
    edit_file_raw(context, target, old_string, new_string, replace_all)
        .map_err(|failure| remediate(failure, target))
}

fn edit_file_raw(
    context: &ToolContext,
    target: &Target,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<(), ToolFailure> {
    let expected = edit_version(context, target, "edit")?;
    let Some(info) = probe(target)? else {
        return Err(ToolFailure::coded(
            "FS_STALE_VERSION",
            format!(
                "cannot edit \"{}\": file changed since it was read",
                target.display()
            ),
        ));
    };
    if !info.is_file {
        return Err(ToolFailure::coded(
            "FS_NOT_REGULAR_FILE",
            format!("cannot edit \"{}\": not a regular file", target.display()),
        ));
    }
    if info.version != expected {
        return Err(ToolFailure::coded(
            "FS_STALE_VERSION",
            format!(
                "cannot edit \"{}\": file changed since it was read",
                target.display()
            ),
        ));
    }
    let text = read_for_edit(target)?;
    let after = apply_literal_edit(
        &text.content,
        old_string,
        new_string,
        replace_all,
        &target.display(),
    )?;
    let restored = restore_line_endings(&after, text.line_endings);
    atomic_write(&target.key, target, &restored, Some(info.mode), false)?;
    let after_info = probe(target)?.ok_or_else(|| {
        ToolFailure::coded(
            "FS_IO_ERROR",
            format!(
                "cannot edit \"{}\": file missing after write",
                target.display()
            ),
        )
    })?;
    record_present(context, target, after_info.version);
    Ok(())
}

/// 高层的写入口：失败文案统一做 DSH 的稳定化（FS_NOT_OBSERVED / FS_STALE_VERSION）。
pub(crate) fn write_file(
    context: &ToolContext,
    target: &Target,
    content: &str,
) -> Result<bool, ToolFailure> {
    write_file_raw(context, target, content).map_err(|failure| remediate(failure, target))
}

/// 读一个可变目标的文本（带版本护栏和 verb 相关文案），供 edit/patch 用。
pub(crate) fn read_guarded_text(
    context: &ToolContext,
    target: &Target,
    verb: &str,
) -> Result<(TextFile, FileInfo), ToolFailure> {
    let expected = edit_version(context, target, verb)?;
    let Some(info) = probe(target)? else {
        return Err(ToolFailure::coded(
            "FS_STALE_VERSION",
            format!(
                "cannot {verb} \"{}\": file changed since it was read",
                target.display()
            ),
        ));
    };
    if !info.is_file {
        return Err(ToolFailure::coded(
            "FS_NOT_REGULAR_FILE",
            format!("cannot {verb} \"{}\": not a regular file", target.display()),
        ));
    }
    if info.version != expected {
        return Err(ToolFailure::coded(
            "FS_STALE_VERSION",
            format!(
                "cannot {verb} \"{}\": file changed since it was read",
                target.display()
            ),
        ));
    }
    let text = read_text_for_verb(target, verb)?;
    Ok((text, info))
}

/// 读文本并 LF 归一（二进制/非法 UTF-8 按 verb 文案拒绝）。
pub(crate) fn read_text_for_verb(target: &Target, verb: &str) -> Result<TextFile, ToolFailure> {
    let bytes = fs::read(&target.key).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ToolFailure::coded(
            "FS_STALE_VERSION",
            format!(
                "cannot {verb} \"{}\": file changed since it was read",
                target.display()
            ),
        ),
        _ => io_failure("cannot read", &target.key, &error),
    })?;
    if bytes.contains(&0) {
        return Err(ToolFailure::coded(
            "FS_NOT_TEXT",
            format!("cannot {verb} \"{}\": binary file", target.display()),
        ));
    }
    let raw = String::from_utf8(bytes).map_err(|_| {
        ToolFailure::coded(
            "FS_NOT_TEXT",
            format!("cannot {verb} \"{}\": invalid UTF-8 text", target.display()),
        )
    })?;
    let line_endings = detect_line_endings(&raw);
    Ok(TextFile {
        content: normalize_line_endings(&raw),
        line_endings,
    })
}

/// patch 的单个文件更新：版本护栏 -> 读 -> 应用 hunk -> 恢复原行尾写回。
pub(crate) fn patch_file(
    context: &ToolContext,
    target: &Target,
    hunks: &[patch::Hunk],
) -> Result<(), ToolFailure> {
    patch_file_raw(context, target, hunks).map_err(|failure| remediate(failure, target))
}

fn patch_file_raw(
    context: &ToolContext,
    target: &Target,
    hunks: &[patch::Hunk],
) -> Result<(), ToolFailure> {
    let (text, info) = read_guarded_text(context, target, "patch")?;
    let after = patch::apply_hunks(&text.content, hunks).map_err(|failure| {
        ToolFailure::coded(
            "FS_PATCH_MISMATCH",
            patch_mismatch_message(target, &failure),
        )
    })?;
    let restored = restore_line_endings(&after, text.line_endings);
    atomic_write(&target.key, target, &restored, Some(info.mode), false)?;
    let after_info = probe(target)?.ok_or_else(|| {
        ToolFailure::coded(
            "FS_IO_ERROR",
            format!(
                "cannot patch \"{}\": file missing after write",
                target.display()
            ),
        )
    })?;
    record_present(context, target, after_info.version);
    Ok(())
}

pub(crate) fn patch_mismatch_message(target: &Target, failure: &patch::HunkFailure) -> String {
    let first = failure.pattern.first().cloned().unwrap_or_default();
    let characters: Vec<char> = first.chars().collect();
    let preview = if characters.len() > 80 {
        format!("{}…", characters[..80].iter().collect::<String>())
    } else {
        first
    };
    format!(
        "cannot patch \"{}\": hunk at patch line {} ({}) does not match — {preview}",
        target.display(),
        failure.hunk_line,
        failure.stage
    )
}

/// 删除：需要有 present 观察且版本未变；成功后记 absent。
pub(crate) fn delete_file(
    context: &ToolContext,
    target: &Target,
    verb: &str,
) -> Result<(), ToolFailure> {
    delete_file_raw(context, target, verb).map_err(|failure| remediate(failure, target))
}

fn delete_file_raw(context: &ToolContext, target: &Target, verb: &str) -> Result<(), ToolFailure> {
    let expected = edit_version(context, target, verb)?;
    let Some(info) = probe(target)? else {
        return Err(ToolFailure::coded(
            "FS_STALE_VERSION",
            format!(
                "cannot {verb} \"{}\": file changed since it was read",
                target.display()
            ),
        ));
    };
    if !info.is_file {
        return Err(ToolFailure::coded(
            "FS_NOT_REGULAR_FILE",
            format!("cannot {verb} \"{}\": not a regular file", target.display()),
        ));
    }
    if info.version != expected {
        return Err(ToolFailure::coded(
            "FS_STALE_VERSION",
            format!(
                "cannot {verb} \"{}\": file changed since it was read",
                target.display()
            ),
        ));
    }
    fs::remove_file(&target.key)
        .map_err(|error| io_failure(&format!("cannot {verb}"), &target.key, &error))?;
    record_absent(context, target);
    Ok(())
}

/// move 的失败：destination_written 区分「什么都没改」和「目的地已写入、源还在」。
pub(crate) struct MoveFailure {
    pub failure: ToolFailure,
    pub destination_written: bool,
}

impl MoveFailure {
    fn before_destination(failure: ToolFailure) -> Self {
        Self {
            failure,
            destination_written: false,
        }
    }
}

/// move：读源（可带 hunk 改写）-> 写目的地 -> 删源；删源失败时目的地保留（不回滚，照 DSH）。
pub(crate) fn move_patched_file(
    context: &ToolContext,
    source: &Target,
    destination: &Target,
    hunks: Option<&[patch::Hunk]>,
) -> Result<(), MoveFailure> {
    let (text, _info) = read_guarded_text(context, source, "move")
        .map_err(|failure| MoveFailure::before_destination(remediate(failure, source)))?;
    let content = match hunks {
        Some(hunks) => {
            let after = patch::apply_hunks(&text.content, hunks).map_err(|failure| {
                MoveFailure::before_destination(ToolFailure::coded(
                    "FS_PATCH_MISMATCH",
                    patch_mismatch_message(source, &failure),
                ))
            })?;
            restore_line_endings(&after, text.line_endings)
        }
        None => restore_line_endings(&text.content, text.line_endings),
    };
    write_file(context, destination, &content).map_err(MoveFailure::before_destination)?;
    if let Err(failure) = delete_file(context, source, "move") {
        // 部分效果如实上报（docs/05 第 8 节）：目的地已写入、源还在。
        // 部分效果如实上报（照 DSH execute.ts:97-105）：失败消息只保留源删除错误，
        // 「目的地已写、源还在」由 apply_patch 的 render 按 DSH 格式单独成行。
        return Err(MoveFailure {
            failure: ToolFailure {
                code: failure.code.clone(),
                message: failure.message,
            },
            destination_written: true,
        });
    }
    Ok(())
}

/// 目的地检查（preflight 模拟用）：写策略 + 普通文件校验，不落盘。
pub(crate) fn check_destination(
    state_exists: bool,
    state_info: Option<&FileInfo>,
    touched: bool,
    context: &ToolContext,
    target: &Target,
) -> Result<(), ToolFailure> {
    check_destination_raw(state_exists, state_info, touched, context, target)
        .map_err(|failure| remediate(failure, target))
}

fn check_destination_raw(
    state_exists: bool,
    state_info: Option<&FileInfo>,
    touched: bool,
    context: &ToolContext,
    target: &Target,
) -> Result<(), ToolFailure> {
    if let Some(info) = state_info
        && !info.is_file
    {
        return Err(ToolFailure::coded(
            "FS_NOT_REGULAR_FILE",
            format!("cannot modify \"{}\": not a regular file", target.display()),
        ));
    }
    if touched {
        return Ok(());
    }
    match write_intent(context, target) {
        WriteIntent::CreateIfAbsent => {
            if state_exists {
                return Err(ToolFailure::coded(
                    "FS_NOT_OBSERVED",
                    format!(
                        "cannot overwrite existing \"{}\" without reading it first",
                        target.display()
                    ),
                ));
            }
        }
        WriteIntent::ReplaceIfVersion(version) => {
            let Some(info) = state_info else {
                return Err(ToolFailure::coded(
                    "FS_STALE_VERSION",
                    format!(
                        "cannot write \"{}\": file no longer exists",
                        target.display()
                    ),
                ));
            };
            if info.version != version {
                return Err(ToolFailure::coded(
                    "FS_STALE_VERSION",
                    format!(
                        "cannot write \"{}\": file changed since it was read",
                        target.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// 源检查（preflight 模拟用）：edit/delete/move 的观察 + 版本护栏。
pub(crate) fn check_source(
    state_info: Option<&FileInfo>,
    touched: bool,
    context: &ToolContext,
    target: &Target,
    verb: &str,
) -> Result<(), ToolFailure> {
    check_source_raw(state_info, touched, context, target, verb)
        .map_err(|failure| remediate(failure, target))
}

fn check_source_raw(
    state_info: Option<&FileInfo>,
    touched: bool,
    context: &ToolContext,
    target: &Target,
    verb: &str,
) -> Result<(), ToolFailure> {
    let intent = if touched {
        None
    } else {
        Some(edit_version(context, target, verb)?)
    };
    let Some(info) = state_info else {
        return Err(ToolFailure::coded(
            "FS_NOT_FOUND",
            format!("cannot {verb} \"{}\": not found", target.display()),
        ));
    };
    if !info.is_file {
        return Err(ToolFailure::coded(
            "FS_NOT_REGULAR_FILE",
            format!("cannot {verb} \"{}\": not a regular file", target.display()),
        ));
    }
    if let Some(version) = intent
        && info.version != version
    {
        return Err(ToolFailure::coded(
            "FS_STALE_VERSION",
            format!(
                "cannot {verb} \"{}\": file changed since it was read",
                target.display()
            ),
        ));
    }
    Ok(())
}

/// 路径本身（不跟随）是不是符号链接；不存在或不是链接都返回 false。
pub(crate) fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}
