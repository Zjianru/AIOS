//! Keyring 的只读视图：设置页/CLI 看的连接描述、签发记录与今天的签发数。
//!
//! 视图里没有 key 正文；读失败降级成空视图并在钥匙文件问题上留信号，不改存储。

use super::{IssueRecord, KeyFileInfo, Keyring, KeyringError, KeyringView, now_seconds};

impl Keyring {
    /// 设置页/CLI 的描述视图；不含任何 key 正文。
    pub fn view(&self) -> KeyringView {
        let identity_path = self.identity_path();
        let (connections, issues, issued_today, problem) = match self.load_store() {
            Ok(store) => {
                let now = now_seconds();
                let today_start = now.saturating_sub(now % 86_400);
                let issued_today = store
                    .issues
                    .iter()
                    .filter(|issue| issue.at >= today_start)
                    .count() as u64;
                (
                    store
                        .connections
                        .into_iter()
                        .map(|item| item.info)
                        .collect(),
                    store.issues,
                    issued_today,
                    None,
                )
            }
            Err(error) => (Vec::new(), Vec::new(), 0, Some(error.to_string())),
        };
        let first_use = problem.is_none() && connections.is_empty();
        let mut issues = issues;
        issues.reverse();
        issues.truncate(20);
        KeyringView {
            connections,
            key_file: KeyFileInfo {
                path: identity_path,
                since: self.since,
                problem,
            },
            issued_today,
            issues,
            first_use,
        }
    }

    /// F1：当前持有的全部 key 正文（只在进程内交给监控脱敏；不落盘、不进页面/日志）。
    pub fn secrets(&self) -> Result<Vec<String>, KeyringError> {
        let _lock = self.lock()?;
        let store = self.load_store()?;
        Ok(store
            .connections
            .into_iter()
            .map(|stored| stored.api_key)
            .filter(|key| !key.is_empty())
            .collect())
    }

    /// 签发记录（最近的在前）。
    pub fn issues(&self) -> Vec<IssueRecord> {
        match self.load_store() {
            Ok(mut store) => {
                store.issues.reverse();
                store.issues
            }
            Err(_) => Vec::new(),
        }
    }
}
