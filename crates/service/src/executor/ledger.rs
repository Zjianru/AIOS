//! 账本：任务账本段、压缩后账本提交与账本工具回执。

use super::*;

impl TaskRuntime {
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

    /// ：压缩成功要 Steer 的消息和 request id；空账本返回 None。
    pub(crate) fn ledger_after_compaction_payload(
        &self,
        task: &Task,
        compaction_id: &str,
    ) -> Option<(String, UserMessage)> {
        let ledger = self.ledger_section(task);
        if ledger.is_empty() {
            return None;
        }
        Some((
            format!("ledger-after-compaction:{compaction_id}"),
            UserMessage::text(format!("账本（压缩后重带）\n{ledger}")),
        ))
    }

    /// 的发送接缝：空账本不调用 send；发送结果原样交给调用方（生产侧只记日志）。
    pub(crate) async fn steer_ledger_with<F>(
        &self,
        task: &Task,
        compaction_id: &str,
        send: F,
    ) -> Result<bool, String>
    where
        F: FnOnce(
            String,
            UserMessage,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
        >,
    {
        let Some((request_id, message)) = self.ledger_after_compaction_payload(task, compaction_id)
        else {
            return Ok(false);
        };
        send(request_id, message).await.map(|()| true)
    }

    /// ：压缩成功后把当前账本 Steer 回会话；空账本不发，失败只记日志。
    pub(crate) async fn steer_ledger_after_compaction(
        &self,
        task_id: TaskId,
        attempt_id: AttemptId,
        conversation: SessionId,
        compaction_id: String,
    ) {
        let Ok(Some(task)) = self.store.task(&task_id) else {
            return;
        };
        if task.latest_attempt().map(|attempt| attempt.id) != Some(attempt_id) {
            return;
        }
        let host = self
            .running
            .lock()
            .expect("thinagent 表锁被毒化")
            .get(conversation.as_str())
            .map(|running| Arc::clone(&running.host));
        let Some(host) = host else {
            return;
        };
        let task_id = task.id.clone();
        if let Err(error) = self
            .steer_ledger_with(&task, &compaction_id, move |request_id, message| {
                let host = Arc::clone(&host);
                Box::pin(async move {
                    host.send_message(&request_id, message, DeliveryMode::Steer)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                })
            })
            .await
        {
            eprintln!("压缩后账本没送回任务 {task_id}：{error}");
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
