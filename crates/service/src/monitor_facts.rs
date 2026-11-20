//! 监控消耗/诊断的服务端现算事实。只读任务库和会话日志，不新建采集。

use std::path::Path;

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use credentials::ConnectionInfo;
use model::Usage;
use session::EventKind;
use tasks::{Stage, Store, Task, TaskId};

use crate::protocol::{
    MonitorAnomaly, MonitorAttempt, MonitorContextParts, MonitorDay, MonitorProject, MonitorRange,
    MonitorRings, MonitorRunning, MonitorSnapshotResponse, MonitorTaskInfo, MonitorTaskRequest,
    MonitorTaskResponse, MonitorTaskSummary, MonitorTodo, MonitorUsageRow,
};
use crate::{ServiceError, stats};

struct Agg {
    connections: Vec<ConnectionInfo>,
    catalog: model::catalog::CatalogSnapshot,
    models: Vec<MonitorUsageRow>,
    tools: Vec<MonitorUsageRow>,
    commands: Vec<MonitorUsageRow>,
    tasks: Vec<MonitorTaskSummary>,
    anomalies: Vec<MonitorAnomaly>,
    days: BTreeMap<String, MonitorDay>,
}

/// 监控消耗：按用户时区口径现算全部任务；Kind 对话不在任务库里，本票先不混入。
pub(crate) fn snapshot(
    store: &Store,
    data_dir: &Path,
    range: MonitorRange,
    timezone: &str,
    running: &[String],
    seats: u64,
    kind_events: &[session::Event],
) -> Result<MonitorSnapshotResponse, ServiceError> {
    let now = Utc::now();
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let start = range_start(range, timezone, now.timestamp_millis());
    let keyring = credentials::Keyring::open_read_only(data_dir.to_path_buf(), None).ok();
    let connections = keyring
        .as_ref()
        .map(|keyring| keyring.view().connections)
        .unwrap_or_default();
    let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
    let tasks = store
        .all_tasks()
        .map_err(|error| ServiceError::TaskUnavailable(format!("任务读不出来：{error}")))?;
    let mut agg = Agg {
        connections,
        catalog,
        models: Vec::new(),
        tools: Vec::new(),
        commands: Vec::new(),
        tasks: Vec::new(),
        anomalies: Vec::new(),
        days: BTreeMap::new(),
    };
    let mut rings = MonitorRings::default();
    for task in tasks {
        let Some(summary) = agg.add_task(
            store,
            data_dir,
            &task,
            start,
            tz,
            running
                .contains(&task.id.as_str().to_owned())
                .then_some(true),
            &mut rings,
        )?
        else {
            continue;
        };
        agg.tasks.push(summary);
    }
    if !kind_events.is_empty() {
        let filtered: Vec<session::Event> = kind_events
            .iter()
            .filter(|event| event.time >= start as u64)
            .cloned()
            .collect();
        if !filtered.is_empty() {
            let selection = session::last_model_selection(kind_events).unwrap_or_else(|| {
                session::ModelSelection {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                }
            });
            let stats =
                stats::session_stats(&filtered, &agg.connections, &agg.catalog, &selection, 0, 0);
            match (rings.cost_usd, stats.cost_usd) {
                (Some(left), Some(right)) => rings.cost_usd = Some(left + right),
                (None, Some(right)) => rings.cost_usd = Some(right),
                _ => {}
            }
            rings.unpriced |= stats.unpriced;
            agg.add_events(&filtered, None, &selection, tz, &mut rings);
        }
    }
    let mut running_tasks = Vec::new();
    let mut queue = Vec::new();
    for task in &agg.tasks {
        if task.running {
            running_tasks.push(task.clone());
        }
        if task.state == "pending" {
            queue.push(task.clone());
        }
    }
    // 做梦只进消耗合计/环：自己的会话不在任务库里，按 dream_run 的 usage_json 补。
    if let Ok(memory) = memory::MemoryStore::open(data_dir.join("memory.sqlite"))
        && let Ok(runs) = memory.dream_runs(10_000)
    {
        for run in runs {
            if run.started_at.saturating_mul(1000) < start {
                continue;
            }
            let connection = agg
                .connections
                .iter()
                .find(|connection| {
                    connection
                        .models
                        .as_ref()
                        .is_some_and(|rows| rows.iter().any(|row| row.id == run.model))
                })
                .or_else(|| agg.connections.first());
            let cost =
                stats::dream_usage_cost(&run.usage_json, connection, &agg.catalog, &run.model);
            if let Ok(usage) = serde_json::from_str::<Usage>(&run.usage_json) {
                rings.tokens = rings
                    .tokens
                    .saturating_add(usage.input_tokens.unwrap_or(0))
                    .saturating_add(usage.output_tokens.unwrap_or(0));
            }
            match (rings.cost_usd, cost) {
                (Some(left), Some(right)) => rings.cost_usd = Some(left + right),
                (None, Some(right)) => rings.cost_usd = Some(right),
                _ => {}
            }
            rings.unpriced |= cost.is_none();
        }
    }
    let models = std::mem::take(&mut agg.models);
    let tools = std::mem::take(&mut agg.tools);
    let commands = std::mem::take(&mut agg.commands);
    let mut anomalies = agg.anomalies;
    anomalies.sort_by(|a, b| a.kind.cmp(&b.kind));
    let anomalous = anomalies.len() as u64;
    Ok(MonitorSnapshotResponse {
        range,
        timezone: timezone.to_owned(),
        generated_at: now.timestamp_millis(),
        days: agg.days.into_values().collect(),
        tasks: agg.tasks,
        projects: Vec::<MonitorProject>::new(),
        models,
        tools,
        commands,
        rings,
        anomalies,
        running: MonitorRunning {
            seats,
            running: running_tasks.len() as u64,
            queue: queue.len() as u64,
            anomalous,
            tasks: running_tasks,
        },
        queue,
    })
}

impl Agg {
    #[allow(clippy::too_many_arguments)]
    fn add_task(
        &mut self,
        store: &Store,
        data_dir: &Path,
        task: &Task,
        start: i64,
        tz: Tz,
        running: Option<bool>,
        rings: &mut MonitorRings,
    ) -> Result<Option<MonitorTaskSummary>, ServiceError> {
        let selection = session::ModelSelection {
            connection: task.model.connection.clone(),
            model: task.model.model.clone(),
            effort_wanted: task.model.effort_wanted.clone(),
        };
        let mut summary = MonitorTaskSummary {
            task_id: task.id.as_str().to_owned(),
            title: task.title.clone(),
            state: "pending".to_owned(),
            model: task.model.model.clone(),
            project_id: None,
            attempt_id: task.latest_attempt().map(|attempt| attempt.id.0),
            running: running.unwrap_or(false),
            started_at: task.latest_attempt().map(|attempt| attempt.started_at),
            ended_at: task.latest_attempt().and_then(|attempt| attempt.ended_at),
            requests: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
            unpriced: false,
            tool_calls: 0,
            context_used: 0,
            context_window: 0,
        };
        summary.state = store
            .stage(&task.id)
            .map(stage_name)
            .unwrap_or_else(|_| "pending".to_owned());
        let mut latest_events = Vec::new();
        for attempt in task.runs.iter().flat_map(|run| &run.attempts) {
            let Some(conversation) = &attempt.conversation else {
                continue;
            };
            let session_id = session::SessionId::parse(conversation.as_str()).map_err(|error| {
                ServiceError::TaskUnavailable(format!("会话 id 不合法：{error}"))
            })?;
            let Ok(store) = session::SessionStore::open_read_only(data_dir, &session_id) else {
                continue;
            };
            let all = store.events().to_vec();
            let filtered: Vec<session::Event> = all
                .iter()
                .filter(|event| event.time >= start as u64)
                .cloned()
                .collect();
            if !all.is_empty() {
                latest_events = all;
            }
            let stats = stats::session_stats(
                &filtered,
                &self.connections,
                &self.catalog,
                &selection,
                0,
                0,
            );
            summary.requests += stats.turns.max(assistant_count(&filtered));
            summary.input_tokens += stats.input_tokens;
            summary.output_tokens += stats.output_tokens;
            match (summary.cost_usd, stats.cost_usd) {
                (Some(left), Some(right)) => summary.cost_usd = Some(left + right),
                (None, Some(right)) => summary.cost_usd = Some(right),
                _ => {}
            }
            summary.unpriced |= stats.unpriced;
            match (rings.cost_usd, stats.cost_usd) {
                (Some(left), Some(right)) => rings.cost_usd = Some(left + right),
                (None, Some(right)) => rings.cost_usd = Some(right),
                _ => {}
            }
            rings.unpriced |= stats.unpriced;
            self.add_events(&filtered, Some(task.id.as_str()), &selection, tz, rings);
        }
        summary.tool_calls = latest_events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::ToolCall { .. }))
            .count() as u64;
        summary.context_used = context_used(&latest_events);
        summary.context_window =
            crate::session_select::model_context_window(data_dir, None, &selection).unwrap_or(0);
        if let Some(usage) = latest_usage(&latest_events) {
            summary.input_tokens = summary
                .input_tokens
                .saturating_add(usage.input_tokens.unwrap_or(0));
        }
        if summary.requests == 0 && summary.input_tokens == 0 {
            return Ok(None);
        }
        Ok(Some(summary))
    }

    fn add_events(
        &mut self,
        events: &[session::Event],
        task_id: Option<&str>,
        selection: &session::ModelSelection,
        tz: Tz,
        rings: &mut MonitorRings,
    ) {
        let mut step_at = 0_u64;
        for event in events {
            match &event.kind {
                EventKind::StepStart { .. } => step_at = event.time,
                EventKind::AssistantMessage {
                    message,
                    usage: Some(usage),
                    ..
                } => {
                    let model_name = match &message.source {
                        model::MessageSource::Model { model, .. } => model.clone(),
                        _ => selection.model.clone(),
                    };
                    let cost = self.price(&model_name, usage);
                    self.add_usage(&model_name, usage, cost);
                    self.add_day_usage(tz, event.time, usage, cost);
                    rings.tokens = rings
                        .tokens
                        .saturating_add(usage.input_tokens.unwrap_or(0))
                        .saturating_add(usage.output_tokens.unwrap_or(0));
                    if step_at > 0 {
                        rings.request_ms = rings
                            .request_ms
                            .saturating_add(event.time.saturating_sub(step_at));
                        rings.busy_ms = rings
                            .busy_ms
                            .saturating_add(event.time.saturating_sub(step_at));
                    }
                }
                EventKind::ToolCall { name, .. } => {
                    add_count(&mut self.tools, name);
                    self.add_day_count(tz, event.time, true);
                }
                _ => {}
            }
        }
        for fact in crate::commands::command_facts(events) {
            add_count(&mut self.commands, &fact.program);
            self.add_day_count(tz, fact.time, false);
        }
        for repeat in crate::commands::repeated_commands(events) {
            self.anomalies.push(MonitorAnomaly {
                kind: "same-command".to_owned(),
                task_id: task_id.map(str::to_owned),
                detail: format!("连跑 {} 次，结果一样：{}", repeat.count, repeat.command),
            });
        }
        rings.unpriced |= self.models.iter().any(|row| row.unpriced);
    }

    fn price(&self, model: &str, usage: &Usage) -> Option<f64> {
        let connection = self
            .connections
            .iter()
            .find(|connection| {
                connection
                    .models
                    .as_ref()
                    .is_some_and(|rows| rows.iter().any(|row| row.id == model))
            })
            .or_else(|| self.connections.first())?;
        let price = stats::effective_price(connection, &self.catalog, model)?;
        stats::usage_cost_usd(usage, &price)
    }

    fn add_day_usage(&mut self, tz: Tz, time: u64, usage: &Usage, cost: Option<f64>) {
        let day = self.day_entry(tz, time);
        day.requests += 1;
        day.input_tokens += usage.input_tokens.unwrap_or(0);
        day.output_tokens += usage.output_tokens.unwrap_or(0);
        match (day.cost_usd, cost) {
            (Some(left), Some(right)) => day.cost_usd = Some(left + right),
            (None, Some(right)) => day.cost_usd = Some(right),
            _ => {}
        }
        if cost.is_none() && has_usage(usage) {
            day.unpriced += 1;
        }
    }

    fn add_day_count(&mut self, tz: Tz, time: u64, tool: bool) {
        let day = self.day_entry(tz, time);
        if tool {
            day.tool_calls += 1;
        } else {
            day.command_calls += 1;
        }
    }

    fn day_entry(&mut self, tz: Tz, time: u64) -> &mut MonitorDay {
        let key = day_key(tz, time);
        self.days.entry(key.clone()).or_insert(MonitorDay {
            day: key,
            cost_usd: None,
            unpriced: 0,
            input_tokens: 0,
            output_tokens: 0,
            requests: 0,
            tool_calls: 0,
            command_calls: 0,
        })
    }

    fn add_usage(&mut self, key: &str, usage: &Usage, cost: Option<f64>) {
        if let Some(row) = self.models.iter_mut().find(|row| row.key == key) {
            row.requests += 1;
            row.input_tokens += usage.input_tokens.unwrap_or(0);
            row.output_tokens += usage.output_tokens.unwrap_or(0);
            row.cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
            row.cache_write_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
            match (row.cost_usd, cost) {
                (Some(left), Some(right)) => row.cost_usd = Some(left + right),
                (None, Some(right)) => row.cost_usd = Some(right),
                _ => {}
            }
            row.unpriced |= cost.is_none() && has_usage(usage);
            return;
        }
        self.models.push(MonitorUsageRow {
            key: key.to_owned(),
            label: key.to_owned(),
            requests: 1,
            input_tokens: usage.input_tokens.unwrap_or(0),
            output_tokens: usage.output_tokens.unwrap_or(0),
            cache_read_tokens: usage.cache_read_input_tokens.unwrap_or(0),
            cache_write_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
            cost_usd: cost,
            unpriced: cost.is_none() && has_usage(usage),
        });
    }
}

fn add_count(rows: &mut Vec<MonitorUsageRow>, key: &str) {
    if let Some(row) = rows.iter_mut().find(|row| row.key == key) {
        row.requests += 1;
        return;
    }
    rows.push(MonitorUsageRow {
        key: key.to_owned(),
        label: key.to_owned(),
        requests: 1,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: None,
        unpriced: false,
    });
}

/// 监控诊断：一个任务/Attempt 的 attempts、五块、todo、事件；Kind 没有任务，查不到就是找错对象。
pub(crate) fn task(
    store: &Store,
    data_dir: &Path,
    request: MonitorTaskRequest,
    running: bool,
) -> Result<MonitorTaskResponse, ServiceError> {
    let task = store
        .task(&TaskId::from(request.task_id.as_str()))
        .map_err(|error| ServiceError::TaskUnavailable(format!("任务读不出来：{error}")))?
        .ok_or_else(|| ServiceError::TaskUnavailable("Kind 不诊断，只能看任务".to_owned()))?;
    let selection = session::ModelSelection {
        connection: task.model.connection.clone(),
        model: task.model.model.clone(),
        effort_wanted: task.model.effort_wanted.clone(),
    };
    let mut attempts = Vec::new();
    let mut wanted_events = Vec::new();
    for attempt in task.runs.iter().flat_map(|run| &run.attempts) {
        let mut events = Vec::new();
        if let Some(conversation) = &attempt.conversation
            && let Ok(session_id) = session::SessionId::parse(conversation.as_str())
            && let Ok(session) = session::SessionStore::open_read_only(data_dir, &session_id)
        {
            events = session.events().to_vec();
        }
        let requests = assistant_count(&events);
        attempts.push(MonitorAttempt {
            attempt_id: attempt.id.0,
            started_at: attempt.started_at,
            ended_at: attempt.ended_at,
            session_id: attempt
                .conversation
                .as_ref()
                .map(|conversation| conversation.as_str().to_owned()),
            requests,
            input_tokens: sum_usage(&events, |usage| usage.input_tokens),
            output_tokens: sum_usage(&events, |usage| usage.output_tokens),
            cost_usd: None,
            unpriced: false,
        });
        if request.attempt_id.is_none() || request.attempt_id == Some(attempt.id.0) {
            wanted_events = events;
        }
    }
    let context_parts = wanted_events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            EventKind::AssistantMessage {
                context_parts: Some(parts),
                ..
            }
            | EventKind::AssistantAttempt {
                context_parts: Some(parts),
                ..
            } => Some(MonitorContextParts {
                fixed_block: parts.fixed_block,
                tool_definitions: parts.tool_definitions,
                task_book_messages: parts.task_book_messages,
                model_replies: parts.model_replies,
                tool_results: parts.tool_results,
            }),
            _ => None,
        });
    let todo = session::last_todos(&wanted_events)
        .unwrap_or_default()
        .into_iter()
        .map(|item| MonitorTodo {
            content: item.content,
            status: item.status.as_str().to_owned(),
        })
        .collect();
    let events = wanted_events
        .iter()
        .map(crate::events::service_event)
        .collect();
    Ok(MonitorTaskResponse {
        task: MonitorTaskInfo {
            task_id: task.id.as_str().to_owned(),
            title: task.title.clone(),
            state: store
                .stage(&task.id)
                .map(stage_name)
                .unwrap_or_else(|_| "pending".to_owned()),
            model: selection.model,
            project_id: None,
            running,
            write_scope: task
                .write_scope
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        },
        attempts,
        context_parts,
        todo,
        events,
    })
}

fn assistant_count(events: &[session::Event]) -> u64 {
    events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::AssistantMessage { .. }))
        .count() as u64
}

fn context_used(events: &[session::Event]) -> u64 {
    for event in events.iter().rev() {
        if let EventKind::AssistantMessage {
            usage: Some(usage), ..
        } = &event.kind
        {
            return usage.input_tokens.unwrap_or(0)
                + usage.cache_read_input_tokens.unwrap_or(0)
                + usage.cache_creation_input_tokens.unwrap_or(0);
        }
    }
    0
}

fn latest_usage(events: &[session::Event]) -> Option<&Usage> {
    events.iter().rev().find_map(|event| match &event.kind {
        EventKind::AssistantMessage { usage, .. } => usage.as_ref(),
        _ => None,
    })
}

fn sum_usage(events: &[session::Event], pick: impl Fn(&Usage) -> Option<u64>) -> u64 {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::AssistantMessage {
                usage: Some(usage), ..
            } => pick(usage),
            _ => None,
        })
        .sum()
}

fn has_usage(usage: &Usage) -> bool {
    usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0) > 0
}

fn stage_name(stage: Stage) -> String {
    match stage {
        Stage::NeedsConditions => "needs_conditions",
        Stage::Pending => "pending",
        Stage::Running => "running",
        Stage::ReadyForReview => "ready_for_review",
        Stage::RequirementMet => "requirement_met",
        Stage::Closed => "closed",
    }
    .to_owned()
}

fn day_key(tz: Tz, time_ms: u64) -> String {
    Utc.timestamp_millis_opt(time_ms as i64)
        .single()
        .unwrap_or_default()
        .with_timezone(&tz)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

fn range_start(range: MonitorRange, timezone: &str, now_ms: i64) -> i64 {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let now = Utc
        .timestamp_millis_opt(now_ms)
        .single()
        .unwrap_or_default();
    let local = now.with_timezone(&tz);
    let today = local.date_naive();
    let date = match range {
        MonitorRange::Today => today,
        MonitorRange::Last7Days => today - Duration::days(6),
        MonitorRange::Month => {
            chrono::NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today)
        }
    };
    date.and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_local_timezone(tz)
        .single()
        .map(|time| time.timestamp_millis())
        .unwrap_or_else(|| {
            date.and_hms_opt(0, 0, 0)
                .unwrap_or_default()
                .and_utc()
                .timestamp_millis()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, tempfile::TempDir, Store) {
        let data = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::open(data.path().join("tasks.sqlite")).unwrap();
        (data, cwd, store)
    }

    #[test]
    fn range_start_uses_the_user_timezone() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-21T03:00:00Z")
            .unwrap()
            .timestamp_millis();
        let today = range_start(MonitorRange::Today, "Asia/Shanghai", now);
        assert_eq!(
            chrono::DateTime::from_timestamp_millis(today)
                .unwrap()
                .to_rfc3339(),
            "2026-11-20T16:00:00+00:00"
        );
        let month = range_start(MonitorRange::Month, "Asia/Shanghai", now);
        assert_eq!(
            chrono::DateTime::from_timestamp_millis(month)
                .unwrap()
                .to_rfc3339(),
            "2026-10-31T16:00:00+00:00"
        );
        assert_eq!(
            chrono::DateTime::from_timestamp_millis(range_start(
                MonitorRange::Last7Days,
                "Asia/Shanghai",
                now
            ))
            .unwrap()
            .to_rfc3339(),
            "2026-11-14T16:00:00+00:00"
        );
    }

    #[test]
    fn empty_snapshot_has_all_sections() {
        let (data, _cwd, store) = temp_store();
        let snapshot =
            snapshot(&store, data.path(), MonitorRange::Today, "UTC", &[], 3, &[]).unwrap();
        assert!(snapshot.tasks.is_empty());
        assert!(snapshot.models.is_empty());
        assert_eq!(snapshot.running.seats, 3);
        assert!(snapshot.rings.cost_usd.is_none());
    }

    #[test]
    fn monitor_task_of_unknown_id_is_not_kind_diagnosis() {
        let (data, _cwd, store) = temp_store();
        let error = task(
            &store,
            data.path(),
            MonitorTaskRequest {
                task_id: "kind".to_owned(),
                attempt_id: None,
            },
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Kind 不诊断"), "{error}");
    }

    #[test]
    fn two_hundred_tasks_with_twenty_requests_cold_snapshot_is_measured() {
        let (data, cwd, store) = temp_store();
        let now = 1_700_000_000_000_i64;
        for index in 0..200 {
            let task = store
                .create_task(
                    tasks::NewTask {
                        title: format!("任务 {index}"),
                        requirement: "把事做完".to_owned(),
                        dod: "做完".to_owned(),
                        cwd: cwd.path().to_path_buf(),
                        conversation: tasks::ConversationId::new("kind"),
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
            let attempt = store.begin_attempt(&task.id, now).unwrap();
            let mut session = session::SessionStore::create(data.path(), cwd.path()).unwrap();
            for request in 0..20 {
                let turn = request as u64 + 1;
                let message = model::Message::assistant(
                    vec![model::ContentBlock::text("ok")],
                    "deepseek",
                    "deepseek-flash",
                );
                session
                    .append(&[
                        session::EventKind::TurnStart { turn },
                        session::EventKind::StepStart { turn, step: 1 },
                        session::EventKind::AssistantMessage {
                            turn,
                            step: 1,
                            message,
                            stream: Vec::new(),
                            usage: Some(model::Usage {
                                input_tokens: Some(1_000),
                                output_tokens: Some(100),
                                cache_read_input_tokens: Some(0),
                                cache_creation_input_tokens: Some(0),
                            }),
                            interrupted: None,
                            effort: Some("high".to_owned()),
                            effort_wanted: Some("high".to_owned()),
                            request_at: None,
                            context_parts: None,
                        },
                        session::EventKind::TurnEnd {
                            turn,
                            reason: session::TurnEndReason::Completed,
                        },
                    ])
                    .unwrap();
            }
            store
                .attach_session(
                    &task.id,
                    attempt,
                    tasks::ConversationId::new(session.id().to_string()),
                )
                .unwrap();
        }
        let started = std::time::Instant::now();
        let snapshot =
            snapshot(&store, data.path(), MonitorRange::Month, "UTC", &[], 3, &[]).unwrap();
        eprintln!("200 tasks cold snapshot: {:?}", started.elapsed());
        assert_eq!(snapshot.tasks.len(), 200);
        assert!(snapshot.rings.tokens >= 200 * 20 * 1_100);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}

#[cfg(test)]
mod bucket_tests {
    use super::*;
    use model::Message;

    fn call(call_id: &str, name: &str, arguments: serde_json::Value) -> session::Event {
        session::Event::new(
            1,
            10,
            EventKind::ToolCall {
                turn: 1,
                step: 1,
                call_id: call_id.to_owned(),
                name: name.to_owned(),
                arguments: arguments.to_string(),
            },
        )
    }

    fn result(call_id: &str, text: &str) -> session::Event {
        session::Event::new(
            2,
            20,
            EventKind::ToolResult {
                turn: 1,
                step: 1,
                message: Message::tool_result(call_id, text, false),
                error: None,
                meta: None,
            },
        )
    }

    #[test]
    fn ls_and_calendar_do_not_cross_tool_and_command_buckets() {
        let mut agg = Agg {
            connections: Vec::new(),
            catalog: model::catalog::snapshot(),
            models: Vec::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            tasks: Vec::new(),
            anomalies: Vec::new(),
            days: BTreeMap::new(),
        };
        let events = vec![
            call("t1", "ls", serde_json::json!({})),
            result("t1", "a.txt"),
            call("t2", "calendar", serde_json::json!({})),
            result("t2", "周一"),
            call("b1", "bash", serde_json::json!({"command": "ls -la"})),
            result("b1", "a.txt"),
            call(
                "b2",
                "bash",
                serde_json::json!({"command": "calendar --today"}),
            ),
            result("b2", "周一"),
        ];
        let selection = session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        };
        agg.add_events(
            &events,
            Some("task-1"),
            &selection,
            chrono_tz::UTC,
            &mut MonitorRings::default(),
        );
        let tool_keys: Vec<&str> = agg.tools.iter().map(|row| row.key.as_str()).collect();
        let command_keys: Vec<&str> = agg.commands.iter().map(|row| row.key.as_str()).collect();
        assert!(
            tool_keys.contains(&"ls"),
            "ls 工具应进 tools：{tool_keys:?}"
        );
        assert!(
            tool_keys.contains(&"calendar"),
            "calendar 工具应进 tools：{tool_keys:?}"
        );
        assert!(
            command_keys.contains(&"ls"),
            "ls 命令应进 commands：{command_keys:?}"
        );
        assert!(
            command_keys.contains(&"calendar"),
            "calendar 命令应进 commands：{command_keys:?}"
        );
        assert!(agg.models.is_empty(), "没有模型用量就不该有 models 行");
        assert_eq!(
            agg.tools
                .iter()
                .find(|row| row.key == "ls")
                .unwrap()
                .requests,
            1
        );
        assert_eq!(
            agg.tools
                .iter()
                .find(|row| row.key == "calendar")
                .unwrap()
                .requests,
            1
        );
        assert_eq!(
            agg.commands
                .iter()
                .find(|row| row.key == "ls")
                .unwrap()
                .requests,
            1
        );
        assert_eq!(
            agg.commands
                .iter()
                .find(|row| row.key == "calendar")
                .unwrap()
                .requests,
            1
        );
    }
}

#[cfg(test)]
mod daily_tests {
    use super::*;
    use chrono::TimeZone;
    use model::Message;

    fn assistant(time: u64, tokens: u64) -> session::Event {
        session::Event::new(
            1,
            time,
            EventKind::AssistantMessage {
                turn: 1,
                step: 1,
                message: Message::assistant(
                    vec![model::ContentBlock::text("ok")],
                    "deepseek",
                    "deepseek-flash",
                ),
                stream: Vec::new(),
                usage: Some(Usage {
                    input_tokens: Some(tokens),
                    output_tokens: Some(tokens / 2),
                    cache_read_input_tokens: Some(0),
                    cache_creation_input_tokens: Some(0),
                }),
                interrupted: None,
                effort: None,
                effort_wanted: None,
                request_at: None,
                context_parts: None,
            },
        )
    }

    fn tool(time: u64, name: &str) -> session::Event {
        session::Event::new(
            2,
            time,
            EventKind::ToolCall {
                turn: 1,
                step: 1,
                call_id: name.to_owned(),
                name: name.to_owned(),
                arguments: "{}".to_owned(),
            },
        )
    }

    #[test]
    fn two_local_days_get_the_events_not_the_task_ends() {
        let shanghai = "Asia/Shanghai".parse::<Tz>().unwrap();
        let day_one = chrono::FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 11, 19, 23, 30, 0)
            .unwrap()
            .timestamp_millis() as u64;
        let day_two = day_one + 2 * 3600 * 1000;
        let mut agg = Agg {
            connections: Vec::new(),
            catalog: model::catalog::snapshot(),
            models: Vec::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            tasks: Vec::new(),
            anomalies: Vec::new(),
            days: BTreeMap::new(),
        };
        let selection = session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        };
        agg.add_events(
            &[assistant(day_one, 1_000), assistant(day_two, 1_000)],
            Some("task-1"),
            &selection,
            shanghai,
            &mut MonitorRings::default(),
        );
        let days: Vec<&MonitorDay> = agg.days.values().collect();
        assert_eq!(days.len(), 2, "{days:?}");
        assert_eq!(days[0].day, "2026-11-19");
        assert_eq!(days[1].day, "2026-11-20");
        assert_eq!(days[0].requests, 1);
        assert_eq!(days[1].requests, 1);
        assert_eq!(days[0].input_tokens, 1_000);
        assert_eq!(days[1].input_tokens, 1_000);
        assert_eq!(days[0].output_tokens, 500);
        assert_eq!(days[1].output_tokens, 500);
    }

    #[test]
    fn utc_midnight_boundary_lands_on_the_user_timezone_day() {
        let shanghai = "Asia/Shanghai".parse::<Tz>().unwrap();
        let timestamp = Utc
            .with_ymd_and_hms(2026, 11, 19, 16, 30, 0)
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(day_key(shanghai, timestamp), "2026-11-20");
        assert_eq!(day_key(chrono_tz::UTC, timestamp), "2026-11-19");
    }

    #[test]
    fn tool_events_fall_on_their_own_day() {
        let shanghai = "Asia/Shanghai".parse::<Tz>().unwrap();
        let day_one = chrono::FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 11, 19, 12, 0, 0)
            .unwrap()
            .timestamp_millis() as u64;
        let day_two = day_one + 24 * 3600 * 1000;
        let mut agg = Agg {
            connections: Vec::new(),
            catalog: model::catalog::snapshot(),
            models: Vec::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            tasks: Vec::new(),
            anomalies: Vec::new(),
            days: BTreeMap::new(),
        };
        let events = vec![tool(day_one, "ls"), tool(day_two, "calendar")];
        let selection = session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        };
        agg.add_events(
            &events,
            Some("task-1"),
            &selection,
            shanghai,
            &mut MonitorRings::default(),
        );
        assert_eq!(agg.days["2026-11-19"].tool_calls, 1);
        assert_eq!(agg.days["2026-11-20"].tool_calls, 1);
    }
}
