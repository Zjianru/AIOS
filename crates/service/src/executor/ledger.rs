//! 账本：任务账本段、压缩后账本提交与账本工具回执。

use super::*;

impl TaskRuntime {
    /// 一次 Attempt 的任务书 JSON；launch 和压缩固定块共用同一段构造。
    pub(crate) fn assignment_value(
        &self,
        task: &Task,
        include_empty_ledger: bool,
    ) -> serde_json::Value {
        let ledger = self.ledger_section(task);
        let inherited_from = task.parent.as_ref().and_then(|parent| {
            self.store
                .task(parent)
                .ok()
                .flatten()
                .map(|root| root.title)
        });
        let quotes: Vec<serde_json::Value> = task
            .versions
            .iter()
            .enumerate()
            .filter_map(|(index, version)| {
                version.quote.as_ref().map(|quote| {
                    serde_json::json!({
                        "version": index + 1,
                        "label": "用户原话",
                        "quote": quote,
                        "inherited_from": inherited_from,
                    })
                })
            })
            .collect();
        let mut assignment = serde_json::Map::new();
        let has_quotes = !quotes.is_empty();
        if has_quotes {
            assignment.insert("user_quotes".to_owned(), serde_json::json!(quotes));
            assignment.insert(
                "requirement".to_owned(),
                serde_json::json!(task.requirement),
            );
            assignment.insert(
                "requirement_label".to_owned(),
                serde_json::json!("Kind 的理解，不是用户原话"),
            );
            if let Some(note) = task
                .versions
                .last()
                .and_then(|version| version.kind_note.as_deref())
            {
                assignment.insert("kind_note".to_owned(), serde_json::json!(note));
                assignment.insert(
                    "kind_note_label".to_owned(),
                    serde_json::json!("Kind 的补充，不算用户要求"),
                );
            }
            let constraints = task.current_constraints();
            if !constraints.is_empty() {
                assignment.insert("constraints".to_owned(), serde_json::json!(constraints));
            }
            assignment.insert("dod".to_owned(), serde_json::json!(task.dod));
        }
        assignment.insert("task_id".to_owned(), serde_json::json!(task.id.as_str()));
        assignment.insert("title".to_owned(), serde_json::json!(task.title));
        if !has_quotes {
            assignment.insert(
                "requirement".to_owned(),
                serde_json::json!(task.requirement),
            );
            assignment.insert("dod".to_owned(), serde_json::json!(task.dod));
        }
        assignment.insert(
            "cwd".to_owned(),
            serde_json::json!(task.effective_cwd().to_string_lossy()),
        );
        if let Some(scope) = task.effective_write_scope() {
            assignment.insert(
                "write_scope".to_owned(),
                serde_json::json!(scope.to_string_lossy()),
            );
        }
        let upstream = self.upstream_results_for(task);
        if !upstream.is_empty() {
            assignment.insert("upstream_results".to_owned(), serde_json::json!(upstream));
        }
        if !ledger.is_empty() || include_empty_ledger {
            assignment.insert("ledger".to_owned(), serde_json::json!(ledger));
        }
        serde_json::Value::Object(assignment)
    }

    /// 开跑用的任务书 JSON 文本：顶层字段按票面顺序写，不受 serde_json Map 排序影响。
    pub(crate) fn assignment_text(&self, task: &Task, include_empty_ledger: bool) -> String {
        let value = self.assignment_value(task, include_empty_ledger);
        let Some(map) = value.as_object() else {
            return value.to_string();
        };
        let has_quotes = map.contains_key("user_quotes");
        let order: &[&str] = if has_quotes {
            &[
                "user_quotes",
                "requirement",
                "requirement_label",
                "kind_note",
                "kind_note_label",
                "constraints",
                "dod",
                "task_id",
                "title",
                "cwd",
                "write_scope",
                "upstream_results",
                "ledger",
            ]
        } else {
            &[
                "task_id",
                "title",
                "requirement",
                "dod",
                "cwd",
                "write_scope",
                "upstream_results",
                "ledger",
            ]
        };
        let mut fields = Vec::new();
        for key in order {
            if let Some(field) = map.get(*key) {
                fields.push(format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap_or_default(),
                    field
                ));
            }
        }
        format!("{{{}}}", fields.join(","))
    }

    /// 压缩替换里重带的任务书+账本原文；只有当前 Attempt 有效。
    pub(crate) fn compaction_task_block(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
    ) -> Option<agent::CompactionFixedBlock> {
        let task = self.store.task(task_id).ok()??;
        if task.latest_attempt().map(|attempt| attempt.id) != Some(attempt_id) {
            return None;
        }
        let assignment = self.assignment_text(&task, true);
        Some(agent::CompactionFixedBlock {
            kind: "task".to_owned(),
            identity: format!("{}:{}:{}", task.id, attempt_id, task.requirement_version()),
            text: format!("以下是程序重带的任务书与账本原文，不是新任务：\n{assignment}"),
        })
    }

    /// ：给这一次 Attempt 的账本段；空账本为空串。读库失败只记日志、不挡开工。
    pub(crate) fn ledger_section(&self, task: &Task) -> String {
        match crate::ledger::ledger_for(&self.store, task) {
            Ok(view) => view.section(),
            Err(error) => {
                eprintln!("任务 {} 的账本读不出来：{error}", task.id);
                String::new()
            }
        }
    }

    /// ：ThinAgent 记账本；回执给在用的条数。
    pub(crate) fn ledger_note_from_tool(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        entries: &[tasks::NewLedgerNote],
    ) -> Result<String, ToolFailure> {
        let counts = self
            .store
            .add_ledger_notes(task_id, attempt_id, entries, Self::now())
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        Ok(format!(
            "记下了。账本里现在：否掉的做法 {} 条，下一步 {} 条",
            counts.rejected, counts.next
        ))
    }

    /// ：读全部账本历史（最新在前，不去重、不分页、不注入）。
    pub(crate) fn ledger_history_from_tool(&self, task_id: &TaskId) -> Result<String, ToolFailure> {
        let notes = self
            .store
            .ledger_notes(task_id)
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        if notes.is_empty() {
            return Ok("账本还是空的。".to_owned());
        }
        let mut lines = vec!["账本历史（最新在前）：".to_owned()];
        for note in notes {
            let kind = match note.kind {
                tasks::LedgerNoteKind::Rejected => "否掉的做法",
                tasks::LedgerNoteKind::Next => "下一步",
            };
            match note.closed_at {
                Some(_) => lines.push(format!(
                    "- [{kind}] {}（第 {} 次记；关闭：{}）",
                    note.text,
                    note.attempt_id.0,
                    note.closed_why.unwrap_or_else(|| "已关闭".to_owned())
                )),
                None => lines.push(format!(
                    "- [{kind}] {}（第 {} 次记；在用）",
                    note.text, note.attempt_id.0
                )),
            }
        }
        Ok(lines.join(
            "
",
        ))
    }
}
