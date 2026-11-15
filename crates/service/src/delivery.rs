//! 交付记录：从任务已有事实里派生「交了什么、依据什么、还缺什么、交到了哪、接受了吗」。
//!
//! 纯派生，不落库、不加表；页面和 Kind 工具读同一份。锚点是最后一份
//! state=Ready 且 origin=Agent 的报告；之后的 Blocked/Failed/Package 只进「还缺什么」。

use std::path::Path;

use serde::{Deserialize, Serialize};
use tasks::{
    Acceptance, GateCheck, GateCheckOutcome, GateConclusion, Report, ReportId, ReportOrigin,
    ReportState, Task, VersionSource,
};
use ts_rs::TS;

use crate::protocol::TaskReadGateCheck;
use crate::result_cards;

/// 一条有颜色的事实行；颜色由程序定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DeliveryLine {
    pub text: String,
    /// done / wait / run / plain / muted
    pub tone: String,
}

/// 交了什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryWhat {
    pub summary: String,
    pub at: i64,
    pub attempt: u64,
    pub version: u32,
    /// None = 只读调查任务；Some(空数组) = 写任务但没记录到改动。
    pub changed: Option<Vec<String>>,
    pub bash: bool,
}

/// 程序门的逐条检查。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryChecks {
    pub commands: Vec<String>,
    pub checks: Vec<TaskReadGateCheck>,
}

/// 依据什么：要求版本、判定、检查记录分开。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryBasis {
    pub version: DeliveryLine,
    pub verdict: DeliveryLine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub record: DeliveryLine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<DeliveryChecks>,
}

/// 接受了吗：状态行 + 每个要求版本一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryAccepted {
    pub state: DeliveryLine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revisions: Option<Vec<DeliveryLine>>,
}

/// 之前的结果：只列当时的事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryHistoryItem {
    pub attempt: u64,
    pub version: u32,
    pub word: String,
    pub at: i64,
    pub accepted: bool,
}

/// 一份交付记录；页面画的结构化字段和 Kind 读的文本来自同一个派生。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRecord {
    pub attempt: u64,
    pub version: u32,
    pub word: DeliveryLine,
    pub what: DeliveryWhat,
    pub basis: DeliveryBasis,
    pub missing: Vec<DeliveryLine>,
    /// 「交到了哪」。
    #[serde(rename = "where")]
    pub where_text: DeliveryLine,
    pub accepted: DeliveryAccepted,
    pub history: Vec<DeliveryHistoryItem>,
}

impl DeliveryRecord {
    /// Kind 工具读的文本：和结构化字段同一份派生。
    pub fn text(&self) -> String {
        let mut out = vec![
            format!(
                "交付记录：第 {} 次结果 · 第 {} 版要求 · {}",
                self.attempt, self.version, self.word.text
            ),
            "交了什么：".to_owned() + &self.what.summary,
        ];
        out.push(format!(
            "第 {} 次结果 · {}",
            self.what.attempt,
            clock(self.what.at)
        ));
        match &self.what.changed {
            Some(changed) if changed.is_empty() => {
                out.push("没有记录到改过的文件".to_owned());
            }
            Some(changed) => {
                out.push(format!(
                    "改了 {} 个文件：{}",
                    changed.len(),
                    changed.join("、")
                ));
            }
            None => out.push("调查任务：没有改动清单".to_owned()),
        }
        if self.what.bash {
            out.push("开了 bash：清单只含写、改工具做的改动".to_owned());
        }
        out.push("用途：没有记录".to_owned());
        out.push("依据什么：".to_owned() + &self.basis.version.text);
        out.push("判定：".to_owned() + &self.basis.verdict.text);
        if let Some(note) = &self.basis.note {
            out.push(format!("你写的：「{note}」"));
        }
        out.push("检查记录：".to_owned() + &self.basis.record.text);
        if let Some(checks) = &self.basis.checks {
            for check in &checks.checks {
                out.push(format!(
                    "  {}. {} · {} · {}",
                    check.index + 1,
                    check.command,
                    check_exit_text(check),
                    check.duration_ms
                ));
            }
        }
        out.push("还缺什么：".to_owned());
        for line in &self.missing {
            out.push("  - ".to_owned() + &line.text);
        }
        out.push("交到了哪：".to_owned() + &self.where_text.text);
        out.push("接受了吗：".to_owned() + &self.accepted.state.text);
        if let Some(revisions) = &self.accepted.revisions {
            for line in revisions {
                out.push("  ".to_owned() + &line.text);
            }
        }
        if !self.history.is_empty() {
            out.push(format!("之前的结果 {} 份：", self.history.len()));
            for item in &self.history {
                out.push(format!(
                    "  第 {} 次结果 · 第 {} 版要求 · {} · {}",
                    item.attempt,
                    item.version,
                    clock(item.at),
                    item.word
                ));
            }
        }
        out.join("\n")
    }
}

/// 交付记录唯一的派生处；门任务、总任务、没交过 Ready 的任务返回 None。
pub fn delivery_for(data_dir: &Path, task: &Task, gate: Option<&Task>) -> Option<DeliveryRecord> {
    if task.is_gate() || task.is_graph_root() {
        return None;
    }
    let anchor = task.reports.iter().rev().find(|report| {
        report.state == ReportState::Ready && report.origin == ReportOrigin::Agent
    })?;
    let attempt = anchor.attempt_id.0;
    let version = anchor.version.max(1);
    let current = task.requirement_version().max(1);
    let acceptance = task.acceptance_for(&anchor.id);

    let (verdict, note, record, checks, passed, gate_missing) = basis_for(gate, anchor, acceptance);
    let mut missing = gate_missing;
    if let Some(question) = &anchor.question {
        missing.push(line(format!("它在报告里问的：{question}"), "wait"));
    }
    if let Some(later) = later_report(task, anchor) {
        missing.push(line(
            format!(
                "之后第 {} 次没交出结果（{}）：{}",
                later.attempt_id.0,
                later_word(later),
                later.summary
            ),
            "wait",
        ));
    }
    if version < current {
        missing.push(line(
            format!("这份是第 {version} 版要求的结果，要求已改到第 {current} 版：没按当前要求验证"),
            "wait",
        ));
    }
    if missing.is_empty() {
        missing.push(line("没有记录到没验证或判断不了的部分", "muted"));
    }

    let accepted = accepted_for(task, anchor, acceptance, gate, passed, version, current);
    let word = if acceptance.is_some() {
        line("你接受了", "done")
    } else if version < current {
        line("旧版要求的结果", "muted")
    } else if !passed {
        line("检查未证明", "wait")
    } else {
        line("等你接受", "wait")
    };
    let changed = task
        .write_scope
        .as_ref()
        .map(|_| result_cards::changed_files(data_dir, task));
    let history = history_for(task, gate, anchor);
    Some(DeliveryRecord {
        attempt,
        version,
        word,
        what: DeliveryWhat {
            summary: anchor.summary.clone(),
            at: anchor.at,
            attempt,
            version,
            changed,
            bash: task.bash,
        },
        basis: DeliveryBasis {
            version: line(
                version_text(version, current),
                if version < current { "wait" } else { "plain" },
            ),
            verdict: line(
                verdict,
                if acceptance.is_some() || passed {
                    "done"
                } else {
                    "wait"
                },
            ),
            note,
            record: line(record, "plain"),
            checks,
        },
        missing,
        where_text: line("没有外部接续", "muted"),
        accepted,
        history,
    })
}

fn basis_for(
    gate: Option<&Task>,
    anchor: &Report,
    acceptance: Option<&Acceptance>,
) -> (
    String,
    Option<String>,
    String,
    Option<DeliveryChecks>,
    bool,
    Vec<DeliveryLine>,
) {
    let mut missing = Vec::new();
    if let Some(gate) = gate {
        let verdict = gate
            .verdicts
            .iter()
            .rev()
            .find(|verdict| verdict.candidate.report_id == anchor.id);
        if gate.is_program_gate() {
            if let Some(verdict) = verdict {
                let report = gate
                    .reports
                    .iter()
                    .find(|report| report.id == verdict.gate_report);
                let applies = gate.verdict_applies(&anchor.id, verdict);
                let word = match verdict.conclusion {
                    GateConclusion::Pass => "程序判了通过",
                    GateConclusion::NeedsFix => "程序判了需修正",
                    GateConclusion::Undetermined => "未确定",
                };
                let mut text = format!(
                    "程序门「{}」{} · 第 {} 版命令 · {}",
                    gate.title,
                    word,
                    verdict.commands_version,
                    clock(verdict.at)
                );
                if !applies {
                    text.push_str(&format!(
                        "；命令已改成第 {} 版，这次判定不再算数",
                        gate.commands_version()
                    ));
                }
                if verdict.conclusion == GateConclusion::Undetermined {
                    let reason = report
                        .map(|report| undetermined_reason(&report.summary))
                        .unwrap_or("没跑完");
                    text = format!(
                        "程序门「{}」未确定（{reason}）：不算通过，也不算有错",
                        gate.title
                    );
                    let skipped = report
                        .map(|report| skipped_commands(&report.checks))
                        .unwrap_or_default();
                    missing.push(line(
                        format!(
                            "程序门这次没有结论：{reason}{}",
                            if skipped.is_empty() {
                                String::new()
                            } else {
                                format!("；没跑的：{}", skipped.join("、"))
                            }
                        ),
                        "wait",
                    ));
                }
                let checks = report.map(|report| DeliveryChecks {
                    commands: gate
                        .gate_commands()
                        .iter()
                        .map(|command| command.command.clone())
                        .collect(),
                    checks: report.checks.iter().map(read_check).collect(),
                });
                let record = report.map_or_else(
                    || "没有记录：程序门还没跑完这一次".to_owned(),
                    |report| {
                        format!(
                            "程序记的：第 {} 版命令，{} 条里跑了 {} 条",
                            verdict.commands_version,
                            report.checks.len(),
                            report
                                .checks
                                .iter()
                                .filter(|check| check.outcome != GateCheckOutcome::Skipped)
                                .count()
                        )
                    },
                );
                return (
                    text,
                    None,
                    record,
                    checks,
                    verdict.conclusion == GateConclusion::Pass && applies,
                    missing,
                );
            }
            if gate
                .latest_attempt()
                .is_some_and(|attempt| attempt.ended_at.is_none())
            {
                return (
                    format!("程序门「{}」正在跑，还没有结论", gate.title),
                    None,
                    "没有记录：程序门还没跑完这一次".to_owned(),
                    None,
                    false,
                    missing,
                );
            }
            if gate
                .latest_attempt()
                .is_some_and(|attempt| attempt.end == Some(tasks::AttemptEnd::Interrupted))
            {
                return (
                    format!("程序门「{}」这次被打断，没有结论", gate.title),
                    None,
                    "没有记录：程序门这次没有写出结论".to_owned(),
                    None,
                    false,
                    missing,
                );
            }
            return (
                format!("未证明：程序门「{}」还没检查这一份", gate.title),
                None,
                "没有记录：程序门还没跑过这一份".to_owned(),
                None,
                false,
                missing,
            );
        }

        // 用户判的里程碑门。
        let report = gate_report_for(gate, &anchor.id);
        let record = report.map_or_else(
            || "没有记录：核查还没报回".to_owned(),
            |report| {
                format!(
                    "没有程序记录。核查是门任务的 ThinAgent 做的，它报的：{}",
                    report.summary
                )
            },
        );
        if let Some(verdict) = verdict {
            let word = if verdict.conclusion == GateConclusion::Pass {
                "你判了通过"
            } else {
                "你判了需修正"
            };
            return (
                format!("里程碑「{}」{} · {}", gate.title, word, clock(verdict.at)),
                verdict.note.clone(),
                record,
                None,
                verdict.conclusion == GateConclusion::Pass,
                missing,
            );
        }
        return (
            format!("未证明：里程碑「{}」还没判这一份", gate.title),
            None,
            record,
            None,
            false,
            missing,
        );
    }

    let verdict = match acceptance {
        Some(acceptance) => format!(
            "没安排门任务：你接受了这一份，接受就算这次检查 · {}",
            clock(acceptance.at)
        ),
        None => "未证明：没安排门任务，等你接受（接受就算这次检查）".to_owned(),
    };
    (
        verdict,
        None,
        "没有记录：没有程序跑过检查".to_owned(),
        None,
        true,
        missing,
    )
}

fn accepted_for(
    task: &Task,
    anchor: &Report,
    acceptance: Option<&Acceptance>,
    gate: Option<&Task>,
    passed: bool,
    version: u32,
    current: u32,
) -> DeliveryAccepted {
    let revisions = revisions_for(task);
    if let Some(acceptance) = acceptance {
        return DeliveryAccepted {
            state: line(
                format!(
                    "你接受了 · 第 {} 版要求 · {}",
                    acceptance.version,
                    clock(acceptance.at)
                ),
                "done",
            ),
            revisions,
        };
    }
    let earlier = task
        .acceptances
        .iter()
        .rev()
        .find(|acceptance| acceptance.report_id != anchor.id);
    let prefix = earlier.map_or_else(String::new, |acceptance| {
        let attempt = task
            .reports
            .iter()
            .find(|report| report.id == acceptance.report_id)
            .map_or_else(|| "?".to_owned(), |report| report.attempt_id.0.to_string());
        format!(
            "接受过的是第 {attempt} 次结果（第 {} 版要求，{}）；",
            acceptance.version,
            clock(acceptance.at)
        )
    });
    let text = if version < current {
        format!("{prefix}这一份是旧版要求的，不能再接受；等它按第 {current} 版交新的")
    } else if let Some(gate) = gate
        && !passed
    {
        format!(
            "{prefix}还没接受：{}判通过以后才能接受",
            if gate.is_program_gate() {
                "程序门"
            } else {
                "里程碑"
            }
        )
    } else {
        format!("{prefix}这一份还没接受")
    };
    DeliveryAccepted {
        state: line(
            text,
            if version < current || (gate.is_some() && !passed) {
                "muted"
            } else {
                "wait"
            },
        ),
        revisions,
    }
}

fn revisions_for(task: &Task) -> Option<Vec<DeliveryLine>> {
    let mut lines = Vec::new();
    for (index, version) in task.versions.iter().enumerate() {
        if version.source != VersionSource::UserAgreed || index == 0 {
            continue;
        }
        let previous = &task.versions[index - 1];
        let requirement_changed =
            version.requirement != previous.requirement || version.dod != previous.dod;
        let scope_changed = version.write_scope != previous.write_scope;
        let what = match (requirement_changed, scope_changed) {
            (true, true) => "改了要求和写入范围",
            (true, false) => "改了要求",
            (false, true) => "改了写入范围",
            (false, false) => "新版本",
        };
        lines.push(line(
            format!("第 {} 版 · {what} · {}", index + 1, clock(version.at)),
            "plain",
        ));
    }
    (!lines.is_empty()).then_some(lines)
}

fn history_for(task: &Task, gate: Option<&Task>, anchor: &Report) -> Vec<DeliveryHistoryItem> {
    task.reports
        .iter()
        .rev()
        .filter(|report| {
            report.id != anchor.id
                && report.state == ReportState::Ready
                && report.origin == ReportOrigin::Agent
        })
        .map(|report| {
            let accepted = task.acceptance_for(&report.id).is_some();
            let word = if accepted {
                "你接受过".to_owned()
            } else if let Some(gate) = gate {
                gate.verdicts
                    .iter()
                    .rev()
                    .find(|verdict| verdict.candidate.report_id == report.id)
                    .map_or_else(
                        || "被新的一份替下".to_owned(),
                        |verdict| match verdict.conclusion {
                            GateConclusion::Pass => "判了通过".to_owned(),
                            GateConclusion::NeedsFix => "判了需修正".to_owned(),
                            GateConclusion::Undetermined => "未确定".to_owned(),
                        },
                    )
            } else {
                "被新的一份替下".to_owned()
            };
            DeliveryHistoryItem {
                attempt: report.attempt_id.0,
                version: report.version.max(1),
                word,
                at: report.at,
                accepted,
            }
        })
        .collect()
}

fn later_report<'a>(task: &'a Task, anchor: &Report) -> Option<&'a Report> {
    task.reports.iter().rev().find(|report| {
        report.at > anchor.at && report.id != anchor.id && !later_word(report).is_empty()
    })
}

fn later_word(report: &Report) -> &'static str {
    if report.origin == ReportOrigin::Package {
        return "任务包缺";
    }
    if report.origin == ReportOrigin::Executor {
        return "";
    }
    match report.state {
        ReportState::Blocked => "卡住了",
        ReportState::Failed => "失败",
        ReportState::Waiting => "",
        ReportState::Ready => "",
    }
}

fn version_text(version: u32, current: u32) -> String {
    if version < current {
        format!("第 {version} 版要求（当前是第 {current} 版）")
    } else {
        format!("第 {version} 版要求")
    }
}

fn gate_report_for<'a>(gate: &'a Task, anchor: &ReportId) -> Option<&'a Report> {
    gate.reports.iter().rev().find(|report| {
        gate.attempt(report.attempt_id)
            .and_then(|attempt| attempt.handoffs.first())
            .is_some_and(|handoff| &handoff.report_id == anchor)
    })
}

fn undetermined_reason(summary: &str) -> &str {
    summary
        .strip_prefix("程序门未确定：")
        .unwrap_or(summary)
        .trim()
}

fn skipped_commands(checks: &[GateCheck]) -> Vec<String> {
    checks
        .iter()
        .filter(|check| check.outcome == GateCheckOutcome::Skipped)
        .map(|check| check.command.clone())
        .collect()
}

fn read_check(check: &GateCheck) -> TaskReadGateCheck {
    TaskReadGateCheck {
        index: check.index,
        command: check.command.clone(),
        exit_code: check.exit_code,
        outcome: match check.outcome {
            GateCheckOutcome::Ran => "ran",
            GateCheckOutcome::TimedOut => "timed_out",
            GateCheckOutcome::SpawnFailed => "spawn_failed",
            GateCheckOutcome::Stopped => "stopped",
            GateCheckOutcome::Skipped => "skipped",
        }
        .to_owned(),
        duration_ms: check.duration_ms,
        tail: check.tail.clone(),
        spill: check.spill.clone(),
    }
}

fn check_exit_text(check: &TaskReadGateCheck) -> String {
    match check.exit_code {
        Some(code) => format!("退出码 {code}"),
        None => match check.outcome.as_str() {
            "timed_out" => "超时".to_owned(),
            "spawn_failed" => "起不来".to_owned(),
            "stopped" => "被停".to_owned(),
            "skipped" => "没跑".to_owned(),
            _ => "没有退出码".to_owned(),
        },
    }
}

fn line(text: impl Into<String>, tone: &str) -> DeliveryLine {
    DeliveryLine {
        text: text.into(),
        tone: tone.to_owned(),
    }
}

fn clock(at: i64) -> String {
    let days = at.div_euclid(86_400);
    let secs = at.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let _ = year;
    format!(
        "{month}月{day}日 {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// Howard Hinnant 的 civil_from_days；UTC 显示，避免引入时区依赖。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
