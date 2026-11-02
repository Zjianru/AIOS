//! 文件观察策略（T006.6，照 DSH packages/fs/fs-observation-policy）。
//!
//! 单独模块：观察状态（present/absent）、写意图、edit/delete 的先读护栏都只在这里；
//! fs 工具在工具执行 seam 调用它（read 成功后 observe，写类先取 intent 再 observe），
//! 对应 DSH provider 触发 fs/write-intent、fs/edit-intent、fs/observed 的位置。
//! DSH 依据：packages/fs/fs-observation-policy/src/index.ts:30-137。
//! 不引入运行时插件（K2）：策略是 tools crate 内的具体类型，由 ToolSessionState 持有。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ToolFailure;
use crate::session::FileVersion;

/// 一个文件在本会话里的观察结果（照 DSH FsObservation：unseen 与 absent 区分）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedFile {
    /// 读过，且读到这个版本。
    Present(FileVersion),
    /// 读过，确认当时不存在（之后 write 可以走 createIfAbsent）。
    Absent,
}

/// 写策略（照 DSH FsWriteIntent）：未见或已确认缺失 -> createIfAbsent；
/// 见过 present -> replaceIfVersion。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteIntent {
    CreateIfAbsent,
    ReplaceIfVersion(FileVersion),
}

/// 会话内的文件观察策略：canonical target key -> 观察结果。
#[derive(Debug, Default)]
pub struct ObservationPolicy {
    files: HashMap<PathBuf, ObservedFile>,
}

impl ObservationPolicy {
    pub fn get(&self, key: impl AsRef<Path>) -> Option<&ObservedFile> {
        self.files.get(key.as_ref())
    }

    /// DSH write-intent：未见/absent 走 createIfAbsent；present 用观察到的版本做 CAS。
    pub fn write_intent(&self, key: &Path) -> WriteIntent {
        match self.files.get(key) {
            Some(ObservedFile::Present(version)) => WriteIntent::ReplaceIfVersion(version.clone()),
            Some(ObservedFile::Absent) | None => WriteIntent::CreateIfAbsent,
        }
    }

    /// DSH edit-intent/delete-intent：未见 -> FS_NOT_OBSERVED；确认缺失 -> FS_NOT_FOUND；
    /// present 返回读到的版本（edit/delete 的 CAS 基）。
    pub fn edit_intent(
        &self,
        key: &Path,
        display: &str,
        verb: &str,
    ) -> Result<FileVersion, ToolFailure> {
        match self.files.get(key) {
            Some(ObservedFile::Present(version)) => Ok(version.clone()),
            Some(ObservedFile::Absent) => Err(ToolFailure::coded(
                "FS_NOT_FOUND",
                format!("cannot {verb} \"{display}\": not found"),
            )),
            None => Err(ToolFailure::coded(
                "FS_NOT_OBSERVED",
                format!("{verb} requires reading \"{display}\" first"),
            )),
        }
    }

    /// 记录一次权威的 present 观察（read 成功、写类成功之后）。
    pub fn observe_present(&mut self, key: impl Into<PathBuf>, version: FileVersion) {
        self.files
            .insert(key.into(), ObservedFile::Present(version));
    }

    /// 记录一次确认缺失（read 找不到时）。
    pub fn observe_absent(&mut self, key: impl Into<PathBuf>) {
        self.files.insert(key.into(), ObservedFile::Absent);
    }

    pub fn clear(&mut self) {
        self.files.clear();
    }

    /// 已观察的文件路径（T005.5 指令刷新用：读到更深目录时补指令）。
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.files.keys()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(raw: &str) -> FileVersion {
        FileVersion::new(raw)
    }

    #[test]
    fn write_intent_distinguishes_unseen_absent_and_present() {
        let mut policy = ObservationPolicy::default();
        let key = Path::new("/tmp/obs/a.txt");
        assert_eq!(policy.write_intent(key), WriteIntent::CreateIfAbsent);
        policy.observe_absent(key);
        assert_eq!(policy.write_intent(key), WriteIntent::CreateIfAbsent);
        policy.observe_present(key, version("v1"));
        assert_eq!(
            policy.write_intent(key),
            WriteIntent::ReplaceIfVersion(version("v1"))
        );
    }

    #[test]
    fn edit_intent_requires_read_then_reports_codes() {
        let mut policy = ObservationPolicy::default();
        let key = Path::new("/tmp/obs/a.txt");
        let unseen = policy.edit_intent(key, "a.txt", "edit").unwrap_err();
        assert_eq!(unseen.code.as_deref(), Some("FS_NOT_OBSERVED"));
        assert!(unseen.message.contains("edit requires reading"));
        policy.observe_absent(key);
        let absent = policy.edit_intent(key, "a.txt", "delete").unwrap_err();
        assert_eq!(absent.code.as_deref(), Some("FS_NOT_FOUND"));
        policy.observe_present(key, version("v2"));
        assert_eq!(
            policy.edit_intent(key, "a.txt", "edit").unwrap(),
            version("v2")
        );
    }

    #[test]
    fn clear_forgets_observations_and_paths_are_listed() {
        let mut policy = ObservationPolicy::default();
        assert!(policy.is_empty());
        policy.observe_present("/tmp/obs/a.txt", version("v1"));
        policy.observe_absent("/tmp/obs/b.txt");
        assert_eq!(policy.len(), 2);
        let mut paths: Vec<&PathBuf> = policy.paths().collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                &PathBuf::from("/tmp/obs/a.txt"),
                &PathBuf::from("/tmp/obs/b.txt")
            ]
        );
        policy.clear();
        assert!(policy.is_empty());
    }
}
