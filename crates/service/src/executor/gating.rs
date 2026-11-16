//! 程序门与门槛：判定/重跑/停止、门检查运行与结果解析。

use super::*;

/// ：正在跑的程序门检查（没有会话；停任务按任务 id 找到它）。
pub(crate) struct RunningGate {
    pub(crate) attempt_id: AttemptId,
    pub(crate) registry: Arc<TaskRegistry>,
    pub(crate) job_id: Mutex<Option<String>>,
    pub(crate) current_index: Mutex<Option<usize>>,
    pub(crate) stop_reason: Mutex<Option<String>>,
    pub(crate) cancelled: AtomicBool,
}

pub(crate) fn task_read_gate_checks(
    checks: &[GateCheck],
) -> Vec<crate::protocol::TaskReadGateCheck> {
    checks
        .iter()
        .map(|check| crate::protocol::TaskReadGateCheck {
            index: check.index,
            command: check.command.clone(),
            exit_code: check.exit_code,
            outcome: gate_check_outcome_key(check.outcome).to_owned(),
            duration_ms: check.duration_ms,
            tail: check.tail.clone(),
            spill: check.spill.clone(),
        })
        .collect()
}

pub(crate) fn gate_check_outcome_key(outcome: GateCheckOutcome) -> &'static str {
    match outcome {
        GateCheckOutcome::Ran => "ran",
        GateCheckOutcome::TimedOut => "timed_out",
        GateCheckOutcome::SpawnFailed => "spawn_failed",
        GateCheckOutcome::Stopped => "stopped",
        GateCheckOutcome::Skipped => "skipped",
    }
}

pub(crate) fn stop_reason(running: &RunningGate) -> String {
    running
        .stop_reason
        .lock()
        .expect("程序门停因锁被毒化")
        .clone()
        .unwrap_or_else(|| "被停".to_owned())
}

/// ：没跑的那条占一行。
pub(crate) fn gate_check_skipped(index: usize, command: &GateCommand, why: &str) -> GateCheck {
    GateCheck {
        index,
        command: command.command.trim().to_owned(),
        exit_code: None,
        outcome: GateCheckOutcome::Skipped,
        duration_ms: 0,
        tail: why.to_owned(),
        spill: None,
    }
}

/// ：读输出文件的尾部（最多 MAX_OUTPUT_BYTES），给程序门报告。
pub(crate) fn tail_file(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(MAX_OUTPUT_BYTES as u64);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

pub(crate) fn gate_tail(read: &JobRead) -> String {
    let mut bytes = std::fs::read(&read.stdout_path)
        .ok()
        .and_then(|_| tail_file(&read.stdout_path))
        .unwrap_or_else(|| read.stdout.as_bytes().to_vec());
    let stderr = tail_file(&read.stderr_path)
        .filter(|bytes| !bytes.is_empty())
        .unwrap_or_else(|| read.stderr.as_bytes().to_vec());
    if !stderr.is_empty() {
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(b"[stderr]\n");
        bytes.extend_from_slice(&stderr);
    }
    while bytes.len() > MAX_OUTPUT_BYTES {
        bytes.remove(0);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(crate) fn gate_spill(read: &JobRead) -> Option<String> {
    if read.stdout_path.as_os_str().is_empty() {
        (!read.stderr_path.as_os_str().is_empty()).then(|| read.stderr_path.display().to_string())
    } else {
        Some(read.stdout_path.display().to_string())
    }
}

impl TaskRuntime {
    /// ：这个任务是不是里程碑门（结果卡判定前先认）。
    pub(crate) fn task_is_gate(&self, id: &TaskId) -> Result<bool, String> {
        self.store
            .task(id)
            .map(|task| task.is_some_and(|task| task.is_gate()))
            .map_err(|error| error.to_string())
    }

    /// ：门判定内部操作；同一事务核对候选/去重/记判定/排 relay。
    ///
    /// 「已经变了」不是错误：返回 changed=true、不记判定，让卡片收成回执。
    pub(crate) fn gate_judge(
        &self,
        request: &GateJudgeRequest,
    ) -> Result<GateJudgeResponse, String> {
        let gate_id = TaskId::from(request.gate_task.as_str());
        let gate_report = ReportId::from(request.gate_report.as_str());
        let conclusion = match request.conclusion {
            ProtocolGateConclusion::Pass => GateConclusion::Pass,
            ProtocolGateConclusion::NeedsFix => GateConclusion::NeedsFix,
        };
        match self.store.judge_gate(
            &gate_id,
            &gate_report,
            conclusion,
            request.note.clone(),
            Self::now(),
        ) {
            Ok(outcome) => {
                if let Ok(Some(gate)) = self.store.task(&gate_id)
                    && let Some(need) = gate.needs.first()
                {
                    self.notify_task_changed(&need.from);
                }
                self.notify_task_changed(&gate_id);
                self.wake();
                let recorded = match outcome.verdict.conclusion {
                    GateConclusion::Pass => ProtocolGateConclusion::Pass,
                    GateConclusion::NeedsFix => ProtocolGateConclusion::NeedsFix,
                    GateConclusion::Undetermined => {
                        return Err("未确定不是用户判定的结论".to_owned());
                    }
                };
                let summary = match outcome.verdict.conclusion {
                    GateConclusion::Pass => "你判了通过",
                    GateConclusion::NeedsFix => "你判了需修正",
                    GateConclusion::Undetermined => "未确定",
                };
                Ok(GateJudgeResponse {
                    verdict: Some(GateVerdictView {
                        gate_report: outcome.verdict.gate_report.as_str().to_owned(),
                        candidate_report: outcome.verdict.candidate.report_id.as_str().to_owned(),
                        requirement_version: outcome.verdict.candidate.requirement_version,
                        conclusion: recorded,
                        note: outcome.verdict.note.clone(),
                        judged_by: Some(match outcome.verdict.judged_by {
                            tasks::VerdictSource::User => "user".to_owned(),
                            tasks::VerdictSource::Program => "program".to_owned(),
                        }),
                        commands_version: outcome.verdict.commands_version,
                        at: outcome.verdict.at,
                    }),
                    changed: false,
                    duplicate: outcome.duplicate,
                    summary: summary.to_owned(),
                    released: outcome.released,
                })
            }
            Err(TaskError::GateReportChanged) => Ok(GateJudgeResponse {
                verdict: None,
                changed: true,
                duplicate: false,
                summary: "已经变了：没有记判定".to_owned(),
                released: Vec::new(),
            }),
            Err(error) => Err(error.to_string()),
        }
    }

    /// ：Kind 的 gate_check_again——只允许未确定/被打断、候选没变、命令版本没变。
    pub(crate) fn gate_check_again(&self, gate_id: &TaskId) -> Result<(), String> {
        let task = self
            .store
            .task(gate_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("找不到这个任务：{gate_id}"))?;
        if !task.is_gate() {
            return Err("这不是门任务".to_owned());
        }
        if !task.is_program_gate() {
            return Err("这是用户判的里程碑门，要重判请在结果卡上点".to_owned());
        }
        if self
            .gate_jobs
            .lock()
            .expect("程序门表锁被毒化")
            .contains_key(gate_id.as_str())
        {
            return Err("程序门现在正在跑，等这一轮结束".to_owned());
        }
        let attempt = task
            .latest_attempt()
            .cloned()
            .ok_or_else(|| "程序门还没开检查过".to_owned())?;
        if attempt.ended_at.is_none() {
            return Err("程序门现在正在跑，等这一轮结束".to_owned());
        }
        let need = task
            .needs
            .first()
            .ok_or_else(|| "程序门没有把关对象".to_owned())?;
        let handoff = need
            .handoff
            .clone()
            .ok_or_else(|| "被把关任务还没有可核的结果".to_owned())?;
        let guarded = self
            .store
            .task(&need.from)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "被把关任务不在了".to_owned())?;
        let current = guarded
            .latest_report()
            .filter(|report| report.state == ReportState::Ready);
        if !current.is_some_and(|report| report.id == handoff.report_id) {
            return Err("候选结果已经变了，要等新的结果或改命令".to_owned());
        }
        let verdict = task
            .latest_report()
            .and_then(|report| task.verdict_for_gate_report(&report.id));
        if let Some(verdict) = verdict
            && verdict.candidate.report_id != handoff.report_id
        {
            return Err("候选结果已经变了，要等新的结果或改命令".to_owned());
        }
        if let Some(verdict) = verdict
            && verdict.commands_version != task.commands_version()
        {
            return Err("命令已经改过，这一版还没跑；用新版本重跑".to_owned());
        }
        let interrupted = attempt.end == Some(AttemptEnd::Interrupted);
        match verdict {
            Some(verdict) if verdict.conclusion == GateConclusion::Undetermined || interrupted => {
                self.queue_launch(gate_id)
            }
            None if interrupted => self.queue_launch(gate_id),
            Some(_) => Err("最新结论不是未确定/被打断，不用重跑".to_owned()),
            None => Err("程序门还没有未确定的结论".to_owned()),
        }
    }

    /// ：程序门按顺序跑命令；不开会话、不占席位。
    pub(crate) async fn run_program_gate(
        self: Arc<Self>,
        gate_id: TaskId,
        attempt_id: AttemptId,
        registry: Arc<TaskRegistry>,
        running: Arc<RunningGate>,
    ) {
        let cleanup = |runtime: &Arc<Self>| {
            let mut jobs = runtime.gate_jobs.lock().expect("程序门表锁被毒化");
            if jobs
                .get(gate_id.as_str())
                .is_some_and(|entry| entry.attempt_id == attempt_id)
            {
                jobs.remove(gate_id.as_str());
            }
        };
        let task = match self.store.task(&gate_id) {
            Ok(Some(task)) => task,
            _ => {
                cleanup(&self);
                return;
            }
        };
        let commands = task.gate_commands().to_vec();
        let cwd = task.effective_cwd().to_path_buf();
        let mut checks: Vec<GateCheck> = Vec::new();
        let mut undetermined: Option<String> = None;
        for (index, command) in commands.iter().enumerate() {
            if running.cancelled.load(Ordering::SeqCst) {
                let reason = stop_reason(&running);
                checks.push(GateCheck {
                    index,
                    command: command.command.trim().to_owned(),
                    exit_code: None,
                    outcome: GateCheckOutcome::Stopped,
                    duration_ms: 0,
                    tail: reason.clone(),
                    spill: None,
                });
                for rest in commands.iter().skip(index + 1) {
                    checks.push(gate_check_skipped(checks.len(), rest, "没跑"));
                }
                undetermined = Some(reason);
                break;
            }
            let spec = JobSpec {
                command: command.command.trim().to_owned(),
                cwd: cwd.clone(),
                env: tools::command_env(),
                label: command
                    .command
                    .trim()
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned(),
                tool_call: None,
            };
            let job_id = match registry.start(spec, true) {
                Ok(id) => id,
                Err(error) => {
                    checks.push(GateCheck {
                        index,
                        command: command.command.trim().to_owned(),
                        exit_code: None,
                        outcome: GateCheckOutcome::SpawnFailed,
                        duration_ms: 0,
                        tail: error.message,
                        spill: None,
                    });
                    for rest in commands.iter().skip(index + 1) {
                        checks.push(gate_check_skipped(checks.len(), rest, "没跑"));
                    }
                    undetermined = Some(format!("起不来：{}", command.command.trim()));
                    break;
                }
            };
            *running.job_id.lock().expect("程序门 job 锁被毒化") = Some(job_id.clone());
            *running
                .current_index
                .lock()
                .expect("程序门当前命令锁被毒化") = Some(index);
            let waited = registry
                .wait_async(&job_id, Duration::from_millis(command.timeout_ms()))
                .await;
            let (read, outcome) = match waited {
                Ok(read) if read.view.status.is_terminal() => {
                    let outcome = if running.cancelled.load(Ordering::SeqCst) {
                        GateCheckOutcome::Stopped
                    } else {
                        GateCheckOutcome::Ran
                    };
                    (read, outcome)
                }
                Ok(read) => {
                    if let Err(error) = registry.request_kill(
                        &job_id,
                        Some(format!(
                            "程序门命令超时（{} 秒）",
                            command.timeout_ms() / 1000
                        )),
                        false,
                    ) {
                        eprintln!("程序门超时结束命令失败：{}", error.message);
                    }
                    let read = registry
                        .await_termination(&job_id, tools::KILL_GRACE)
                        .await
                        .unwrap_or(read);
                    let outcome = if running.cancelled.load(Ordering::SeqCst) {
                        GateCheckOutcome::Stopped
                    } else {
                        GateCheckOutcome::TimedOut
                    };
                    (read, outcome)
                }
                Err(error) => {
                    let read = JobRead {
                        view: tools::JobView {
                            id: job_id.clone(),
                            kind: "bash",
                            label: String::new(),
                            status: tools::JobStatus::Failed,
                            exit_code: None,
                            signal: None,
                            started_at: 0,
                            finished_at: Some(0),
                            detail: Some(error.message.clone()),
                        },
                        stdout: String::new(),
                        stderr: error.message,
                        stdout_dropped: false,
                        stderr_dropped: false,
                        stdout_path: PathBuf::new(),
                        stderr_path: PathBuf::new(),
                        stdout_spill: None,
                        stderr_spill: None,
                    };
                    (read, GateCheckOutcome::SpawnFailed)
                }
            };
            let duration_ms = read
                .view
                .finished_at
                .unwrap_or(read.view.started_at)
                .saturating_sub(read.view.started_at);
            checks.push(GateCheck {
                index,
                command: command.command.trim().to_owned(),
                exit_code: read.view.exit_code,
                outcome,
                duration_ms,
                tail: gate_tail(&read),
                spill: gate_spill(&read),
            });
            *running.job_id.lock().expect("程序门 job 锁被毒化") = None;
            *running
                .current_index
                .lock()
                .expect("程序门当前命令锁被毒化") = None;
            if outcome != GateCheckOutcome::Ran {
                for rest in commands.iter().skip(index + 1) {
                    checks.push(gate_check_skipped(checks.len(), rest, "没跑"));
                }
                undetermined = Some(match outcome {
                    GateCheckOutcome::TimedOut => format!(
                        "超时：{}（{} 秒）",
                        command.command.trim(),
                        command.timeout_ms() / 1000
                    ),
                    GateCheckOutcome::Stopped => stop_reason(&running),
                    GateCheckOutcome::SpawnFailed => "起不来".to_owned(),
                    _ => "没跑完".to_owned(),
                });
                break;
            }
        }
        cleanup(&self);
        let conclusion = if undetermined.is_some() {
            GateConclusion::Undetermined
        } else if checks
            .iter()
            .any(|check| check.exit_code.is_some_and(|code| code != 0))
        {
            GateConclusion::NeedsFix
        } else {
            GateConclusion::Pass
        };
        let summary = match conclusion {
            GateConclusion::Pass => format!("程序门通过：{} 条命令全部退出码 0", commands.len()),
            GateConclusion::NeedsFix => {
                let bad = checks
                    .iter()
                    .filter(|check| check.exit_code.is_some_and(|code| code != 0))
                    .count();
                let first = checks
                    .iter()
                    .find(|check| check.exit_code.is_some_and(|code| code != 0));
                match first {
                    Some(check) => format!(
                        "{} 条命令：{} 条退出码非 0（{} → {}）",
                        commands.len(),
                        bad,
                        check.command,
                        check.exit_code.unwrap_or(0)
                    ),
                    None => "程序门需修正".to_owned(),
                }
            }
            GateConclusion::Undetermined => format!(
                "程序门未确定：{}",
                undetermined.unwrap_or_else(|| "没跑完".to_owned())
            ),
        };
        if let Err(error) = self.store.record_program_gate_result(
            &gate_id,
            attempt_id,
            summary,
            checks,
            conclusion,
            Self::now(),
        ) {
            eprintln!("程序门 {gate_id} 的结果没写进任务库：{error}");
        }
        if let Ok(Some(gate)) = self.store.task(&gate_id)
            && let Some(need) = gate.needs.first()
        {
            self.notify_task_changed(&need.from);
        }
        self.notify_task_changed(&gate_id);
        self.wake();
    }

    /// 停一个任务：立即停走「先持久、再取消」；收尾停走「插固定说明、打标记、等报」。
    ///
    /// 页面（Call::TaskStop）和 Kind 的 task_stop 都调到这一个方法。
    /// ：改命令前停门；停那次检查的原因写「命令已改」。
    pub(crate) async fn stop_gate_for_update(
        &self,
        task_id: &str,
    ) -> Result<crate::protocol::TaskStopResponse, ServiceError> {
        let id = TaskId::from(task_id);
        let Some(attempt) = self
            .store
            .task(&id)
            .map_err(|error| ServiceError::TaskUnavailable(error.to_string()))?
            .and_then(|task| task.latest_attempt().cloned())
        else {
            return Ok(crate::protocol::TaskStopResponse {
                stopped: false,
                stage: self
                    .store
                    .stage(&id)
                    .map(|stage| stage_key(stage).to_owned())
                    .unwrap_or_default(),
                message: NOT_RUNNING.to_owned(),
            });
        };
        if attempt.ended_at.is_some() {
            return Ok(crate::protocol::TaskStopResponse {
                stopped: false,
                stage: self
                    .store
                    .stage(&id)
                    .map(|stage| stage_key(stage).to_owned())
                    .unwrap_or_default(),
                message: NOT_RUNNING.to_owned(),
            });
        }
        self.stop_program_gate(task_id, &id, attempt.id, "命令已改")
            .await
    }

    /// ：停程序门——结束当前进程组；结果由 run_program_gate 写成「未确定」。
    pub(crate) async fn stop_program_gate(
        &self,
        task_id: &str,
        id: &TaskId,
        attempt_id: AttemptId,
        reason: &str,
    ) -> Result<crate::protocol::TaskStopResponse, ServiceError> {
        let running = self
            .gate_jobs
            .lock()
            .expect("程序门表锁被毒化")
            .get(id.as_str())
            .filter(|running| running.attempt_id == attempt_id)
            .cloned();
        let Some(running) = running else {
            let stage = self
                .store
                .stage(id)
                .map_err(|error| ServiceError::TaskUnavailable(format!("阶段读不出来：{error}")))?;
            return Ok(crate::protocol::TaskStopResponse {
                stopped: false,
                stage: stage_key(stage).to_owned(),
                message: NOT_RUNNING.to_owned(),
            });
        };
        *running.stop_reason.lock().expect("程序门停因锁被毒化") = Some(reason.to_owned());
        running.cancelled.store(true, Ordering::SeqCst);
        let job = running.job_id.lock().expect("程序门 job 锁被毒化").clone();
        if let Some(job_id) = job {
            if let Err(error) =
                running
                    .registry
                    .request_kill(&job_id, Some(STOPPED_JOB_REASON.to_owned()), false)
            {
                eprintln!("停程序门命令失败：{}", error.message);
            }
            if let Err(error) = running
                .registry
                .await_termination(&job_id, tools::KILL_GRACE)
                .await
            {
                eprintln!("等程序门命令结束失败：{}", error.message);
            }
        }
        let stage = self
            .store
            .stage(id)
            .map_err(|error| ServiceError::TaskUnavailable(format!("阶段读不出来：{error}")))?;
        Ok(crate::protocol::TaskStopResponse {
            stopped: true,
            stage: stage_key(stage).to_owned(),
            message: format!(
                "已停程序门：结束正在跑的命令，这次检查记未确定（任务 {task_id}，阶段：{}）。",
                stage_label(stage)
            ),
        })
    }

    /// ：结果卡上的里程碑事实（门卡 = 等你判 + 核查对象；被把关卡 = 等核查）。
    pub(crate) fn gate_card_facts(&self, task: &Task, report: &Report) -> Option<Value> {
        if report.state != ReportState::Ready {
            return None;
        }
        if task.is_gate() {
            let need = task.needs.first()?;
            let guarded = self.store.task(&need.from).ok().flatten()?;
            let candidate = task.latest_attempt()?.handoffs.first()?.clone();
            let ordinal = ready_report_ordinal(&guarded, &candidate.report_id)?;
            return Some(json!({
                "milestone": true,
                "state": "pending_verdict",
                "guarded": {"id": guarded.id.as_str(), "title": guarded.title},
                "candidate": {
                    "report_id": candidate.report_id.as_str(),
                    "requirement_version": candidate.requirement_version,
                    "ordinal": ordinal,
                },
            }));
        }
        let parent = task.parent.as_ref()?;
        for sibling in self.store.children(parent).ok()? {
            if sibling.is_gate()
                && sibling
                    .needs
                    .first()
                    .is_some_and(|need| need.from == task.id)
            {
                // 这不是门第一次核：卡上自动行要说「核查自动重跑」。
                let recheck = sibling.latest_attempt().is_some();
                return Some(json!({
                    "milestone": true,
                    "state": "waiting",
                    "id": sibling.id.as_str(),
                    "title": sibling.title,
                    "recheck": recheck,
                    "program": sibling.is_program_gate(),
                    "commands": sibling.gate_commands().iter().map(|command| command.command.clone()).collect::<Vec<_>>(),
                }));
            }
        }
        None
    }
}
