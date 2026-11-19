//! 交付记录：任务交付文本与 delivery 事实。

use super::*;

impl TaskRuntime {
    /// 任务对话的只读快照：头部来自任务聚合，事件来自当前 Attempt 的会话日志。
    ///
    /// 只读打开：不创建会话、不拿写者锁；会话日志缺失/损坏时把原因带进可读错误。
    /// 分页照 events 的预算（500 条消息/2 轮/50 条下限），`lastSeq` 始终是日志末端。
    /// ：还没开跑的任务也能读：头部照给（attempts: 0），事件为空、lastSeq 为 0；
    /// 统计照冻结模型算空日志（轮/步/token 都是 0）。
    /// ：只读派生一份交付记录；门由任务库的 guarding_gate 取，不挂 TaskRead。
    pub(crate) async fn task_delivery(
        &self,
        request: crate::protocol::TaskDeliveryRequest,
    ) -> Result<crate::protocol::TaskDeliveryResponse, ServiceError> {
        let task = self.task(&request.task_id)?;
        let gate = self
            .store
            .guarding_gate(&task.id)
            .map_err(|error| ServiceError::TaskUnavailable(format!("门任务读不出来：{error}")))?;
        Ok(crate::protocol::TaskDeliveryResponse {
            delivery: crate::delivery::delivery_for(&self.data_dir, &task, gate.as_ref()),
        })
    }

    /// 把任务里还没送出的用户输入 Steer 进任务会话；request id task-say:<inputId> 判重。
    pub(crate) async fn deliver_delivery(self: &Arc<Self>, claim: EffectClaim) {
        let EffectPayload::Delivery { task_id } = &claim.payload else {
            self.finish_or_fail(&claim, Err("delivery 载荷不是 delivery".to_owned()));
            return;
        };
        let task = match self.store.task(task_id) {
            Ok(Some(task)) => task,
            Ok(None) => {
                self.finish_or_fail(&claim, Ok(()));
                return;
            }
            Err(error) => {
                self.finish_or_fail(&claim, Err(error.to_string()));
                return;
            }
        };
        let Some(attempt) = task.latest_attempt() else {
            self.finish_or_fail(&claim, Ok(()));
            return;
        };
        // 最近一次 Attempt 是重启打断的：这次说话就是接续（trigger resume、wake 席位、
        // 先放恢复说明）。别的结束原因照 的说话接续走。
        let mode = if task.is_interrupted() {
            ResumeMode::Interrupted
        } else {
            ResumeMode::Say
        };
        // 保留宿主不是「还在跑」：用户说话前先停命令/关宿主，再走接续。
        if self
            .running_for(task_id, attempt.id)
            .is_some_and(|running| running.retained)
            && let Some(conversation) = attempt.conversation.as_ref()
        {
            self.stop_retained_for_reopen(task_id, conversation.as_str())
                .await;
        }
        let mut recovery_attempt = None;
        let mut opened_attempt = None;
        let host = match self.running_for(task_id, attempt.id) {
            Some(running) => running.host,
            // 没在跑：停/结束之后在任务对话里说话，接着原会话开新一轮。
            None => match self.resume_for_delivery(&claim, &task, attempt, mode).await {
                Some(opened) => {
                    opened_attempt = Some(opened.attempt_id);
                    if matches!(mode, ResumeMode::Interrupted) {
                        recovery_attempt = Some(opened.attempt_id);
                    }
                    opened.host
                }
                None => return,
            },
        };
        // ：新开的接续 Attempt 先交最新的上游结果，再送用户的话（顺序由 Steer 队列保证）。
        if let Some(attempt_id) = opened_attempt
            && let Err(error) = self.send_upstream_note(&host, &task, attempt_id).await
        {
            eprintln!("上游结果没送进任务会话：{error}");
        }
        let mut sent = Ok(());
        let inputs = task.inputs.len();
        for (index, input) in task.inputs.iter().enumerate() {
            // 打断后的第一句话：恢复说明和用户的话同一条消息，恢复说明在前；
            // request id 记在这次接续上。
            let recovery = recovery_attempt.filter(|_| index + 1 == inputs);
            let request_id = match recovery {
                Some(attempt_id) => format!("resume:{task_id}:{attempt_id}"),
                None => format!("task-say:{}", input.id.as_str()),
            };
            let mut message = match self.say_message(input) {
                Ok(message) => message,
                Err(error) => {
                    sent = Err(format!(
                        "输入 {} 的附件没读回来：{error}",
                        input.id.as_str()
                    ));
                    break;
                }
            };
            if recovery.is_some() {
                let mut blocks = message.into_message().content;
                blocks.insert(0, ContentBlock::text(self.resume_note(&task)));
                message = UserMessage::new(blocks);
            }
            if let Err(error) = host
                .send_message(&request_id, message, DeliveryMode::Steer)
                .await
            {
                sent = Err(format!(
                    "输入 {} 没送进任务会话：{error}",
                    input.id.as_str()
                ));
                break;
            }
        }
        match sent {
            Ok(()) => best_effort(
                "delivery 完成没写进任务库",
                self.store.complete_effect(&claim),
            ),
            Err(error) => best_effort(
                "delivery 失败没写进任务库",
                self.store.fail_effect(&claim, &error, Self::now()),
            ),
        }
    }

    /// Kind 的 task_status：阶段、最近报告全文、Attempt 数；waiting 时向 ThinAgent 要一次。
    /// ：Kind 的 task_delivery——和任务页、TaskDelivery 请求读同一份派生。
    pub(crate) async fn delivery_text(&self, task_id: &str) -> Result<String, ToolFailure> {
        let id = TaskId::from(task_id);
        let task = self
            .store
            .task(&id)
            .map_err(|error| ToolFailure::plain(error.to_string()))?
            .ok_or_else(|| ToolFailure::plain(format!("任务不存在：{task_id}")))?;
        if task.is_graph_root() {
            return Err(ToolFailure::plain(
                "总任务没有交付记录，看它的子任务".to_owned(),
            ));
        }
        if task.is_gate() {
            return Err(ToolFailure::plain(
                "门任务没有交付记录，看它把关的任务".to_owned(),
            ));
        }
        let gate = self
            .store
            .guarding_gate(&task.id)
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        let Some(record) = crate::delivery::delivery_for(&self.data_dir, &task, gate.as_ref())
        else {
            return Err(ToolFailure::plain("还没交出过结果".to_owned()));
        };
        Ok(record.text())
    }
}
