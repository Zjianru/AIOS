//! 会话旁注与全局描述文件。

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 一段会话旁注的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoteKind {
    Described,
    Omitted,
    BackendFailed,
}

/// 会话旁里的一行：同一 attachmentId 第一次写下的文字为准。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionNote {
    #[serde(rename = "attachmentId")]
    pub attachment_id: String,
    pub text: String,
    pub kind: NoteKind,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}

/// 一张图的全局描述；只用于第一次发送时选文字，写一次不改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalNote {
    #[serde(rename = "attachmentId")]
    pub attachment_id: String,
    pub description: String,
    pub model: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn notes_path(data_dir: &Path, session_id: &str) -> PathBuf {
    data_dir
        .join("sessions")
        .join(format!("{session_id}.vision-notes.jsonl"))
}

/// 一个会话的旁注：启动时读一次，之后新写的行也进内存；同一 id 后来的行忽略。
#[derive(Debug, Clone)]
pub struct SessionNotes {
    path: PathBuf,
    by_id: BTreeMap<String, VisionNote>,
}

impl SessionNotes {
    /// 读会话旁注；会话目录不存在时返回空表（第一次发送会创建文件）。
    pub fn load(data_dir: &Path, session_id: &str) -> std::io::Result<Self> {
        let path = notes_path(data_dir, session_id);
        let mut by_id = BTreeMap::new();
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let Ok(note) = serde_json::from_str::<VisionNote>(line) else {
                    continue;
                };
                by_id.entry(note.attachment_id.clone()).or_insert(note);
            }
        }
        Ok(Self { path, by_id })
    }

    pub fn get(&self, attachment_id: &str) -> Option<&VisionNote> {
        self.by_id.get(attachment_id)
    }

    pub fn notes(&self) -> impl Iterator<Item = (&String, &VisionNote)> {
        self.by_id.iter()
    }

    /// 第一次写这个附件时落一行；已有行时原样返回 false，不覆盖。
    pub fn append_if_absent(&mut self, note: VisionNote) -> std::io::Result<bool> {
        if self.by_id.contains_key(&note.attachment_id) {
            return Ok(false);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_string(&note)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.sync_data()?;
        self.by_id.insert(note.attachment_id.clone(), note);
        Ok(true)
    }

    /// 写一行对应的文字；created_at 用当前时间。
    pub fn write_text(
        &mut self,
        attachment_id: &str,
        kind: NoteKind,
        text: String,
    ) -> std::io::Result<bool> {
        self.append_if_absent(VisionNote {
            attachment_id: attachment_id.to_owned(),
            text,
            kind,
            created_at: now_ms(),
        })
    }
}

/// 全局描述目录：<data_dir>/vision/notes/<sha256 hex>.json。
#[derive(Debug, Clone)]
pub struct GlobalNotes {
    root: PathBuf,
}

impl GlobalNotes {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("vision").join("notes"),
        }
    }

    fn path_for(&self, attachment_id: &str) -> Option<PathBuf> {
        let hex = attachment_id.strip_prefix("sha256:")?;
        let valid = hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        valid.then(|| self.root.join(format!("{hex}.json")))
    }

    pub fn read(&self, attachment_id: &str) -> Option<GlobalNote> {
        let path = self.path_for(attachment_id)?;
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// 写一次不改；已有文件返回 false。
    pub fn write_if_absent(&self, note: &GlobalNote) -> std::io::Result<bool> {
        let Some(path) = self.path_for(&note.attachment_id) else {
            return Ok(false);
        };
        if path.exists() {
            return Ok(false);
        }
        fs::create_dir_all(&self.root)?;
        let bytes = serde_json::to_vec(note)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let temp = path.with_extension("json.tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        match fs::rename(&temp, &path) {
            Ok(()) => Ok(true),
            Err(error) => {
                drop(fs::remove_file(&temp));
                Err(error)
            }
        }
    }
}
