//! `aios_query`：Kind 查 AIOS 现在的设置、连接、模型、用量和产品信息。
//!
//! 只读服务里**已经存下的**数据（D71）：设置读 `settings.json`、连接读 Keyring 的视图、
//! 模型读目录和存下的厂商列表、用量读会话日志、产品信息是编进二进制的常数。不现拉厂商
//! 列表（保存连接和「获取」时才拉）；输出里没有 key 正文，也不带连接的地址和
//! 鉴权头——连接只给名字、提供方、有没有 key、上次检查结果。
//!
//! 挂在 skill `aios-settings` 后面：本会话加载过它以后，下一步才开出这个工具。

use chrono::{Datelike, Local, TimeZone};
use std::path::PathBuf;

use serde_json::{Value, json};
use tools::{Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolResult};

use crate::protocol::WorkResponse;
use crate::work::{TaskFact, WaitItem};

/// 查询的七个主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Topic {
    Settings,
    Connections,
    Models,
    Usage,
    Product,
    Calendar,
    Work,
}

impl Topic {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "settings" => Some(Self::Settings),
            "connections" => Some(Self::Connections),
            "models" => Some(Self::Models),
            "usage" => Some(Self::Usage),
            "product" => Some(Self::Product),
            "calendar" => Some(Self::Calendar),
            "work" => Some(Self::Work),
            _ => None,
        }
    }
}

/// 只读查询工具。
pub struct QueryTool {
    data_dir: PathBuf,
    /// Keyring 的钥匙文件位置；None = 用数据目录里的配置/默认位置。
    identity: Option<PathBuf>,
    /// 这个装配实际开出的 Kind 工具名单；product 的「能做」逐行跟它走。
    tool_names: Vec<String>,
    parameters: Value,
}

impl QueryTool {
    pub fn new(data_dir: impl Into<PathBuf>, identity: Option<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            identity,
            tool_names: crate::root::KIND_TOOLS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "enum": ["settings", "connections", "models", "usage", "product", "calendar", "work"],
                        "description": "settings: this conversation's model and effort, the selection new conversations use, and the currency. connections: connections already stored, with whether a key is saved and the last verification result. models: the models each connection can pick, with masking rules. usage: this conversation's turns, steps, tokens and cost. product: what AIOS can and cannot do. calendar: appointments, their triggers/bindings/watch/cycle, and recent runs. work: the same work snapshot the work page reads, projected into facts and actions (no graph edges)."
                    }
                },
                "required": ["topic"]
            }),
        }
    }

    pub fn with_tool_names(mut self, names: Vec<String>) -> Self {
        self.tool_names = names;
        self
    }

    fn read(&self, topic: Topic, context: &ToolContext) -> Result<Value, ToolFailure> {
        match topic {
            Topic::Settings => self.settings(context),
            Topic::Connections => self.connections(),
            Topic::Models => self.models(),
            Topic::Usage => self.usage(context),
            Topic::Product => Ok(product(&self.tool_names)),
            Topic::Calendar => self.calendar(),
            Topic::Work => self.work(context),
        }
    }

    fn calendar(&self) -> Result<Value, ToolFailure> {
        let path = self.data_dir.join("calendar.sqlite");
        if !path.exists() {
            return Ok(json!({ "appointments": [] }));
        }
        let store = calendar::CalendarStore::open(&path)
            .map_err(|error| ToolFailure::plain(format!("日历库读不出来：{error}")))?;
        let appointments = store
            .list_appointments()
            .map_err(|error| ToolFailure::plain(format!("日历读不出来：{error}")))?;
        let mut rows = Vec::with_capacity(appointments.len());
        for appointment in appointments {
            let runs = store
                .runs(&appointment.id)
                .unwrap_or_default()
                .into_iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>();
            rows.push(json!({
                "id": appointment.id,
                "title": appointment.title,
                "condition": appointment.condition,
                "status": appointment.status.as_str(),
                "version": appointment.version,
                "rule": appointment.schedule.rule,
                "trigger": crate::calendar_trigger::describe(
                    &appointment.schedule.rule,
                    &appointment.schedule.timezone,
                    appointment.schedule.start_at,
                ),
                "timezone": appointment.schedule.timezone,
                "startAt": appointment.schedule.start_at,
                "binding": match &appointment.binding {
                    calendar::Binding::Kind => json!({ "kind": "kind" }),
                    calendar::Binding::Task(task) => json!({ "kind": "task", "taskId": task }),
                },
                "watch": appointment.watch.as_ref().map(|watch| match watch {
                    calendar::WatchSource::Url(url) => json!({ "kind": "url", "target": url }),
                    calendar::WatchSource::File(file) => json!({ "kind": "file", "target": file }),
                    calendar::WatchSource::TaskFacts(task) => json!({ "kind": "taskFacts", "target": task }),
                }),
                "cycle": appointment.cycle.as_ref().map(|cycle| json!({
                    "taskId": cycle.task_id,
                    "maxRounds": cycle.max_rounds,
                })),
                "nextTriggerAt": appointment.next_trigger_at,
                "sourceTurn": appointment.source_turn,
                "runs": runs.iter().map(|run| json!({
                    "id": run.id,
                    "trigger": run.trigger.clone().as_str(),
                    "result": run.result.as_str(),
                    "reason": run.reason,
                    "nominalAt": run.nominal_at,
                    "mergedCount": run.merged_count,
                    "sentTurn": run.sent_turn,
                })).collect::<Vec<_>>(),
            }));
        }
        Ok(json!({ "appointments": rows }))
    }

    fn work(&self, context: &ToolContext) -> Result<Value, ToolFailure> {
        let store = tasks::Store::open(self.data_dir.join(crate::TASKS_DB_FILE))
            .map_err(|error| ToolFailure::plain(format!("任务库读不出来：{error}")))?;
        let events = context.session_events();
        let pending = crate::work::pending_questions(&events);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or_default();
        let mut response = crate::work::snapshot(&store, &events, &pending, now)
            .map_err(|error| ToolFailure::plain(format!("工作快照读不出来：{error}")))?;
        // 页面 work_snapshot 还会合并日历暂停的等你项；这里必须走同一个合并，否则件数对不上。
        let calendar_path = self.data_dir.join("calendar.sqlite");
        if calendar_path.exists()
            && let Ok(calendar) = calendar::CalendarStore::open(&calendar_path)
            && let Ok(waits) = crate::work::calendar_pause_waits(&calendar)
        {
            crate::work::merge_waits(&mut response, waits);
        }
        Ok(project_work(&response, now))
    }

    fn keyring(&self) -> Result<credentials::Keyring, ToolFailure> {
        credentials::Keyring::open(&self.data_dir, self.identity.as_deref())
            .map_err(|error| ToolFailure::plain(format!("读不出 Keyring 的视图：{error}")))
    }

    fn settings(&self, context: &ToolContext) -> Result<Value, ToolFailure> {
        let settings = crate::settings::load(&self.data_dir);
        let current = crate::settings::current_selection(&self.data_dir, &context.session_events())
            .ok_or_else(|| ToolFailure::plain("这条对话还没有选过模型，也没有上次的选择可读"))?;
        let next = settings.last_selection.map_or(Value::Null, |selection| {
            json!({
                "connection": selection.connection,
                "model": selection.model,
                "effort": selection.effort_wanted,
            })
        });
        Ok(json!({
            "session": {
                "connection": current.connection,
                "model": current.model,
                "effort": current.effort_wanted,
            },
            "newConversations": next,
            "currency": settings.currency,
        }))
    }

    fn connections(&self) -> Result<Value, ToolFailure> {
        let view = self.keyring()?.view();
        let rows: Vec<Value> = view
            .connections
            .iter()
            .map(|connection| {
                json!({
                    "id": connection.id,
                    "name": connection.name,
                    "provider": connection.preset,
                    "hasKey": connection.key_saved,
                    "lastCheck": serde_json::to_value(&connection.check).unwrap_or(Value::Null),
                })
            })
            .collect();
        Ok(json!({ "connections": rows }))
    }

    fn models(&self) -> Result<Value, ToolFailure> {
        let view = self.keyring()?.view();
        let disabled = crate::disabled::table(&self.data_dir);
        let rows: Vec<Value> = view
            .connections
            .iter()
            .map(|connection| {
                let primary = crate::models::primary_model(&self.data_dir, connection);
                let models: Vec<Value> =
                    crate::models::selectable_models(&self.data_dir, connection)
                        .into_iter()
                        .map(|info| {
                            let masked = crate::disabled::match_disabled(
                                &disabled.models,
                                &connection.id,
                                &info.id,
                            );
                            json!({
                                "id": info.id,
                                "name": info.name,
                                "primary": info.id == primary,
                                "disabled": masked.is_some(),
                            })
                        })
                        .collect();
                json!({
                    "id": connection.id,
                    "name": connection.name,
                    "models": models,
                })
            })
            .collect();
        Ok(json!({
            "connections": rows,
            "disabled": disabled.models,
            "disabledUnknown": disabled.unknown,
        }))
    }

    fn usage(&self, context: &ToolContext) -> Result<Value, ToolFailure> {
        let events = context.session_events();
        if events.is_empty() {
            return Err(ToolFailure::plain("拿不到这条对话的会话日志"));
        }
        let view = self.keyring()?.view();
        let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
        let current = crate::settings::current_selection(&self.data_dir, &events)
            .ok_or_else(|| ToolFailure::plain("这条对话还没有选过模型，也没有上次的选择可读"))?;
        let stats =
            crate::stats::session_stats(&events, &view.connections, &catalog, &current, 0, 0);
        Ok(json!({
            "turns": stats.turns,
            "steps": stats.steps,
            "inputTokens": stats.input_tokens,
            "outputTokens": stats.output_tokens,
            "cacheReadTokens": stats.cache_read_tokens,
            "cacheWriteTokens": stats.cache_write_tokens,
            "costUsd": stats.cost_usd,
            "unpriced": stats.unpriced,
            "connection": stats.connection,
            "model": stats.model,
        }))
    }
}

/// 产品信息和能力边界（A04）：编进二进制的常数，不读磁盘。
fn wait_action(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "resume" | "confirm" => ("去看", "你"),
        "answer" => ("去答", "你"),
        "judge" => ("去判", "你"),
        "accept" => ("看交付记录", "你"),
        _ => ("查看", "你"),
    }
}

fn task_action(task: &TaskFact, group: &str) -> (&'static str, &'static str) {
    if group == "problems" {
        return ("让 Kind 补条件", "Kind");
    }
    if group == "advancing" {
        return ("ThinAgent 推进", "ThinAgent");
    }
    if task.after_waiting {
        ("等前置完成", "ThinAgent")
    } else if task.stage == "needs_conditions" {
        ("等 Kind 补条件", "Kind")
    } else {
        ("等排程", "排程")
    }
}

fn task_reason(task: &TaskFact) -> String {
    task.latest
        .as_ref()
        .and_then(|latest| {
            latest
                .question
                .clone()
                .or_else(|| Some(latest.summary.clone()))
        })
        .filter(|text| !text.trim().is_empty())
        .or_else(|| task.after_waiting.then(|| "等前置完成".to_owned()))
        .unwrap_or_else(|| task.stage.clone())
}

fn project_task(task: &TaskFact, group: &str) -> Value {
    let (action, owner) = task_action(task, group);
    let cancel = if task.cancelled_at.is_none() && task.stage != "closed" {
        json!("取消（Kind 出卡）")
    } else {
        Value::Null
    };
    json!({
        "id": task.id,
        "title": task.title,
        "stage": task.stage,
        "reason": task_reason(task),
        "also": [],
        "action": action,
        "actionOwner": owner,
        "cancel": cancel,
    })
}

fn project_wait(item: &WaitItem) -> Value {
    let (action, owner) = wait_action(&item.kind);
    let id = item
        .task_id
        .clone()
        .unwrap_or_else(|| format!("{}:{}:{}", item.kind, item.title, item.at));
    json!({
        "id": id,
        "taskId": item.task_id,
        "appointmentId": item.appointment_id,
        "title": item.title,
        "stage": item.stage,
        "reason": item.body,
        "need": item.need,
        "also": item.also,
        "action": action,
        "actionOwner": owner,
        "cancel": "取消（Kind 出卡）",
    })
}

fn week_start_ms(now_ms: i64) -> i64 {
    let Some(now) = Local.timestamp_millis_opt(now_ms).single() else {
        return now_ms;
    };
    let date = now.date_naive();
    let monday = date - chrono::Duration::days(i64::from(date.weekday().num_days_from_monday()));
    Local
        .from_local_datetime(&monday.and_hms_opt(0, 0, 0).expect("midnight is valid"))
        .single()
        .map(|at| at.timestamp_millis())
        .unwrap_or(now_ms)
}

/// T077.1 F2：逐条照抄 web/src/pages/work/workModel.ts 的 columnOf 顺序：
/// 已关闭 → 等你 → 排队 → 在做 → 门已判 → 其余等你。
fn column_of(
    id: &str,
    stage: &str,
    gate: bool,
    wait_task_ids: &std::collections::HashSet<&str>,
) -> &'static str {
    if stage == "closed" {
        return "done";
    }
    if wait_task_ids.contains(id) {
        return "wait";
    }
    if stage == "needs_conditions" || stage == "pending" {
        return "queue";
    }
    if stage == "running" || stage == "ready_for_review" {
        return "doing";
    }
    if gate && stage == "requirement_met" {
        return "doing";
    }
    "wait"
}

fn done_this_week(
    stage: &str,
    gate: bool,
    cancelled_at: Option<i64>,
    closed_at: Option<i64>,
    week_start: i64,
) -> bool {
    stage == "closed"
        && !gate
        && cancelled_at.is_none()
        && closed_at.is_some_and(|at| at >= week_start)
}

fn project_work(response: &WorkResponse, now_ms: i64) -> Value {
    let wait_ids: std::collections::HashSet<&str> = response
        .wait
        .iter()
        .filter_map(|item| item.task_id.as_deref())
        .collect();
    let by_id: std::collections::HashMap<&str, &TaskFact> = response
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect();
    let mut problems = Vec::new();
    let mut advancing = Vec::new();
    let week_start = week_start_ms(now_ms);
    for task in &response.tasks {
        // G1：分组逐条照 web workModel.ts 的 columnOf / problemsOf / advancingOf。
        match column_of(task.id.as_str(), &task.stage, task.gate, &wait_ids) {
            "queue" => {
                if task.stage == "needs_conditions" && !task.after_waiting {
                    problems.push(project_task(task, "problems"));
                }
            }
            "doing" => advancing.push(project_task(task, "advancing")),
            _ => {}
        }
    }
    let done: Vec<&TaskFact> = response
        .tasks
        .iter()
        .filter(|task| {
            done_this_week(
                &task.stage,
                task.gate,
                task.cancelled_at,
                task.closed_at,
                week_start,
            )
        })
        .collect();
    let task_columns = {
        let mut groups = serde_json::Map::new();
        for id in ["wait", "queue", "doing", "done"] {
            groups.insert(id.to_owned(), json!([]));
        }
        for task in &response.tasks {
            let column = column_of(task.id.as_str(), &task.stage, task.gate, &wait_ids);
            groups[column].as_array_mut().unwrap().push(json!(task.id));
        }
        Value::Object(groups)
    };
    // J3：queue 就是 columns.queue，个数取列表长度；出了问题但仍在队列里的任务两边都在。
    let queue: Vec<Value> = task_columns["queue"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|id| by_id.get(id).map(|task| project_task(task, "queue")))
        .collect();
    json!({
        "generatedAt": response.generated_at,
        "counts": {
            "wait": response.wait.len(),
            "problems": problems.len(),
            "advancing": advancing.len(),
            "queue": queue.len(),
            "doneThisWeek": done.len(),
        },
        "wait": response.wait.iter().map(project_wait).collect::<Vec<_>>(),
        "problems": problems,
        "advancing": advancing,
        "queue": queue,
        "columns": task_columns,
        "doneThisWeek": {
            "count": done.len(),
            "titles": done.iter().map(|task| task.title.clone()).collect::<Vec<_>>(),
        },
    })
}

fn tool_can(tool: &str) -> &'static str {
    match tool {
        "read" => "读文件内容。",
        "glob" => "按路径模式找文件。",
        "grep" => "按内容搜文件。",
        "skill" => "加载技能说明再照做。",
        "todo_write" => "给这一轮列待办。",
        "ask_user_question" => "问你问题。",
        "canvas" => "把代码/文档/网页画成可看的画布。",
        "aios_query" => "读 AIOS 自己的设置、连接、模型、用量、工作页和日历。",
        "propose_action" => "出确认卡；改设置、建项目、派任务图都等你在卡上点。",
        "project_create" => "用户直接说要建项目时，当场建并回执。",
        "web_search" => "搜网页（要你在设置页存 Exa key）。",
        "web_fetch" => "读网页链接（要你在设置页存 Exa key）。",
        "memory_remember" => "在你明确说了以后，记一条长期记忆。",
        "memory_note" => "记一条自己的笔记。",
        "memory_about_draft" => "起草「关于你」的更新，等你确认。",
        "memory_search" => "搜记忆、日记和 Kind 对话。",
        "memory_read" => "读一条记忆或日记。",
        "memory_diary" => "追加一行日记。",
        "memory_dream_now" => "整理一次记忆（做梦）。",
        "memory_undo_dream" => "撤销上一次整理。",
        "memory_delete_note" => "用户明确说删时删一条笔记。",
        "kind_react" => "先给用户本轮消息回一个表情（最多一个）。",
        "kind_say" => "最后把回答分成 2 条以上发；成功后这一轮结束。",
        "task_accept" => "派一个任务或一张任务图，交给 ThinAgent。",
        "task_status" => "读一个任务的状态和最近报告。",
        "task_delivery" => "读任务的交付记录和结果卡。",
        "task_stop" => "停掉任务正在跑的这一轮（不是取消任务）。",
        "task_update" => "改未完成任务的写入范围或要求。",
        "task_graph_propose" => "提议一张任务图，用户确认后才派。",
        "task_continue" => "让被卡住/被停的任务在原会话接着做。",
        "gate_check_again" => "重跑程序门检查。",
        "appointment_cancel" => "取消一条日历约定。",
        _ => "做这件事。",
    }
}

/// T077.1 F1：product 的「能做」跟着装配实际开出的完整 Kind 名单逐行生成。
fn product(tools: &[String]) -> Value {
    let can: Vec<String> = tools
        .iter()
        .map(|tool| format!("{tool}：{}", tool_can(tool)))
        .collect();
    json!({
        "product": "AIOS",
        "about": "面向个人的桌面智能体工作台：把模型、工具和个人上下文组织成可执行的任务流。",
        "can": can,
        "cannot": [
            "自己建连接、填 key 或换 key：这些只在设置页做，要你亲手点。",
            "现拉厂商的模型列表、现测连接：那是设置页上的按钮。",
            "绕过确认卡直接改设置或派任务：要改就得你在卡上点。"
        ]
    })
}

impl Tool for QueryTool {
    fn name(&self) -> &str {
        "aios_query"
    }

    fn description(&self) -> &str {
        "Read AIOS's own current settings, connections, models, this conversation's usage, or what AIOS can and cannot do. Read-only: it never changes anything and never returns API keys."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkill(vec![
            "aios-guide".to_owned(),
            "aios-settings".to_owned(),
            "aios-calendar".to_owned(),
        ])
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let topic = match parse_topic(&call.arguments) {
                Ok(topic) => topic,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            match self.read(topic, context) {
                Ok(value) => ToolResult::success(
                    call.id,
                    serde_json::to_string_pretty(&value)
                        .unwrap_or_else(|error| format!("查询结果没法序列化成 JSON：{error}")),
                ),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn parse_topic(arguments: &str) -> Result<Topic, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let name = args
        .get("topic")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("invalid topic: expected a non-empty string"))?;
    Topic::parse(name).ok_or_else(|| {
        ToolFailure::plain(format!(
            "invalid topic {name:?}: expected settings, connections, models, usage, product or calendar"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::WaitItem;

    #[test]
    fn product_can_has_exactly_one_line_per_complete_kind_tool() {
        let full = crate::root::kind_tool_names(true);
        let body = product(&full);
        let can = body["can"].as_array().expect("can 数组");
        let names: Vec<&str> = can
            .iter()
            .filter_map(Value::as_str)
            .map(|line| line.split('：').next().unwrap_or(""))
            .collect();
        assert_eq!(names.len(), full.len(), "每个 Kind 工具一行");
        for tool in &full {
            assert!(names.contains(&tool.as_str()), "工具面漏了 {tool}");
            assert!(
                can.iter().any(|line| {
                    line.as_str()
                        .is_some_and(|text| text.starts_with(&(tool.clone() + "：")))
                }),
                "工具面缺一行 {tool}"
            );
        }
        for name in names {
            assert!(full.iter().any(|tool| tool == name), "名单外工具 {name}");
        }
        for task_tool in [
            "task_accept",
            "task_status",
            "task_delivery",
            "task_stop",
            "task_update",
            "task_graph_propose",
            "task_continue",
            "gate_check_again",
            "appointment_cancel",
        ] {
            assert!(
                full.iter().any(|tool| tool == task_tool),
                "缺任务工具 {task_tool}"
            );
        }
        let base = product(&crate::root::kind_tool_names(false));
        assert!(
            !base["can"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .any(|line| line.starts_with("task_accept：")),
            "没任务运行时不能列任务工具"
        );
        let cannot = body["cannot"].as_array().expect("cannot 数组");
        assert_eq!(cannot.len(), 3, "不能只留永远成立的三条");
        let text = serde_json::to_string(cannot).unwrap();
        assert!(text.contains("建连接"));
        assert!(text.contains("填 key"));
        assert!(text.contains("绕过确认卡"));
    }

    fn wait(kind: &str, task: &str, title: &str) -> WaitItem {
        WaitItem {
            kind: kind.to_owned(),
            task_id: Some(task.to_owned()),
            title: title.to_owned(),
            stage: Some("ready_for_review".to_owned()),
            need: "要你".to_owned(),
            body: "原因原文".to_owned(),
            at: 1_700_000_000_000,
            turn: None,
            also: Vec::new(),
            pick: None,
            write: None,
            version: None,
            appointment_id: None,
        }
    }

    #[test]
    fn work_columns_follow_the_shared_web_grouping_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../web/src/pages/work/work-grouping.fixture.json"
        ))
        .unwrap();
        let wait_ids: std::collections::HashSet<&str> = fixture["waitTaskIds"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut groups = serde_json::Map::new();
        for id in ["wait", "queue", "doing", "done"] {
            groups.insert(id.to_owned(), json!([]));
        }
        for task in fixture["tasks"].as_array().unwrap() {
            let id = task["id"].as_str().unwrap();
            let stage = task["stage"].as_str().unwrap();
            let gate = task["gate"].as_bool().unwrap();
            let column = column_of(id, stage, gate, &wait_ids);
            groups[column].as_array_mut().unwrap().push(json!(id));
        }
        for key in ["wait", "queue", "doing", "done"] {
            assert_eq!(
                Value::Object(groups.clone())[key],
                fixture["groups"][key],
                "组 {key} 与页面夹具不一致"
            );
        }
        // 不自己复写过滤条件：拿同一份夹具跑产品投影，取 problems 的 id 比。
        let tasks: Vec<TaskFact> = fixture["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|task| {
                let mut fact = task_fact(
                    task["id"].as_str().unwrap(),
                    task["stage"].as_str().unwrap(),
                    task["afterWaiting"].as_bool() == Some(true),
                );
                fact.gate = task["gate"].as_bool().unwrap();
                fact.closed_at = task["closedAt"].as_i64();
                fact.cancelled_at = task["cancelledAt"].as_i64();
                fact
            })
            .collect();
        let waits: Vec<WaitItem> = fixture["waitTaskIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| {
                serde_json::from_value(json!({
                    "kind": "accept",
                    "taskId": id.as_str().unwrap(),
                    "title": id.as_str().unwrap(),
                    "need": "等你",
                    "body": "夹具",
                    "at": 0,
                }))
                .unwrap()
            })
            .collect();
        let response = WorkResponse {
            generated_at: fixture["now"].as_i64().unwrap(),
            tasks,
            wait: waits,
            graphs: Vec::new(),
        };
        let projected = project_work(&response, response.generated_at);
        let problems: Vec<&str> = projected["problems"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        assert_eq!(
            json!(problems),
            fixture["problems"],
            "出了问题的任务两边都在"
        );
        let now = fixture["now"].as_i64().unwrap();
        let week_start = week_start_ms(now);
        let done: Vec<&str> = fixture["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|task| {
                done_this_week(
                    task["stage"].as_str().unwrap(),
                    task["gate"].as_bool().unwrap(),
                    task["cancelledAt"].as_i64(),
                    task["closedAt"].as_i64(),
                    week_start,
                )
            })
            .map(|task| task["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            json!(done),
            fixture["doneThisWeek"],
            "本周完成与页面夹具不一致"
        );
    }

    #[test]
    fn work_topic_merges_calendar_pause_wait_like_page_snapshot() {
        use std::sync::Arc;

        let data = tempfile::tempdir().unwrap();
        let now = 1_700_000_000_000_i64;
        let calendar = calendar::CalendarStore::open(data.path().join("calendar.sqlite")).unwrap();
        let appointment = calendar
            .create_appointment(calendar::NewAppointment {
                title: "循环约定".to_owned(),
                condition: "列给我".to_owned(),
                schedule: calendar::Schedule {
                    rule: "FREQ=DAILY".to_owned(),
                    timezone: "Asia/Shanghai".to_owned(),
                    start_at: now,
                },
                binding: calendar::Binding::Task("task-cycle".to_owned()),
                watch: None,
                cycle: Some(calendar::CycleSpec {
                    task_id: "task-cycle".to_owned(),
                    max_rounds: None,
                }),
                duration_minutes: None,
            })
            .unwrap();
        let paused = calendar
            .pause_with_reason(&appointment.id, "等你确认下一轮")
            .unwrap();
        let store = tasks::Store::open(data.path().join("tasks.sqlite")).unwrap();
        let mut page = crate::work::snapshot(&store, &[], &[], now).unwrap();
        crate::work::merge_waits(
            &mut page,
            crate::work::calendar_pause_waits(&calendar).unwrap(),
        );
        assert_eq!(page.wait.len(), 1, "页面会合并日历暂停项");

        let tool = QueryTool::new(data.path(), None);
        let context = ToolContext::with_session(
            data.path(),
            Arc::new(tools::ToolSessionState::new(data.path(), "test")),
        );
        let projected = tool.work(&context).unwrap();
        assert_eq!(projected["counts"]["wait"], page.wait.len());
        assert_eq!(projected["wait"][0]["appointmentId"], paused.id);
        assert!(
            projected["wait"][0]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("等你确认下一轮"))
        );
    }

    #[test]
    fn work_projection_keeps_page_wait_count_and_no_cancel_or_graph_edges() {
        let response = WorkResponse {
            generated_at: 1_700_000_000_000,
            tasks: Vec::new(),
            wait: vec![
                wait("accept", "task-a", "任务甲"),
                wait("confirm", "task-b", "任务乙"),
            ],
            graphs: Vec::new(),
        };
        let projected = project_work(&response, response.generated_at);
        assert_eq!(projected["counts"]["wait"], 2);
        assert_eq!(projected["wait"].as_array().unwrap().len(), 2);
        let text = projected.to_string();
        assert!(!text.contains("edges"));
        assert!(!text.contains("edges"));
        assert!(!text.contains("payload"));
        assert!(text.contains("任务甲"));
        assert!(text.contains("原因原文"));
        assert!(text.contains("actionOwner"));
    }

    fn task_fact(id: &str, stage: &str, after_waiting: bool) -> TaskFact {
        serde_json::from_value(json!({
            "id": id,
            "title": id,
            "stage": stage,
            "version": 1,
            "attempts": 0,
            "createdAt": 1_699_000_000_000_i64,
            "running": false,
            "gate": false,
            "after": [],
            "afterWaiting": after_waiting,
        }))
        .unwrap()
    }

    #[test]
    fn projection_queue_is_the_column_including_problem_tasks() {
        let response = WorkResponse {
            generated_at: 1_700_000_000_000,
            tasks: vec![
                task_fact("problem-1", "needs_conditions", false),
                task_fact("queue-1", "pending", false),
            ],
            wait: Vec::new(),
            graphs: Vec::new(),
        };
        let projected = project_work(&response, response.generated_at);
        let ids = |key: &str| -> Vec<String> {
            projected[key]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item["id"].as_str().map(str::to_owned))
                .collect()
        };
        assert_eq!(ids("problems"), vec!["problem-1"]);
        assert_eq!(ids("queue"), vec!["problem-1", "queue-1"]);
        assert_eq!(projected["counts"]["queue"], 2);
        assert_eq!(projected["columns"]["queue"].as_array().unwrap().len(), 2);
    }
}
