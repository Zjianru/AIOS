//! 盯变化：网页/本机文件/任务事实三种来源；存上次内容和哈希，变了才给 diff。
//!
//! URL 复用 web_fetch 的正文抓取（不查 Exa key、不改 URL 规则）；文件只读、规范化、限大小。

use super::*;
use crate::web::{WebFetchConfig, WebFetchTool};
use calendar::WatchSource;
use similar::TextDiff;

const WATCH_SUMMARY_CHARS: usize = 4_000;

pub(crate) enum WatchResult {
    Unchanged,
    Changed { summary: String },
    Failed(String),
}

impl TaskRuntime {
    pub(crate) async fn watch_check(&self, appointment: &calendar::Appointment) -> WatchResult {
        let Some(source) = &appointment.watch else {
            return WatchResult::Unchanged;
        };
        let content = match self.watch_content(source).await {
            Ok(content) => content,
            Err(reason) => return WatchResult::Failed(reason),
        };
        let hash = content_hash(&content);
        let previous = match self.calendar().and_then(|store| {
            store
                .watch_state(&appointment.id)
                .map_err(|error| error.to_string())
        }) {
            Ok(previous) => previous,
            Err(reason) => return WatchResult::Failed(reason),
        };
        let Some(previous) = previous else {
            if let Err(error) = self.calendar().and_then(|store| {
                store
                    .save_watch_state(&appointment.id, &content, &hash, None)
                    .map_err(|error| error.to_string())
            }) {
                return WatchResult::Failed(error);
            }
            return WatchResult::Unchanged;
        };
        if previous.hash == hash && previous.content == content {
            return WatchResult::Unchanged;
        }
        let (summary, attachment_id) =
            match render_watch_diff(&previous.content, &content, &self.data_dir) {
                Ok(result) => result,
                Err(reason) => return WatchResult::Failed(reason),
            };
        let summary = format!("变了：\n{summary}");
        if let Err(error) = self.calendar().and_then(|store| {
            store
                .save_watch_state(&appointment.id, &content, &hash, attachment_id.as_deref())
                .map_err(|error| error.to_string())
        }) {
            return WatchResult::Failed(error);
        }
        WatchResult::Changed { summary }
    }

    async fn watch_content(&self, source: &WatchSource) -> Result<String, String> {
        match source {
            WatchSource::Url(url) => {
                let config = WebFetchConfig {
                    data_dir: self.options.data_dir.clone(),
                    identity: self.options.identity.clone(),
                    self_addr: self.options.web_listen.clone(),
                    http: model::http::HttpEnv::from_process(),
                    timeout: crate::web::WEB_FETCH_TIMEOUT,
                };
                fetch_watch_url(config, url).await
            }
            WatchSource::File(path) => {
                let canonical = std::fs::canonicalize(path)
                    .map_err(|error| format!("取不到文件（{error}）"))?;
                let metadata = std::fs::metadata(&canonical)
                    .map_err(|error| format!("取不到文件（{error}）"))?;
                if !metadata.is_file() {
                    return Err("取不到文件（不是普通文件）".to_owned());
                }
                // 正文上限照 web_fetch。
                if metadata.len() > 5 * 1024 * 1024 {
                    return Err("取不到文件（超过 5 MiB）".to_owned());
                }
                let bytes =
                    std::fs::read(&canonical).map_err(|error| format!("取不到文件（{error}）"))?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
            WatchSource::TaskFacts(task_id) => {
                let response = crate::work::snapshot(&self.store, &[], &[], Self::now())
                    .map_err(|error| format!("取不到任务事实（{error}）"))?;
                let fact = response
                    .tasks
                    .iter()
                    .find(|fact| fact.id == *task_id)
                    .ok_or_else(|| "取不到任务事实（工作快照里没有这个任务）".to_owned())?;
                serde_json::to_string(fact).map_err(|error| format!("取不到任务事实（{error}）"))
            }
        }
    }
}

fn content_hash(content: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// 两份内容的行 diff：摘要最多 4000 字；更长的全文存附件。
fn render_watch_diff(
    previous: &str,
    current: &str,
    data_dir: &std::path::Path,
) -> Result<(String, Option<String>), String> {
    let diff = TextDiff::from_lines(previous, current)
        .unified_diff()
        .header("上次", "这次")
        .to_string();
    if diff.chars().count() <= WATCH_SUMMARY_CHARS {
        return Ok((diff, None));
    }
    let stored = tools::attachments::store(data_dir, "watch-diff.txt", diff.as_bytes())
        .map_err(|error| format!("diff 太长，附件没存成（{error}）"))?;
    Ok((
        format!(
            "{}…（完整 diff 见附件）",
            diff.chars().take(WATCH_SUMMARY_CHARS).collect::<String>()
        ),
        Some(stored.id),
    ))
}

async fn fetch_watch_url(config: WebFetchConfig, url: &str) -> Result<String, String> {
    WebFetchTool::new(config)
        .fetch_body(url)
        .await
        .map_err(|failure| failure.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_diff_stays_inline() {
        let directory = tempfile::tempdir().unwrap();
        let (summary, attachment) =
            render_watch_diff("一\n二\n", "一\n三\n", directory.path()).unwrap();
        assert!(summary.contains("-二"));
        assert!(summary.contains("+三"));
        assert!(attachment.is_none());
    }

    #[test]
    fn long_diff_is_stored_as_an_attachment() {
        let directory = tempfile::tempdir().unwrap();
        let previous = (0..2000)
            .map(|index| format!("旧行 {index}\n"))
            .collect::<String>();
        let current = (0..2000)
            .map(|index| format!("新行 {index}\n"))
            .collect::<String>();
        let (summary, attachment) =
            render_watch_diff(&previous, &current, directory.path()).unwrap();
        assert!(summary.chars().count() <= WATCH_SUMMARY_CHARS + 20);
        let id = attachment.expect("长 diff 要放附件");
        let stored = tools::attachments::read(directory.path(), &id).unwrap();
        assert!(stored.bytes > previous.len());
    }

    #[tokio::test]
    async fn url_source_fetches_body_without_exa_key() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_string("watch body"))
            .expect(1)
            .mount(&server)
            .await;
        let directory = tempfile::tempdir().unwrap();
        let config = WebFetchConfig {
            data_dir: directory.path().to_path_buf(),
            identity: Some(directory.path().join("identity.txt")),
            self_addr: None,
            http: model::http::HttpEnv {
                http_proxy: Some(server.uri()),
                https_proxy: None,
                all_proxy: None,
                no_proxy: None,
            },
            timeout: crate::web::WEB_FETCH_TIMEOUT,
        };
        let tool = WebFetchTool::new(config);
        let body = tool.fetch_body("http://93.184.216.34/watch").await.unwrap();
        assert!(body.contains("watch body"), "{body}");
    }
}
