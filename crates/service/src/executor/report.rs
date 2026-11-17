//! 报告与 relay：工具上报落点、relay/停止后收尾与回合结束事实。

use super::*;

/// ：写任务当前要求版本的 ready 报告 → 结果卡「待你核」/页头 review。
///
/// relay 给结果卡加 card_state:review、task-read 给页头 review 共用这一处判断。
pub(crate) fn is_review_report(task: &Task, report: &Report) -> bool {
    report.state == ReportState::Ready
        && report.version == task.requirement_version()
        && crate::result_cards::write_scope_display(task).is_some()
}

/// 被把关任务的候选报告排第几次：全部 Ready 报告同一个口径（结果卡和页头判定共用）。
pub(crate) fn ready_report_ordinal(task: &Task, report_id: &ReportId) -> Option<u64> {
    task.reports
        .iter()
        .filter(|report| report.state == ReportState::Ready)
        .position(|report| &report.id == report_id)
        .map(|index| index as u64 + 1)
}

/// ：报告状态映射到协议视图（页头只在 taskStatusOf 一处把词定出来）。
pub(crate) fn task_read_report_state(state: ReportState) -> TaskReadReportState {
    match state {
        ReportState::Ready => TaskReadReportState::Ready,
        ReportState::Blocked => TaskReadReportState::Blocked,
        ReportState::Failed => TaskReadReportState::Failed,
        ReportState::Waiting => TaskReadReportState::Waiting,
    }
}

/// ：报告来源映射到协议视图（package 的 blocked 是「没开跑」，agent 的是「卡住了」）。
pub(crate) fn task_read_report_origin(origin: ReportOrigin) -> TaskReadReportOrigin {
    match origin {
        ReportOrigin::Package => TaskReadReportOrigin::Package,
        ReportOrigin::Agent => TaskReadReportOrigin::Agent,
        ReportOrigin::Executor => TaskReadReportOrigin::Executor,
        ReportOrigin::Program => TaskReadReportOrigin::Program,
    }
}

pub(crate) fn report_origin_key(origin: ReportOrigin) -> &'static str {
    match origin {
        ReportOrigin::Package => "package",
        ReportOrigin::Agent => "agent",
        ReportOrigin::Executor => "executor",
        ReportOrigin::Program => "program",
    }
}

pub(crate) fn report_state_key(state: ReportState) -> &'static str {
    match state {
        ReportState::Ready => "ready",
        ReportState::Blocked => "blocked",
        ReportState::Failed => "failed",
        ReportState::Waiting => "waiting",
    }
}

/// relay 摘要最多 600 字；更长时带原始字数（照 DSH resultRelay）。
pub(crate) fn relay_summary(text: &str) -> (String, Option<usize>) {
    let chars = text.chars().count();
    if chars > 600 {
        (text.chars().take(600).collect(), Some(chars))
    } else {
        (text.to_owned(), None)
    }
}

impl TaskRuntime {
    /// 把一条 relay Steer 进发起的 Kind 会话；那个会话不是当前根会话时 Later。
    pub(crate) async fn deliver_relay(self: &Arc<Self>, claim: EffectClaim) {
        let (task, body, request_id) = match self.relay_message(&claim) {
            Ok(Some(parts)) => parts,
            Ok(None) => {
                self.finish_or_fail(&claim, Ok(()));
                return;
            }
            Err(error) => {
                self.finish_or_fail(&claim, Err(error));
                return;
            }
        };
        // 测试钩子：在 Kind append 前模拟进程停掉，效果行原样留给下一次启动。
        if self.crash_before_relay.swap(false, Ordering::SeqCst) {
            self.shutting_down.store(true, Ordering::SeqCst);
            return;
        }
        let target = task.conversation.as_str();
        let Some(inner) = self.inner.upgrade() else {
            self.later(&claim);
            return;
        };
        let guard = inner.lock().await;
        let Some(host) = guard.as_ref() else {
            drop(guard);
            self.later(&claim);
            return;
        };
        if host.agent().id().as_str() != target {
            drop(guard);
            self.later(&claim);
            return;
        }
        let message = UserMessage::text(body.to_string());
        let sent = host
            .send_message(&request_id, message, DeliveryMode::Steer)
            .await;
        drop(guard);
        match sent {
            Ok(_) => best_effort("relay 完成没写进任务库", self.store.complete_effect(&claim)),
            Err(error) => best_effort(
                "relay 失败没写进任务库",
                self.store
                    .fail_effect(&claim, &error.to_string(), Self::now()),
            ),
        }
    }

    /// 拼一条 relay 的正文和 request id；报告和停止共用 task_report 的形状。
    ///
    /// 返回 Ok(None)：载荷指向的任务已经不在了，效果行直接收掉。
    pub(crate) fn relay_message(
        &self,
        claim: &EffectClaim,
    ) -> Result<Option<(Task, Value, String)>, String> {
        let task_id = match &claim.payload {
            EffectPayload::Relay { task_id, .. } | EffectPayload::Stop { task_id, .. } => task_id,
            EffectPayload::GateVerdict { gate_task, .. } => gate_task,
            EffectPayload::UpstreamChanged { upstream_task, .. } => upstream_task,
            EffectPayload::Launch { .. }
            | EffectPayload::Delivery { .. }
            | EffectPayload::Notice { .. }
            | EffectPayload::Restart { .. }
            | EffectPayload::AppointmentWake { .. } => {
                return Err("relay 载荷不是 relay".to_owned());
            }
        };
        let Some(task) = self
            .store
            .task(task_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let stage = self
            .store
            .stage(&task.id)
            .map_err(|error| error.to_string())?;
        match &claim.payload {
            EffectPayload::Relay {
                report_id,
                released,
                ..
            } => {
                let Some(report) = task.reports.iter().find(|report| report.id == *report_id)
                else {
                    return Err(format!("任务 {} 没有报告 {report_id}", task.id));
                };
                let (summary, chars) = relay_summary(&report.summary);
                let mut body = json!({
                    "kind": "task_report",
                    "task": {
                        "id": task.id.as_str(),
                        "title": task.title,
                        "stage": stage_key(stage),
                    },
                    "origin": report_origin_key(report.origin),
                    "state": report_state_key(report.state),
                    "summary": summary,
                });
                if let Some(chars) = chars {
                    body["chars"] = json!(chars);
                }
                if let Some(question) = &report.question {
                    body["question"] = json!(question);
                }
                // ：这份 ready 报告自动放出的下游（程序事实一行）。
                if !released.is_empty() {
                    body["auto_started"] = json!(released);
                }
                // 写任务的结果卡：写入范围、改动清单和固定一行。只有当前版本的 ready 报告
                // 才给「待你核」的卡状态（阶段仍由 task.stage 报待检查）。
                if let Some(scope) = crate::result_cards::write_scope_display(&task) {
                    let changed = crate::result_cards::changed_files(&self.data_dir, &task);
                    body["changed_line"] =
                        json!(crate::result_cards::changed_line(changed.len(), &scope));
                    body["changed"] = json!(changed);
                    body["write_scope"] = json!(scope);
                    if is_review_report(&task, report) {
                        body["card_state"] = json!("review");
                    }
                }
                // 开了 bash 的写任务：卡上照实标明 bash 的改动不在写集清单里（D85）。
                if task.bash {
                    body["bash_note"] = json!(BASH_NOTE);
                }
                // 开了工作树：卡上写路径，用户和 Kind 去核时知道改动在哪。
                if let Some(worktree) = &task.worktree_state {
                    body["worktree_path"] = json!(worktree.root.to_string_lossy());
                }
                // ：门结果卡的里程碑事实；被把关任务的卡标「等核查」。
                if let Some(gate_facts) = self.gate_card_facts(&task, report) {
                    body["gate"] = gate_facts;
                }
                // ：程序门结果卡的命令/版本/判定/逐条检查事实。
                if task.is_program_gate() {
                    let verdict = task.verdict_for_gate_report(&report.id);
                    body["program_gate"] = json!({
                        "commands": task.gate_commands().iter().map(|command| json!({
                            "command": command.command,
                            "timeout_ms": command.timeout_ms,
                        })).collect::<Vec<_>>(),
                        "commands_version": task.commands_version(),
                        "judged_by": verdict.map(|verdict| match verdict.judged_by {
                            tasks::VerdictSource::User => "user",
                            tasks::VerdictSource::Program => "program",
                        }),
                        "conclusion": verdict.map(|verdict| match verdict.conclusion {
                            GateConclusion::Pass => "pass",
                            GateConclusion::NeedsFix => "needs_fix",
                            GateConclusion::Undetermined => "undetermined",
                        }),
                        "checks": report.checks,
                    });
                }
                // ：卡上要能分清这张结果卡是哪份报告（门判定只作用在它判的那份上）。
                body["report_id"] = json!(report.id.as_str());
                // ：这份报告绑的要求版本（小标题版本 >1 时加「· 第 N 版要求」）。
                body["requirement_version"] = json!(report.version);
                let request_id = format!("task-report:{}:{}", task.id, report.id);
                Ok(Some((task, body, request_id)))
            }
            EffectPayload::Stop { attempt_id, .. } => {
                let summary = format!(
                    "用户立即停了任务 {} 的第 {} 次 Attempt。",
                    task.id, attempt_id
                );
                let body = json!({
                    "kind": "task_report",
                    "task": {
                        "id": task.id.as_str(),
                        "title": task.title,
                        "stage": stage_key(stage),
                    },
                    "origin": "executor",
                    "state": "stopped",
                    "summary": summary,
                });
                let request_id = format!("task-stop:{}:{}", task.id, attempt_id);
                Ok(Some((task, body, request_id)))
            }
            EffectPayload::GateVerdict { gate_report, .. } => {
                let gate = self
                    .store
                    .task(&task.id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("任务 {} 不在了", task.id))?;
                let Some(verdict) = gate.verdict_for_gate_report(gate_report).cloned() else {
                    return Err(format!("门 {} 没有报告 {gate_report} 的判定", gate.id));
                };
                let Some(need) = gate.needs.first() else {
                    return Err(format!("门 {} 没有把关对象", gate.id));
                };
                let guarded = self
                    .store
                    .task(&need.from)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("被把关任务 {} 不在了", need.from))?;
                let gate_report_fact = gate.reports.iter().find(|report| report.id == *gate_report);
                let body = json!({
                    "kind": "gate_verdict",
                    "gate": {"id": gate.id.as_str(), "title": gate.title},
                    "conclusion": match verdict.conclusion {
                        tasks::GateConclusion::Pass => "pass",
                        tasks::GateConclusion::NeedsFix => "needs_fix",
                        tasks::GateConclusion::Undetermined => "undetermined",
                    },
                    "candidate": {
                        "task": {"id": guarded.id.as_str(), "title": guarded.title},
                        "report_id": verdict.candidate.report_id.as_str(),
                        "requirement_version": verdict.candidate.requirement_version,
                    },
                    "note": verdict.note,
                    "judged_by": match verdict.judged_by {
                        tasks::VerdictSource::User => "user",
                        tasks::VerdictSource::Program => "program",
                    },
                    "commands_version": verdict.commands_version,
                    "checks": gate_report_fact.map(|report| report.checks.clone()).unwrap_or_default(),
                    "summary": gate_report_fact.map(|report| report.summary.clone()).unwrap_or_default(),
                });
                let request_id = format!("gate-verdict:{}:{}", gate.id, gate_report);
                Ok(Some((gate, body, request_id)))
            }
            EffectPayload::UpstreamChanged {
                upstream_task,
                downstream_task,
                upstream_report,
            } => {
                let upstream = self
                    .store
                    .task(upstream_task)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("上游任务 {upstream_task} 不在了"))?;
                let downstream = self
                    .store
                    .task(downstream_task)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("下游任务 {downstream_task} 不在了"))?;
                let report = upstream
                    .reports
                    .iter()
                    .find(|report| report.id == *upstream_report)
                    .ok_or_else(|| format!("上游任务没有报告 {upstream_report}"))?;
                let body = json!({
                    "kind": "upstream_changed",
                    "upstream": {
                        "id": upstream.id.as_str(),
                        "title": upstream.title,
                        "report_id": report.id.as_str(),
                        "requirement_version": report.version,
                        "summary": report.summary,
                    },
                    "downstream": {"id": downstream.id.as_str(), "title": downstream.title},
                });
                let request_id = format!(
                    "upstream-changed:{}:{}:{}",
                    upstream.id, downstream.id, upstream_report
                );
                Ok(Some((upstream, body, request_id)))
            }
            EffectPayload::Launch { .. }
            | EffectPayload::Delivery { .. }
            | EffectPayload::Notice { .. }
            | EffectPayload::Restart { .. }
            | EffectPayload::AppointmentWake { .. } => Err("relay 载荷不是 relay".to_owned()),
        }
    }

    /// turn 结束但还没上报：照 DSH 合成 waiting；有最后一段回复就带它，没有就提示去问。
    pub(crate) async fn turn_ended(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &SessionId,
    ) {
        let Ok(Some(task)) = self.store.task(task_id) else {
            return;
        };
        if task.latest_attempt().map(|attempt| attempt.id) != Some(attempt_id) {
            return;
        }
        // 轮次结束就释放席位；终态报告/停止也走这里，释放幂等。
        self.release_seat(task_id.as_str());
        // 停止已经把这个 Attempt 的结果定死：取消后的收尾不再合成 waiting。
        if task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_some())
        {
            return;
        }
        if task.latest_report().is_some_and(|report| {
            matches!(
                report.state,
                ReportState::Ready | ReportState::Blocked | ReportState::Failed
            )
        }) {
            return;
        }
        // 用户自己停下这一轮：不算 ThinAgent 没上报，不合成 waiting，Attempt 留着接着做。
        if self.last_turn_was_user_aborted(conversation) {
            return;
        }
        // 收尾停：turn 结束还没报，先走 的 waiting 合成，再把这个 Attempt 记成 stopped。
        if self.after_report_requested(conversation) {
            self.settle_after_report(task_id, attempt_id, conversation);
            return;
        }
        self.record_waiting(task_id, attempt_id, conversation);
        self.wake();
    }

    /// 这个会话最后一轮是不是用户取消的（停下这一轮；立即停会先把 Attempt 记成结束，走不到这里）。
    pub(crate) fn last_turn_was_user_aborted(&self, conversation: &SessionId) -> bool {
        let Ok(store) = session::SessionStore::open_read_only(&self.data_dir, conversation) else {
            return false;
        };
        store
            .events()
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                session::EventKind::TurnEnd { reason, .. } => Some(matches!(
                    reason,
                    session::TurnEndReason::Aborted {
                        reason: session::AbortCause::User
                    }
                )),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// 这个会话有没有收尾停标记（在跑表里查；会话摘掉之后就是 false）。
    pub(crate) fn after_report_requested(&self, conversation: &SessionId) -> bool {
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .get(conversation.as_str())
            .is_some_and(|entry| entry.host.agent().stop_after_report_requested())
    }

    /// turn 结束还没报：照 合成 waiting（有最后一段回复就带它，没有就提示去问）。
    pub(crate) fn record_waiting(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &SessionId,
    ) {
        let said = self.last_assistant_text(conversation);
        let summary = if said.trim().is_empty() {
            WAITING_FALLBACK.to_owned()
        } else {
            said
        };
        if let Err(error) = self.store.record_report(
            task_id,
            attempt_id,
            NewReport {
                state: ReportState::Waiting,
                origin: ReportOrigin::Executor,
                summary,
                question: None,
            },
            Self::now(),
        ) {
            eprintln!("任务 {task_id} 的等待报告没写进去：{error}");
        }
    }

    /// 收尾停的收尾：waiting 合成之后把这个 Attempt 记成 stopped，并从在跑表摘掉会话。
    ///
    /// 幂等：Attempt 已经结束、已经有终态报告、或已经不是最新 Attempt 时什么都不做。
    pub(crate) fn settle_after_report(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &SessionId,
    ) {
        let Ok(Some(task)) = self.store.task(task_id) else {
            return;
        };
        if task.latest_attempt().map(|attempt| attempt.id) != Some(attempt_id) {
            return;
        }
        if task
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_some())
        {
            return;
        }
        if task.latest_report().is_some_and(|report| {
            matches!(
                report.state,
                ReportState::Ready | ReportState::Blocked | ReportState::Failed
            )
        }) {
            return;
        }
        self.record_waiting(task_id, attempt_id, conversation);
        if let Err(error) =
            self.store
                .end_attempt(task_id, attempt_id, Self::now(), AttemptEnd::Stopped)
        {
            eprintln!("任务 {task_id} 的收尾停没记进任务库：{error}");
            return;
        }
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .remove(conversation.as_str());
        self.release_seat(task_id.as_str());
        self.wake();
    }

    /// ThinAgent 会话里最后一段助手文字（turn 结束时的最后回复）。
    pub(crate) fn last_assistant_text(&self, conversation: &SessionId) -> String {
        let Ok(store) = session::SessionStore::open_read_only(&self.data_dir, conversation) else {
            return String::new();
        };
        store
            .events()
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                session::EventKind::AssistantMessage { message, .. } => Some(
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                ),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// ThinAgent 报了一份报告：同事务写报告 + 排 relay；终态就是这次 Attempt 的结束，关掉会话。
    pub(crate) fn report(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &str,
        report: NewReport,
    ) -> Result<(ReportId, bool), TaskError> {
        let state = report.state;
        let duplicate = self
            .store
            .task(task_id)?
            .and_then(|task| task.latest_report().map(|latest| latest.state))
            == Some(state);
        let id = self
            .store
            .record_report(task_id, attempt_id, report, Self::now())?;
        if matches!(
            state,
            ReportState::Ready | ReportState::Blocked | ReportState::Failed
        ) {
            // 照 spec：报了以后 Attempt 以 reported 结束，卡照报的状态出。报告已经落库，
            // 结束原因写失败不该把它变成工具错误（报过的结果不能被重报）。
            best_effort(
                "task_report 的 Attempt 没收尾",
                self.store
                    .end_attempt(task_id, attempt_id, Self::now(), AttemptEnd::Reported),
            );
            self.running
                .lock()
                .expect("thinagent 表锁被毒化")
                .remove(conversation);
            self.release_seat(task_id.as_str());
        }
        self.wake();
        Ok((id, duplicate))
    }

    /// ThinAgent 交回：同事务写一份 blocked 报告（问题由程序写死）并排 relay；
    /// 这个 Attempt 以 reported 结束。Kind 收到 relay 以后怎么问，由 Kind 自己说。
    pub(crate) fn handback(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &str,
        reason: String,
    ) -> Result<String, ToolFailure> {
        let report = NewReport {
            state: ReportState::Blocked,
            origin: ReportOrigin::Agent,
            summary: reason,
            question: Some(HANDBACK_QUESTION.to_owned()),
        };
        let (id, created) = self
            .store
            .record_handback_report(task_id, attempt_id, report, Self::now())
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        // 同一个 Attempt 只交回一次：已有 blocked 报告（含并行的第一次交回）就是「已交回」。
        if !created {
            return Ok("已交回".to_owned());
        }
        // 和 task_report 一样：报告落库后结束这个 Attempt；结束写失败不改写工具回执。
        best_effort(
            "交回的 Attempt 没收尾",
            self.store
                .end_attempt(task_id, attempt_id, Self::now(), AttemptEnd::Reported),
        );
        self.running
            .lock()
            .expect("thinagent 表锁被毒化")
            .remove(conversation);
        self.release_seat(task_id.as_str());
        self.wake();
        Ok(format!(
            "已交回 Kind（报告 {id}）。Kind 会问用户怎么办；不要再自己做这件事。"
        ))
    }

    /// task_report 的实际落点：写报告 + 排 relay（store 同一事务），返回回执。
    pub(crate) fn report_from_tool(
        &self,
        task_id: &TaskId,
        attempt_id: AttemptId,
        conversation: &str,
        report: NewReport,
    ) -> Result<String, ToolFailure> {
        let state = report.state;
        let (id, duplicate) = self
            .report(task_id, attempt_id, conversation, report)
            .map_err(|error| ToolFailure::plain(error.to_string()))?;
        if duplicate {
            Ok(format!(
                "这个 Attempt 已经报过 {state:?}（报告 {id}）；没有重复上报。"
            ))
        } else {
            Ok(format!(
                "已上报 {state:?}（报告 {id}）。结果会作为 task_report 消息送到 Kind。"
            ))
        }
    }
}
