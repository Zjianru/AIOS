//! 循环约定——报回后送「第 N 轮」、程序事实暂停、工作快照等你项、继续准入。

use super::*;
use crate::work::WaitItem;
use calendar::{AppointmentStatus, NewRun, RunResult, TriggerKind};

const CYCLE_PAUSE_PREFIX: &str = "循环约定暂停：";

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
        let appointments = store
            .list_appointments()
            .map_err(|error| error.to_string())?;
        let mut waits = Vec::new();
        for appointment in appointments {
            if appointment.status != AppointmentStatus::Paused {
                continue;
            }
            let Some(cycle) = &appointment.cycle else {
                continue;
            };
            let Some(reason) = store
                .latest_version_reason(&appointment.id)
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            let Some(reason) = reason.strip_prefix(CYCLE_PAUSE_PREFIX) else {
                continue;
            };
            waits.push(WaitItem {
                kind: "answer".to_owned(),
                task_id: Some(cycle.task_id.clone()),
                title: appointment.title.clone(),
                stage: None,
                need: format!("循环约定暂停了：{reason}"),
                body: reason.to_owned(),
                at: appointment.updated_at,
                turn: None,
                also: Vec::new(),
                pick: None,
                write: None,
                version: Some(appointment.version),
                appointment_id: Some(appointment.id.clone()),
            });
        }
        Ok(waits)
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
        signatures.windows(2).all(|pair| pair[0] == pair[1])
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
