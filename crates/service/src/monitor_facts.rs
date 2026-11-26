//! 监控消耗/诊断的服务端现算事实。只读任务库和会话日志，不新建采集。

use std::path::Path;

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use credentials::ConnectionInfo;
use model::Usage;
use session::EventKind;
use tasks::{Stage, Store, Task, TaskId};

use crate::protocol::{
    MonitorAnomaly, MonitorAttempt, MonitorContextParts, MonitorDay, MonitorDayGroup,
    MonitorProject, MonitorRange, MonitorRings, MonitorRunning, MonitorSnapshotResponse,
    MonitorTaskInfo, MonitorTaskRequest, MonitorTaskResponse, MonitorTaskSummary, MonitorTodo,
    MonitorUsageRow,
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
    project_labels: HashMap<String, String>,
}

/// 花费/排行的归属：项目、个人任务、Kind 对话（后两者 projectId 都空，靠 label 区分）。
#[derive(Debug, Clone)]
enum CostGroup {
    Project(String),
    Personal,
    Kind,
}

impl CostGroup {
    fn project_id(&self) -> Option<&str> {
        match self {
            CostGroup::Project(id) => Some(id),
            CostGroup::Personal | CostGroup::Kind => None,
        }
    }

    fn label(&self, labels: &HashMap<String, String>) -> String {
        match self {
            CostGroup::Project(id) => labels.get(id).cloned().unwrap_or_else(|| id.clone()),
            CostGroup::Personal => "个人".to_owned(),
            CostGroup::Kind => "Kind 对话".to_owned(),
        }
    }
}

/// 监控消耗：按用户时区口径现算全部任务；Kind 对话不在任务库里，本票先不混入。
#[allow(clippy::too_many_arguments)]
pub(crate) fn snapshot(
    store: &Store,
    data_dir: &Path,
    range: MonitorRange,
    timezone: &str,
    project_labels: &HashMap<String, String>,
    running: &[String],
    running_request_at: &HashMap<String, u64>,
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
        project_labels: project_labels.clone(),
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
            running_request_at.get(task.id.as_str()).copied(),
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
            agg.add_events(
                &filtered,
                None,
                &CostGroup::Kind,
                &selection,
                tz,
                &mut rings,
            );
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
    // 「在忙」= 时间环五段之和里能采到的请求+工具+命令（等首字/没采到由前端按事实算）。
    rings.busy_ms = rings
        .request_ms
        .saturating_add(rings.tool_ms)
        .saturating_add(rings.command_ms);
    let models = std::mem::take(&mut agg.models);
    let tools = std::mem::take(&mut agg.tools);
    let commands = std::mem::take(&mut agg.commands);
    fill_days(&mut agg.days, range, tz, now.timestamp_millis());
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
        in_flight_request_at: Option<u64>,
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
            project_id: task.project_id.clone(),
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
        let group = match task.project_id.clone() {
            Some(id) => CostGroup::Project(id),
            None => CostGroup::Personal,
        };
        let mut latest_events = Vec::new();
        let mut task_events = Vec::new();
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
            task_events.extend(filtered.iter().cloned());
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
            summary.requests += assistant_count(&filtered);
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
            self.add_events(
                &filtered,
                Some(task.id.as_str()),
                &group,
                &selection,
                tz,
                rings,
            );
        }
        if let Some(anomaly) = longest_request_anomaly(
            &task_events,
            task.id.as_str(),
            running.unwrap_or(false),
            in_flight_request_at,
            Utc::now().timestamp_millis().max(0) as u64,
        ) {
            self.anomalies.push(anomaly);
        }
        summary.tool_calls = latest_events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::ToolCall { .. }))
            .count() as u64;
        summary.context_used = context_used(&latest_events);
        summary.context_window =
            crate::session_select::model_context_window(data_dir, None, &selection).unwrap_or(0);
        if summary.requests == 0 && summary.input_tokens == 0 {
            return Ok(None);
        }
        Ok(Some(summary))
    }

    fn add_events(
        &mut self,
        events: &[session::Event],
        task_id: Option<&str>,
        group: &CostGroup,
        selection: &session::ModelSelection,
        tz: Tz,
        rings: &mut MonitorRings,
    ) {
        let mut step_at = 0_u64;
        let mut calls: HashMap<String, (u64, bool)> = HashMap::new();
        let project_id = group.project_id().map(str::to_owned);
        // 按会话日志切换连接：session/model 到下一个 TurnStart 才生效，和 session_stats 同口径。
        let mut round_connection = selection.connection.clone();
        let mut pending_connection: Option<String> = None;
        for event in events {
            match &event.kind {
                EventKind::StepStart { .. } => step_at = event.time,
                EventKind::TurnStart { .. } => {
                    if let Some(connection) = pending_connection.take() {
                        round_connection = connection;
                    }
                }
                EventKind::SessionModel { connection, .. } => {
                    pending_connection = Some(connection.clone());
                }
                EventKind::AssistantMessage {
                    message,
                    usage: Some(usage),
                    ..
                } => {
                    let model_name = match &message.source {
                        model::MessageSource::Model { model, .. } => model.clone(),
                        _ => selection.model.clone(),
                    };
                    let cost = self.price(&round_connection, &model_name, usage);
                    self.add_usage(&model_name, usage, cost, group);
                    self.add_day_usage(tz, event.time, usage, cost, group);
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
                EventKind::ToolCall { call_id, name, .. } => {
                    calls.insert(call_id.clone(), (event.time, name == "bash"));
                    add_count(&mut self.tools, name, project_id.clone());
                    self.add_day_count(tz, event.time, true, group);
                }
                EventKind::ToolResult { message, .. } => {
                    if let Some(call_id) = message.tool_call_id.as_deref()
                        && let Some((started, bash)) = calls.remove(call_id)
                    {
                        let delta = event.time.saturating_sub(started);
                        if bash {
                            rings.command_ms = rings.command_ms.saturating_add(delta);
                        } else {
                            rings.tool_ms = rings.tool_ms.saturating_add(delta);
                        }
                    }
                }
                _ => {}
            }
        }
        for fact in crate::commands::command_facts(events) {
            add_count(&mut self.commands, &fact.program, project_id.clone());
            self.add_day_count(tz, fact.time, false, group);
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

    fn price(&self, connection_id: &str, model: &str, usage: &Usage) -> Option<f64> {
        let connection = self
            .connections
            .iter()
            .find(|connection| connection.id == connection_id)?;
        let price = stats::effective_price(connection, &self.catalog, model)?;
        stats::usage_cost_usd(usage, &price)
    }

    fn add_day_usage(
        &mut self,
        tz: Tz,
        time: u64,
        usage: &Usage,
        cost: Option<f64>,
        group: &CostGroup,
    ) {
        let label = group.label(&self.project_labels);
        let project = group.project_id().map(str::to_owned);
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
        let group_entry = day
            .groups
            .iter_mut()
            .find(|item| item.project_id.as_deref() == project.as_deref() && item.label == label);
        if let Some(item) = group_entry {
            item.unpriced += u64::from(cost.is_none() && has_usage(usage));
            match (item.cost_usd, cost) {
                (Some(left), Some(right)) => item.cost_usd = Some(left + right),
                (None, Some(right)) => item.cost_usd = Some(right),
                _ => {}
            }
            return;
        }
        day.groups.push(MonitorDayGroup {
            project_id: project,
            label,
            cost_usd: cost,
            unpriced: u64::from(cost.is_none() && has_usage(usage)),
        });
    }

    fn add_day_count(&mut self, tz: Tz, time: u64, tool: bool, _group: &CostGroup) {
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
            groups: Vec::new(),
        })
    }

    fn add_usage(&mut self, key: &str, usage: &Usage, cost: Option<f64>, group: &CostGroup) {
        let project_id = group.project_id().map(str::to_owned);
        if let Some(row) = self
            .models
            .iter_mut()
            .find(|row| row.key == key && row.project_id == project_id)
        {
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
            project_id,
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

fn add_count(rows: &mut Vec<MonitorUsageRow>, key: &str, project_id: Option<String>) {
    if let Some(row) = rows
        .iter_mut()
        .find(|row| row.key == key && row.project_id == project_id)
    {
        row.requests += 1;
        return;
    }
    rows.push(MonitorUsageRow {
        key: key.to_owned(),
        label: key.to_owned(),
        project_id,
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
    let keyring = credentials::Keyring::open_read_only(data_dir.to_path_buf(), None).ok();
    let connections = keyring
        .as_ref()
        .map(|keyring| keyring.view().connections)
        .unwrap_or_default();
    let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
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
        // 每个 Attempt 的花费按它自己的会话日志 usage + 同一套价格口径现算。
        let attempt_stats = stats::session_stats(&events, &connections, &catalog, &selection, 0, 0);
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
            cost_usd: attempt_stats.cost_usd,
            unpriced: attempt_stats.unpriced,
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
            project_id: task.project_id.clone(),
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

/// 本次请求 vs 同任务此前已完成请求最长；只写事实，不设阈值。
fn longest_request_anomaly(
    events: &[session::Event],
    task_id: &str,
    running: bool,
    in_flight_request_at: Option<u64>,
    now_ms: u64,
) -> Option<MonitorAnomaly> {
    let mut timings: Vec<(u64, u64)> = events
        .iter()
        .filter_map(|event| {
            stats::request_duration_ms(event).map(|duration| (event.time, duration))
        })
        .collect();
    timings.sort_by_key(|(time, _)| *time);
    let (current, prior, suffix) = if running {
        // 在飞：用 agent 内存里的 requestAt→现在；没有 requestAt 就没有可比。
        let request_at = in_flight_request_at?;
        let current = now_ms.saturating_sub(request_at);
        let prior = timings.iter().map(|(_, duration)| *duration).max()?;
        (current, prior, "（在飞）")
    } else {
        let (_, current) = *timings.last()?;
        let prior = timings[..timings.len().saturating_sub(1)]
            .iter()
            .map(|(_, duration)| *duration)
            .max()?;
        (current, prior, "")
    };
    // 只有本次严格超过同任务此前最长才写事实；没有可比/不超都不出。
    if current <= prior {
        return None;
    }
    Some(MonitorAnomaly {
        kind: "request-longest".to_owned(),
        task_id: Some(task_id.to_owned()),
        detail: format!(
            "本次 {}{suffix}，此前最长 {}",
            human_duration(current),
            human_duration(prior)
        ),
    })
}

/// 服务端写人话时长：毫秒/秒/分钟。
fn human_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}毫秒")
    } else if ms < 10_000 {
        format!("{:.1}秒", ms as f64 / 1_000.0)
    } else if ms < 60_000 {
        format!("{}秒", ms / 1_000)
    } else {
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1_000;
        format!("{minutes}分{seconds:02}秒")
    }
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

/// 范围内每天都给一根柱；没事件的天留空（不伪造 0 之外的账）。
fn fill_days(days: &mut BTreeMap<String, MonitorDay>, range: MonitorRange, tz: Tz, now_ms: i64) {
    let start = Utc
        .timestamp_millis_opt(range_start(range, tz.name(), now_ms))
        .single();
    let Some(start) = start else { return };
    let today = Utc
        .timestamp_millis_opt(now_ms)
        .single()
        .map(|now| now.with_timezone(&tz).date_naive());
    let Some(today) = today else { return };
    let mut date = start.with_timezone(&tz).date_naive();
    while date <= today {
        let key = date.format("%Y-%m-%d").to_string();
        days.entry(key.clone()).or_insert(MonitorDay {
            day: key,
            cost_usd: None,
            unpriced: 0,
            input_tokens: 0,
            output_tokens: 0,
            requests: 0,
            tool_calls: 0,
            command_calls: 0,
            groups: Vec::new(),
        });
        date += Duration::days(1);
    }
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

    pub(crate) fn temp_store() -> (tempfile::TempDir, tempfile::TempDir, Store) {
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
        let snapshot = snapshot(
            &store,
            data.path(),
            MonitorRange::Today,
            "UTC",
            &HashMap::new(),
            &[],
            &HashMap::new(),
            3,
            &[],
        )
        .unwrap();
        assert!(snapshot.tasks.is_empty());
        assert_eq!(snapshot.days.len(), 1, "今天也要有一根空柱");
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
                        quote: None,
                        quote_source: None,
                        kind_note: None,
                        project_id: None,
                        cwd_source: None,
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
        let snapshot = snapshot(
            &store,
            data.path(),
            MonitorRange::Month,
            "UTC",
            &HashMap::new(),
            &[],
            &HashMap::new(),
            3,
            &[],
        )
        .unwrap();
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
            project_labels: HashMap::new(),
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
            &CostGroup::Personal,
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

    fn timed_call(seq: u64, time: u64, call_id: &str, name: &str) -> session::Event {
        session::Event::new(
            seq,
            time,
            EventKind::ToolCall {
                turn: 1,
                step: 1,
                call_id: call_id.to_owned(),
                name: name.to_owned(),
                arguments: if name == "bash" {
                    serde_json::json!({"command": "ls"}).to_string()
                } else {
                    "{}".to_owned()
                },
            },
        )
    }

    fn timed_result(seq: u64, time: u64, call_id: &str) -> session::Event {
        session::Event::new(
            seq,
            time,
            EventKind::ToolResult {
                turn: 1,
                step: 1,
                message: Message::tool_result(call_id, "ok", false),
                error: None,
                meta: None,
            },
        )
    }

    #[test]
    fn rings_split_tool_and_command_and_day_groups_by_project() {
        let now = 1_700_000_000_000_u64;
        let assistant = session::Event::new(
            0,
            now,
            EventKind::AssistantMessage {
                turn: 1,
                step: 1,
                message: Message::assistant(
                    vec![model::ContentBlock::text("ok")],
                    "deepseek",
                    "deepseek-flash",
                ),
                stream: Vec::new(),
                usage: Some(model::Usage {
                    input_tokens: Some(1_000),
                    output_tokens: Some(10),
                    cache_read_input_tokens: Some(0),
                    cache_creation_input_tokens: Some(0),
                }),
                interrupted: None,
                effort: None,
                effort_wanted: None,
                request_at: Some(now),
                context_parts: None,
            },
        );
        let events = vec![
            assistant,
            timed_call(1, now, "t1", "read"),
            timed_result(2, now + 25, "t1"),
            timed_call(3, now + 40, "b1", "bash"),
            timed_result(4, now + 100, "b1"),
        ];
        let mut agg = Agg {
            connections: Vec::new(),
            catalog: model::catalog::snapshot(),
            models: Vec::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            tasks: Vec::new(),
            anomalies: Vec::new(),
            days: BTreeMap::new(),
            project_labels: HashMap::from([("p1".to_owned(), "项目一".to_owned())]),
        };
        let selection = session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        };
        let mut rings = MonitorRings::default();
        agg.add_events(
            &events,
            Some("task-1"),
            &CostGroup::Project("p1".to_owned()),
            &selection,
            chrono_tz::UTC,
            &mut rings,
        );
        assert_eq!(rings.tool_ms, 25, "工具 call→result 配对");
        assert_eq!(rings.command_ms, 60, "bash call→result 配对");
        assert_eq!(rings.tool_ms + rings.command_ms, 85, "两段不混");
        assert!(
            agg.models
                .iter()
                .all(|row| row.project_id.as_deref() == Some("p1"))
        );
        assert_eq!(agg.tools[0].project_id.as_deref(), Some("p1"));
        assert_eq!(agg.commands[0].project_id.as_deref(), Some("p1"));
        let day = agg.days.values().next().expect("今天有一根柱");
        assert_eq!(day.groups.len(), 1);
        assert_eq!(day.groups[0].label, "项目一");
        assert_eq!(day.groups[0].project_id.as_deref(), Some("p1"));

        // Kind 对话同一天单独一组（projectId 空，label 区分）。
        let mut kind_events = vec![session::Event::new(
            9,
            now + 10,
            EventKind::AssistantMessage {
                turn: 1,
                step: 1,
                message: Message::assistant(
                    vec![model::ContentBlock::text("ok")],
                    "deepseek",
                    "deepseek-flash",
                ),
                stream: Vec::new(),
                usage: Some(model::Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(1),
                    cache_read_input_tokens: Some(0),
                    cache_creation_input_tokens: Some(0),
                }),
                interrupted: None,
                effort: None,
                effort_wanted: None,
                request_at: Some(now + 10),
                context_parts: None,
            },
        )];
        agg.add_events(
            &mut kind_events,
            None,
            &CostGroup::Kind,
            &selection,
            chrono_tz::UTC,
            &mut rings,
        );
        let day = agg.days.values().next().unwrap();
        let labels: Vec<&str> = day
            .groups
            .iter()
            .map(|group| group.label.as_str())
            .collect();
        assert!(labels.contains(&"项目一"));
        assert!(labels.contains(&"Kind 对话"));
        assert!(labels.contains(&"个人") == false || labels.contains(&"个人"));
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
            project_labels: HashMap::new(),
        };
        let selection = session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        };
        agg.add_events(
            &[assistant(day_one, 1_000), assistant(day_two, 1_000)],
            Some("task-1"),
            &CostGroup::Personal,
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
            project_labels: HashMap::new(),
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
            &CostGroup::Personal,
            &selection,
            shanghai,
            &mut MonitorRings::default(),
        );
        assert_eq!(agg.days["2026-11-19"].tool_calls, 1);
        assert_eq!(agg.days["2026-11-20"].tool_calls, 1);
    }
}

#[cfg(test)]
mod t075_usage_tests {
    use super::*;
    use model::Message;

    fn usage(input: u64, output: u64) -> model::Usage {
        model::Usage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        }
    }

    fn assistant(turn: u64, input: u64, output: u64) -> session::EventKind {
        session::EventKind::AssistantMessage {
            turn,
            step: 1,
            message: Message::assistant(
                vec![model::ContentBlock::text("ok")],
                "deepseek",
                "deepseek-flash",
            ),
            stream: Vec::new(),
            usage: Some(usage(input, output)),
            interrupted: None,
            effort: Some("high".to_owned()),
            effort_wanted: Some("high".to_owned()),
            request_at: None,
            context_parts: None,
        }
    }

    fn task_with_usage(
        data: &std::path::Path,
        cwd: &std::path::Path,
        store: &Store,
        inputs: &[u64],
    ) -> tasks::Task {
        let now = 1_700_000_000_000_i64;
        let task = store
            .create_task(
                tasks::NewTask {
                    quote: None,
                    quote_source: None,
                    kind_note: None,
                    project_id: None,
                    cwd_source: None,
                    title: "D78 标记文件".to_owned(),
                    requirement: "写标记文件".to_owned(),
                    dod: "标记文件存在".to_owned(),
                    cwd: cwd.to_path_buf(),
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
        let mut session = session::SessionStore::create(data, cwd).unwrap();
        // 同一轮里两条 assistant usage：requests 要数 assistant 消息（2），
        // 不能数 Turn（1），也不能把最后一条再加一遍。
        let turn = 1;
        let mut events = vec![
            session::EventKind::TurnStart { turn },
            session::EventKind::StepStart { turn, step: 1 },
        ];
        for input in inputs {
            events.push(assistant(turn, *input, 100));
        }
        events.push(session::EventKind::TurnEnd {
            turn,
            reason: session::TurnEndReason::Completed,
        });
        session.append(&events).unwrap();
        store
            .attach_session(
                &task.id,
                attempt,
                tasks::ConversationId::new(session.id().to_string()),
            )
            .unwrap();
        task
    }

    #[test]
    fn task_input_tokens_match_task_read_stats_not_last_usage_twice() {
        let (data, cwd, store) = super::tests::temp_store();
        // D78 固定日志：总 input 8540，最后一条 assistant 是 486；重复加会变 9026。
        let task = task_with_usage(data.path(), cwd.path(), &store, &[8_054, 486]);
        let snapshot = snapshot(
            &store,
            data.path(),
            MonitorRange::Today,
            "UTC",
            &HashMap::new(),
            &[],
            &HashMap::new(),
            3,
            &[],
        )
        .unwrap();
        let entry = snapshot
            .tasks
            .iter()
            .find(|entry| entry.task_id == task.id.as_str())
            .expect("任务在快照里");
        assert_eq!(entry.input_tokens, 8_540, "不能把最后一条 usage 再加一遍");
        assert_eq!(entry.requests, 2, "两次 assistant 请求");
        assert_eq!(entry.output_tokens, 200);
    }

    fn timed(seq: u64, time: u64, request_at: u64, last_frame: u64) -> session::Event {
        let duration = last_frame - request_at;
        session::Event::new(
            seq,
            time,
            session::EventKind::AssistantMessage {
                turn: 1,
                step: 1,
                message: Message::assistant(
                    vec![model::ContentBlock::text("ok")],
                    "deepseek",
                    "deepseek-flash",
                ),
                stream: vec![model::AssistantStreamRecord::TextChunks {
                    time0: request_at,
                    index: 0,
                    dt: vec![duration],
                    texts: vec!["a".to_owned(), "b".to_owned()],
                }],
                usage: Some(usage(10, 5)),
                interrupted: None,
                effort: None,
                effort_wanted: None,
                request_at: Some(request_at),
                context_parts: None,
            },
        )
    }

    fn task_with_events(
        data: &std::path::Path,
        cwd: &std::path::Path,
        store: &Store,
        events: Vec<session::EventKind>,
    ) -> tasks::Task {
        let now = 1_700_000_000_000_i64;
        let task = store
            .create_task(
                tasks::NewTask {
                    quote: None,
                    quote_source: None,
                    kind_note: None,
                    project_id: None,
                    cwd_source: None,
                    title: "异常事实任务".to_owned(),
                    requirement: "跑".to_owned(),
                    dod: "跑完".to_owned(),
                    cwd: cwd.to_path_buf(),
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
        let mut session = session::SessionStore::create(data, cwd).unwrap();
        session.append(&events).unwrap();
        store
            .attach_session(
                &task.id,
                attempt,
                tasks::ConversationId::new(session.id().to_string()),
            )
            .unwrap();
        task
    }

    #[test]
    fn snapshot_wires_longest_request_anomaly() {
        let (data, cwd, store) = super::tests::temp_store();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let mut first = timed(1, now - 10_000, now - 10_000, now - 9_000);
        let mut second = timed(2, now - 5_000, now - 5_000, now - 3_000);
        if let session::EventKind::AssistantMessage { turn, .. } = &mut first.kind {
            *turn = 1;
        }
        if let session::EventKind::AssistantMessage { turn, .. } = &mut second.kind {
            *turn = 2;
        }
        let task = task_with_events(
            data.path(),
            cwd.path(),
            &store,
            vec![
                session::EventKind::TurnStart { turn: 1 },
                first.kind,
                session::EventKind::TurnStart { turn: 2 },
                second.kind,
            ],
        );
        let snapshot = snapshot(
            &store,
            data.path(),
            MonitorRange::Today,
            "UTC",
            &HashMap::new(),
            &[],
            &HashMap::new(),
            3,
            &[],
        )
        .unwrap();
        let anomaly = snapshot
            .anomalies
            .iter()
            .find(|anomaly| anomaly.task_id.as_deref() == Some(task.id.as_str()))
            .expect("snapshot 应带最长请求事实");
        assert_eq!(anomaly.kind, "request-longest");
        assert_eq!(anomaly.detail, "本次 2.0秒，此前最长 1.0秒");
    }

    #[test]
    fn longest_request_only_reports_when_current_exceeds_prior() {
        let events = vec![
            timed(1, 100, 1_000, 1_500),
            timed(2, 200, 2_000, 2_900),
            timed(3, 300, 3_000, 3_700),
        ];
        // 本次 700ms < 此前最长 900ms：不出。
        assert!(longest_request_anomaly(&events, "task-1", false, None, 0).is_none());
        // 没有此前请求：不出。
        assert!(longest_request_anomaly(&events[..1], "task-1", false, None, 0).is_none());
        // 本次 2.1 秒 > 此前 900ms：出，人话。
        let longer = timed(4, 400, 4_000, 6_100);
        let anomaly = longest_request_anomaly(
            &[events[0].clone(), events[1].clone(), longer],
            "task-1",
            false,
            None,
            0,
        )
        .unwrap();
        assert_eq!(anomaly.kind, "request-longest");
        assert_eq!(anomaly.detail, "本次 2.1秒，此前最长 900毫秒");
        let missing = timed(5, 500, 5_000, 5_700);
        let mut missing = missing;
        if let session::EventKind::AssistantMessage {
            request_at, stream, ..
        } = &mut missing.kind
        {
            *request_at = None;
            stream.clear();
        }
        assert!(longest_request_anomaly(&[missing], "task-1", false, None, 0).is_none());
    }

    #[test]
    fn human_duration_uses_minutes_seconds_milliseconds() {
        assert_eq!(human_duration(840_000), "14分00秒");
        assert_eq!(human_duration(21_000), "21秒");
        assert_eq!(human_duration(1_500), "1.5秒");
        assert_eq!(human_duration(10_000), "10秒");
        assert_eq!(human_duration(800), "800毫秒");
    }

    #[test]
    fn in_flight_uses_agent_memory_request_at() {
        let completed = timed(1, 1_000, 1_000, 1_500);
        let anomaly =
            longest_request_anomaly(&[completed], "task-1", true, Some(1_800), 3_000).unwrap();
        assert_eq!(anomaly.detail, "本次 1.2秒（在飞），此前最长 500毫秒");
        assert!(longest_request_anomaly(&[], "task-1", true, Some(2_000), 2_500).is_none());
    }

    #[test]
    fn in_flight_without_request_at_does_not_report() {
        let completed = timed(1, 1_000, 1_000, 1_500);
        let open =
            session::Event::new(2, 1_800, session::EventKind::StepStart { turn: 2, step: 1 });
        assert!(longest_request_anomaly(&[completed, open], "task-1", true, None, 3_000).is_none());
        assert!(
            longest_request_anomaly(
                &[session::Event::new(
                    1,
                    1_000,
                    session::EventKind::StepStart { turn: 1, step: 1 },
                )],
                "task-1",
                true,
                None,
                2_000,
            )
            .is_none()
        );
    }

    #[test]
    fn repeated_same_command_is_one_fact_and_other_bash_resets() {
        let bash = |seq, call: &str, command: &str| {
            session::Event::new(
                seq,
                10,
                session::EventKind::ToolCall {
                    turn: 1,
                    step: 1,
                    call_id: call.to_owned(),
                    name: "bash".to_owned(),
                    arguments: serde_json::json!({"command": command}).to_string(),
                },
            )
        };
        let result = |seq, call: &str, text: &str| {
            session::Event::new(
                seq,
                20,
                session::EventKind::ToolResult {
                    turn: 1,
                    step: 1,
                    message: Message::tool_result(call, text, false),
                    error: None,
                    meta: None,
                },
            )
        };
        let read = |seq| {
            session::Event::new(
                seq,
                15,
                session::EventKind::ToolCall {
                    turn: 1,
                    step: 1,
                    call_id: "read".to_owned(),
                    name: "read".to_owned(),
                    arguments: serde_json::json!({"path": "a"}).to_string(),
                },
            )
        };
        let events = vec![
            bash(1, "c1", "cargo test -q"),
            result(2, "c1", "ok"),
            read(3),
            bash(4, "c2", "cargo test -q"),
            result(5, "c2", "ok"),
            bash(6, "c3", "cargo test -q"),
            result(7, "c3", "ok"),
            bash(8, "c4", "cargo test -q"),
            result(9, "c4", "different"),
            bash(10, "c5", "ls"),
            result(11, "c5", "ok"),
        ];
        let facts = crate::commands::repeated_commands(&events);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].count, 3);
        assert_eq!(facts[0].command, "cargo test -q");
    }

    #[test]
    fn multiple_attempts_are_summed_once() {
        let (data, cwd, store) = super::tests::temp_store();
        let task = task_with_usage(data.path(), cwd.path(), &store, &[8_054, 486]);
        // 第二个 Attempt：再 1000 input，monitor 应等于两个会话 stats 之和，不重复。
        let loaded = store.task(&task.id).unwrap().expect("任务存在");
        let first = loaded.latest_attempt().expect("第一个 Attempt").id;
        store
            .end_attempt(
                &task.id,
                first,
                1_700_000_000_000,
                tasks::AttemptEnd::Reported,
            )
            .unwrap();
        let attempt = store.begin_attempt(&task.id, 1_700_000_000_001).unwrap();
        let mut session = session::SessionStore::create(data.path(), cwd.path()).unwrap();
        session
            .append(&[
                session::EventKind::TurnStart { turn: 1 },
                session::EventKind::StepStart { turn: 1, step: 1 },
                assistant(1, 1_000, 10),
                session::EventKind::TurnEnd {
                    turn: 1,
                    reason: session::TurnEndReason::Completed,
                },
            ])
            .unwrap();
        store
            .attach_session(
                &task.id,
                attempt,
                tasks::ConversationId::new(session.id().to_string()),
            )
            .unwrap();
        let snapshot = snapshot(
            &store,
            data.path(),
            MonitorRange::Today,
            "UTC",
            &HashMap::new(),
            &[],
            &HashMap::new(),
            3,
            &[],
        )
        .unwrap();
        let entry = snapshot
            .tasks
            .iter()
            .find(|entry| entry.task_id == task.id.as_str())
            .expect("任务在快照里");
        assert_eq!(entry.input_tokens, 9_540, "多 Attempt 每个会话只加一遍");
        assert_eq!(entry.requests, 3);
    }

    #[test]
    fn attempt_cost_is_computed_from_its_own_usage() {
        let (data, cwd, store) = super::tests::temp_store();
        let keyring = credentials::Keyring::open_read_only(data.path().to_path_buf(), None)
            .expect("空 keyring 也能打开");
        keyring
            .save_connection(credentials::NewConnection {
                id: "deepseek".to_owned(),
                name: "DeepSeek".to_owned(),
                model: "deepseek-flash".to_owned(),
                endpoint: "https://api.deepseek.com".to_owned(),
                api_key: "sk-test".to_owned(),
                preset: None,
                models: Some(vec![credentials::ModelRow {
                    id: "deepseek-flash".to_owned(),
                    name: None,
                    context_window: None,
                    max_tokens: None,
                    price_input: Some(1.0),
                    price_output: Some(2.0),
                    price_cache_read: Some(0.0),
                    price_cache_write: Some(0.0),
                }]),
                auth_header: None,
            })
            .expect("存连接");
        let task_fixture = task_with_usage(data.path(), cwd.path(), &store, &[8_054, 486]);
        let detail = task(
            &store,
            data.path(),
            MonitorTaskRequest {
                task_id: task_fixture.id.as_str().to_owned(),
                attempt_id: None,
            },
            false,
        )
        .unwrap();
        let attempt = detail.attempts.first().expect("有一个 Attempt");
        assert_eq!(attempt.input_tokens, 8_540);
        assert!(attempt.cost_usd.is_some(), "Attempt 花费按自己的日志现算");
        assert!(!attempt.unpriced, "有价目录里不能标无价");
    }
}

#[cfg(test)]
mod t075_price_tests {
    use super::*;

    fn connection(id: &str, input_price: f64) -> credentials::ConnectionInfo {
        credentials::ConnectionInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            model: "priced-model".to_owned(),
            endpoint: "https://api.example.com/anthropic".to_owned(),
            saved_at: 0,
            check: credentials::CheckState::Never,
            key_saved: true,
            preset: "deepseek".to_owned(),
            models: Some(vec![credentials::ModelRow {
                id: "priced-model".to_owned(),
                name: None,
                context_window: None,
                max_tokens: None,
                price_input: Some(input_price),
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
            }]),
            auth_header: None,
        }
    }

    fn event(seq: u64, time: u64) -> session::Event {
        session::Event::new(
            seq,
            time,
            EventKind::AssistantMessage {
                turn: 1,
                step: 1,
                message: model::Message::assistant(
                    vec![model::ContentBlock::text("ok")],
                    "example",
                    "priced-model",
                ),
                stream: Vec::new(),
                usage: Some(model::Usage {
                    input_tokens: Some(1_000_000),
                    output_tokens: Some(0),
                    cache_read_input_tokens: Some(0),
                    cache_creation_input_tokens: Some(0),
                }),
                interrupted: None,
                effort: None,
                effort_wanted: None,
                request_at: Some(time),
                context_parts: None,
            },
        )
    }

    fn agg() -> Agg {
        Agg {
            connections: vec![connection("cheap", 1.0), connection("expensive", 100.0)],
            catalog: model::catalog::snapshot(),
            models: Vec::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            tasks: Vec::new(),
            anomalies: Vec::new(),
            days: BTreeMap::new(),
            project_labels: HashMap::new(),
        }
    }

    #[test]
    fn price_uses_event_connection_not_first_matching_model() {
        let time = 1_700_000_000_000;
        let mut cheap = agg();
        let mut rings = MonitorRings::default();
        cheap.add_events(
            &[
                session::Event::new(1, time - 10, EventKind::TurnStart { turn: 1 }),
                event(2, time),
            ],
            None,
            &CostGroup::Kind,
            &session::ModelSelection {
                connection: "cheap".to_owned(),
                model: "priced-model".to_owned(),
                effort_wanted: "high".to_owned(),
            },
            "UTC".parse().unwrap(),
            &mut rings,
        );
        assert_eq!(cheap.models[0].cost_usd, Some(1.0), "cheap 连接价");
        assert_eq!(
            cheap.days.values().next().and_then(|day| day.cost_usd),
            Some(1.0),
            "日柱同样按事件连接价"
        );
        assert!(!rings.unpriced, "有价连接不能标无价");

        let mut expensive = agg();
        let mut ring2 = MonitorRings::default();
        expensive.add_events(
            &[event(2, time)],
            None,
            &CostGroup::Kind,
            &session::ModelSelection {
                connection: "expensive".to_owned(),
                model: "priced-model".to_owned(),
                effort_wanted: "high".to_owned(),
            },
            "UTC".parse().unwrap(),
            &mut ring2,
        );
        assert_eq!(
            expensive.models[0].cost_usd,
            Some(100.0),
            "expensive 连接价"
        );
    }

    #[test]
    fn unknown_connection_is_unpriced_not_borrowed_from_first() {
        let mut agg = agg();
        let mut rings = MonitorRings::default();
        agg.add_events(
            &[event(2, 1_700_000_000_000)],
            None,
            &CostGroup::Kind,
            &session::ModelSelection {
                connection: "missing".to_owned(),
                model: "priced-model".to_owned(),
                effort_wanted: "high".to_owned(),
            },
            "UTC".parse().unwrap(),
            &mut rings,
        );
        assert_eq!(agg.models[0].cost_usd, None);
        assert!(rings.unpriced);
    }
}
