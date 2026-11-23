//! 循环约定——报回后送「第 N 轮」、程序事实暂停、工作快照等你项、继续准入。

use super::*;
use crate::work::WaitItem;
use calendar::{AppointmentStatus, NewRun, RunResult, TriggerKind};

impl TaskRuntime {
    /// 根会话里一条 task-report 送到后调：找绑这个任务、在用的循环约定。
    pub(crate) fn calendar_on_task_report(&self, task_id: &str) {
        let Ok(store) = self.calendar() else {
            return;
        };
        let Ok(Some(task)) = self.store.task(&TaskId::from(task_id)) else {
            return;
        };
        let Ok(stage) = self.store.stage(&task.id) else {
            return;
        };
        let Ok(appointments) = store.list_appointments() else {
            return;
        };
        for appointment in appointments {
            if appointment.status != AppointmentStatus::Active {
                continue;
            }
            let Some(cycle) = &appointment.cycle else {
                continue;
            };
            if cycle.task_id != task_id {
                continue;
            }
            if stage == Stage::Closed {
                if let Err(error) = store.set_status(&appointment.id, AppointmentStatus::Ended) {
                    eprintln!("循环约定结束失败：{error}");
                }
                self.emit_calendar_changed(&appointment.id);
                continue;
            }
            let Some(report) = task.latest_report() else {
                continue;
            };
            let mut pause_reason = match report.state {
                ReportState::Blocked => Some("卡住了"),
                ReportState::Failed => Some("失败"),
                _ => None,
            };
            if pause_reason.is_none() && self.cycle_no_progress(&task) {
                pause_reason = Some("连续 3 轮没有进展");
            }
            if pause_reason.is_none() {
                let Ok(runs) = store.runs(&appointment.id) else {
                    continue;
                };
                let rounds = runs
                    .iter()
                    .filter(|run| {
                        run.trigger == TriggerKind::Report && run.result != RunResult::Skipped
                    })
                    .count();
                if cycle
                    .max_rounds
                    .is_some_and(|max| rounds >= usize::try_from(max).unwrap_or(usize::MAX))
                {
                    pause_reason = Some("到了约定的轮数");
                }
            }
            if let Some(reason) = pause_reason {
                if let Err(error) = store.pause_with_reason(&appointment.id, reason) {
                    eprintln!("循环约定暂停失败：{error}");
                }
                self.emit_calendar_changed(&appointment.id);
                continue;
            }
            let Ok(runs) = store.runs(&appointment.id) else {
                continue;
            };
            let round = runs
                .iter()
                .filter(|run| run.trigger == TriggerKind::Report)
                .count()
                + 1;
            let now = Self::now();
            let Ok((run, created)) = store.record_run(NewRun {
                appointment_id: appointment.id.clone(),
                nominal_at: now,
                manual: false,
                trigger: TriggerKind::Report,
                reason: format!("第 {round} 轮报回"),
                merged_count: 0,
                report_id: Some(report.id.to_string()),
            }) else {
                continue;
            };
            if created && !self.calendar_effect_pending(run.id) {
                if let Err(error) = self.store.enqueue_effect(
                    EffectPayload::AppointmentWake {
                        appointment_id: appointment.id.clone(),
                        run_id: run.id,
                    },
                    now,
                ) {
                    eprintln!("循环约定唤醒没排上：{error}");
                    continue;
                }
                self.wake();
            }
            self.emit_calendar_changed(&appointment.id);
        }
    }

    /// task_continue 的「循环约定到点」：约定在用、绑的就是这个任务、最近一次运行已送。
    pub(crate) fn calendar_cycle_continue_cause(
        &self,
        task: &Task,
    ) -> Result<Option<ContinueCause>, String> {
        let store = self.calendar()?;
        let Some(latest_report) = task.latest_report() else {
            return Ok(None);
        };
        let appointments = store
            .list_appointments()
            .map_err(|error| error.to_string())?;
        for appointment in appointments {
            if appointment.status != AppointmentStatus::Active {
                continue;
            }
            let Some(cycle) = &appointment.cycle else {
                continue;
            };
            if cycle.task_id != task.id.as_str() {
                continue;
            }
            let runs = store
                .runs(&appointment.id)
                .map_err(|error| error.to_string())?;
            let Some(last) = runs.last() else {
                continue;
            };
            if last.trigger == TriggerKind::Report
                && last.result == RunResult::Sent
                && last.report_id.as_deref() == Some(latest_report.id.as_str())
            {
                return Ok(Some(ContinueCause::CycleDue {
                    appointment_id: appointment.id.clone(),
                    run_id: last.id,
                }));
            }
        }
        Ok(None)
    }

    /// 测试与诊断：按任务 id 看循环约定到点的准入。
    pub(crate) fn calendar_cycle_continue_cause_for(
        &self,
        task_id: &str,
    ) -> Result<Option<ContinueCause>, String> {
        let task = self
            .store
            .task(&TaskId::from(task_id))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("找不到这个任务：{task_id}"))?;
        self.calendar_cycle_continue_cause(&task)
    }

    /// 工作快照里的「等你」：被程序暂停的循环约定。
    pub(crate) fn calendar_pause_waits(&self) -> Result<Vec<WaitItem>, String> {
        let store = self.calendar()?;
        crate::work::calendar_pause_waits(store)
    }

    fn cycle_no_progress(&self, task: &Task) -> bool {
        if task.reports.len() < 3 {
            return false;
        }
        let signatures: Vec<String> = task
            .reports
            .iter()
            .rev()
            .take(3)
            .map(|report| report_signature(&self.data_dir, task, report))
            .collect();
        cycle_signatures_stall(&signatures)
    }
}

/// 这份报告所属 Attempt 的改动路径（不拿整个任务累计，避免每轮都带上前几轮的文件）。
fn report_changed_paths(data_dir: &std::path::Path, task: &Task, report: &Report) -> Vec<String> {
    let Some(attempt) = task.attempt(report.attempt_id) else {
        return Vec::new();
    };
    let Some(conversation) = &attempt.conversation else {
        return Vec::new();
    };
    let Ok(session_id) = SessionId::parse(conversation.as_str()) else {
        return Vec::new();
    };
    let Ok(snapshot) = session::SessionStore::open_read_only(data_dir, &session_id) else {
        return Vec::new();
    };
    crate::result_cards::changed_paths(snapshot.events(), task.effective_cwd())
}

fn signature_with_files(report: &Report, checks: &[String], files: &[String]) -> String {
    format!("{:?}|{checks:?}|{files:?}", report.state)
}

/// 连续 3 份签名一样就算没进展；单独抽出来给测试直接看签名序列。
fn cycle_signatures_stall(signatures: &[String]) -> bool {
    signatures.len() >= 3 && signatures.windows(2).all(|pair| pair[0] == pair[1])
}

fn report_signature(data_dir: &std::path::Path, task: &Task, report: &Report) -> String {
    let checks: Vec<String> = report
        .checks
        .iter()
        .map(|check| {
            format!(
                "{}:{:?}:{:?}",
                check.command, check.outcome, check.exit_code
            )
        })
        .collect();
    let files = report_changed_paths(data_dir, task, report);
    signature_with_files(report, &checks, &files)
}

#[cfg(test)]
mod signature_tests {
    use super::*;
    use tasks::{AttemptId, ReportId, ReportOrigin, ReportState};

    fn report(attempt: u64) -> Report {
        Report {
            id: ReportId::from(format!("report-{attempt}")),
            attempt_id: AttemptId(attempt),
            state: ReportState::Ready,
            origin: ReportOrigin::Agent,
            summary: "done".to_owned(),
            question: None,
            at: 1_794_936_000_000 + i64::try_from(attempt).unwrap(),
            version: 1,
            checks: Vec::new(),
            changed_count: None,
        }
    }

    #[test]
    fn different_files_per_attempt_make_different_signatures() {
        let first = signature_with_files(&report(1), &[], &["a.rs".to_owned()]);
        let second = signature_with_files(&report(2), &[], &["b.rs".to_owned()]);
        let third = signature_with_files(&report(3), &[], &["c.rs".to_owned()]);
        assert_ne!(first, second);
        assert_ne!(second, third);
    }

    #[test]
    fn same_files_and_checks_keep_the_signature() {
        let files = vec!["a.rs".to_owned()];
        let checks = vec!["cargo test:false".to_owned()];
        assert_eq!(
            signature_with_files(&report(1), &checks, &files),
            signature_with_files(&report(2), &checks, &files)
        );
    }
}

#[cfg(test)]
mod attempt_signature_tests {
    use super::*;
    use serde_json::json;

    fn write_events(path: &std::path::Path) -> Vec<session::EventKind> {
        vec![
            session::EventKind::ToolCall {
                turn: 1,
                step: 1,
                call_id: "call-write".to_owned(),
                name: "write".to_owned(),
                arguments: "{}".to_owned(),
            },
            session::EventKind::ToolResult {
                turn: 1,
                step: 1,
                message: model::Message::tool_result("call-write", "ok", false),
                error: None,
                meta: Some(json!({ "diffs": [{ "path": path }] })),
            },
        ]
    }

    #[test]
    fn report_signature_uses_only_its_own_attempt_changes() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path();
        let store = tasks::Store::open(data.join("tasks.sqlite")).unwrap();
        let now = 1_794_936_000_000_i64;
        let task = store
            .create_task(
                tasks::NewTask {
                    project_id: None,
                    cwd_source: None,
                    title: "循环签名".to_owned(),
                    requirement: "每轮改不同文件".to_owned(),
                    dod: "dod".to_owned(),
                    cwd: data.to_path_buf(),
                    conversation: tasks::ConversationId::new("kind-cycle-signature"),
                    model: tasks::ModelChoice {
                        connection: "deepseek".to_owned(),
                        model: "deepseek-flash".to_owned(),
                        effort_wanted: "high".to_owned(),
                    },
                    tools: Vec::new(),
                },
                now,
            )
            .unwrap();
        for (index, file) in ["a.rs", "b.rs", "c.rs"].into_iter().enumerate() {
            let mut session = session::SessionStore::create(data, data).unwrap();
            let attempt = store.begin_attempt(&task.id, now + index as i64).unwrap();
            store
                .attach_session(
                    &task.id,
                    attempt,
                    tasks::ConversationId::new(session.id().as_str()),
                )
                .unwrap();
            session.append(&write_events(&data.join(file))).unwrap();
            store
                .record_report(
                    &task.id,
                    attempt,
                    tasks::NewReport {
                        state: tasks::ReportState::Ready,
                        origin: tasks::ReportOrigin::Agent,
                        summary: format!("第 {index} 轮"),
                        question: None,
                    },
                    now + index as i64,
                )
                .unwrap();
            store
                .end_attempt(
                    &task.id,
                    attempt,
                    now + index as i64,
                    tasks::AttemptEnd::Reported,
                )
                .unwrap();
        }
        let task = store.task(&task.id).unwrap().unwrap();
        assert_eq!(task.reports.len(), 3);
        let signatures: Vec<String> = task
            .reports
            .iter()
            .map(|report| report_signature(data, &task, report))
            .collect();
        assert_eq!(
            signatures
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3,
            "三份报告改的文件不同，签名必须不同：{signatures:?}"
        );
        assert!(
            !super::cycle_signatures_stall(&signatures),
            "签名不同就不该按连续三同签名暂停"
        );
    }
}
