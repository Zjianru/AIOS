//! 记忆的服务层：打开库，给 Kind/ThinAgent 拼注入文本，压缩前把日记写进库。

use std::path::Path;
use std::sync::Arc;

use agent::{MemoryContext, MemoryContextProvider, PreCompactionDiary, PreCompactionHook};
use memory::MemoryStore;
use memory::types::{Binding, Layer, MemoryItem, WriteContext, WriteOp};

pub struct MemoryRuntime {
    store: Arc<MemoryStore>,
}

impl MemoryRuntime {
    pub fn open(data_dir: &Path) -> Result<Self, memory::MemoryError> {
        let store = MemoryStore::open(data_dir.join("memory.sqlite"))?;
        Ok(Self {
            store: Arc::new(store),
        })
    }

    pub fn store(&self) -> &Arc<MemoryStore> {
        &self.store
    }

    /// Kind：关于你全文 + 条目索引；都没有就返回 None（不往空会话塞空记忆）。
    pub fn kind_context(&self) -> Option<String> {
        let view = self.store.view().ok()?;
        let mut parts = Vec::new();
        if !view.about_you.is_empty() {
            parts.push("关于你：".to_owned());
            parts.extend(
                view.about_you
                    .iter()
                    .map(|item| format!("- {}", item_text(item))),
            );
        }
        let entries: Vec<&MemoryItem> = view.entries.iter().chain(view.notes.iter()).collect();
        if !entries.is_empty() {
            parts.push("条目索引：".to_owned());
            parts.extend(entries.iter().map(|item| {
                format!(
                    "- {} · {} · {} · {}",
                    item.id,
                    item.title,
                    item.scope.as_str(),
                    item.binding.as_str()
                )
            }));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(
                "
",
            ))
        }
    }

    /// ThinAgent：只给个人范围、有约束力的条目。
    pub fn thin_context(&self) -> Option<String> {
        let view = self.store.view().ok()?;
        let mut parts = Vec::new();
        for item in view.about_you.iter().chain(view.entries.iter()) {
            if item.binding == Binding::Binding && item.status == "active" {
                parts.push(format!("- {}", item_text(item)));
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(
                "有约束力的记忆：
"
                .to_owned()
                    + &parts.join(
                        "
",
                    ),
            )
        }
    }

    pub fn kind_provider(&self) -> MemoryContextProvider {
        let runtime = Arc::new(Self {
            store: Arc::clone(&self.store),
        });
        Arc::new(move || {
            let identity = runtime
                .store
                .version()
                .map_err(|error| error.to_string())?
                .to_string();
            Ok(runtime
                .kind_context()
                .map(|text| MemoryContext { identity, text }))
        })
    }

    pub fn thin_provider(&self) -> MemoryContextProvider {
        let runtime = Arc::new(Self {
            store: Arc::clone(&self.store),
        });
        Arc::new(move || {
            let identity = runtime
                .store
                .version()
                .map_err(|error| error.to_string())?
                .to_string();
            Ok(runtime
                .thin_context()
                .map(|text| MemoryContext { identity, text }))
        })
    }

    pub fn pre_compaction_hook(&self) -> PreCompactionHook {
        let store = Arc::clone(&self.store);
        Arc::new(move |diary: PreCompactionDiary| {
            let store = Arc::clone(&store);
            Box::pin(async move {
                if diary.text.trim().is_empty() {
                    return Ok(());
                }
                store
                    .write(
                        WriteOp::Diary {
                            text: diary.text,
                            source_ref: diary.compaction_id,
                        },
                        &WriteContext::kind(),
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        })
    }
}

fn item_text(item: &MemoryItem) -> String {
    let body = match (item.summary.is_empty(), item.body.is_empty()) {
        (true, true) => String::new(),
        (true, false) => item.body.clone(),
        (false, true) => item.summary.clone(),
        (false, false) => format!("{}：{}", item.summary, item.body),
    };
    if item.title.is_empty() {
        body
    } else if body.is_empty() {
        item.title.clone()
    } else {
        format!("{}：{}", item.title, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory::types::{WriteContext, WriteOp};

    #[test]
    fn kind_thin_sections_differ_and_follow_store_changes() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = MemoryRuntime::open(dir.path()).unwrap();
        runtime
            .store()
            .write(
                WriteOp::Remember {
                    title: "回答先给结论".to_owned(),
                    summary: "先结论".to_owned(),
                    body: "再给理由".to_owned(),
                    source_kind: "user".to_owned(),
                    source_ref: "u1".to_owned(),
                },
                &WriteContext::user(),
            )
            .unwrap();
        runtime
            .store()
            .write(
                WriteOp::Note {
                    title: "GBK CSV".to_owned(),
                    summary: "先看编码".to_owned(),
                    body: "sources/ 下有 GBK".to_owned(),
                    source_kind: "kind".to_owned(),
                    source_ref: "k1".to_owned(),
                },
                &WriteContext::kind(),
            )
            .unwrap();
        let kind = runtime.kind_context().unwrap();
        assert!(kind.contains("关于你"));
        assert!(kind.contains("回答先给结论"));
        assert!(kind.contains("条目索引"));
        assert!(kind.contains("GBK CSV"));
        let thin = runtime.thin_context().unwrap();
        assert!(thin.contains("有约束力的记忆"));
        assert!(thin.contains("回答先给结论"));
        assert!(!thin.contains("GBK CSV"), "ThinAgent 只拿有约束力的条目");
        let before = runtime.kind_provider()().unwrap().unwrap().identity;
        runtime
            .store()
            .write(
                WriteOp::Note {
                    title: "新笔记".to_owned(),
                    summary: String::new(),
                    body: "新".to_owned(),
                    source_kind: "kind".to_owned(),
                    source_ref: "k2".to_owned(),
                },
                &WriteContext::kind(),
            )
            .unwrap();
        let after = runtime.kind_provider()().unwrap().unwrap().identity;
        assert_ne!(before, after);
    }
}
